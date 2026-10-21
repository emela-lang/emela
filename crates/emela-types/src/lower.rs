//! Converts HIR type expressions ([`TypeRef`]) into [`Ty`].
//!
//! Registers type, enum and error definitions in [`TyCons`] to build the type table, then looks
//! type expressions up in it.
//!
//! [`TypeRef`]: emela_resolve::hir::TypeRef

use emela_resolve::hir::{
    DefId, DefKind, EffectItem, EffectRef, EffectSet, EnumItem, ErrorSet, Fields, Item, Program,
    Res, Span, TypeId, TypeItem, TypeKind, TypeParam, TypeRef,
};
use la_arena::ArenaMap;
use rustc_hash::FxHashMap;
use smol_str::SmolStr;

use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::infer::InferCtx;
use crate::row::{Row, RowElem, SetKind, Tag};
use crate::ty::{ParamKind, Ty, TyConData, TyConId, TyConKind, TyCons, TyParam};

/// What omitted `fails` and `use` clauses on a function type mean, decided by where they are
/// written (spec 9.2).
pub(crate) enum Omitted<'a> {
    /// Inside a declaration (fields, consts, ...): empty.
    Empty,
    /// Inside a top-level fn's parameter types: adds an invisible set type parameter.
    Hidden(&'a mut Hidden),
    /// Return type and binding annotations, lambda parameters: inferred from the body, so a fresh
    /// set variable. Lambdas are not generalized, so their parameters get set variables rather
    /// than invisible set type parameters.
    Fresh(&'a mut InferCtx),
}

/// Invisible set type parameters inside parameter types (spec 9.2), placed after the written
/// type parameters.
pub(crate) struct Hidden {
    /// The number of written type parameters; invisible ones are indexed after these.
    base: usize,
    pub(crate) params: Vec<TyParam>,
    /// The current parameter's name, used to name invisible set type parameters (`E(f)`, `R(f)`).
    param: SmolStr,
    /// How many error sets and effect sets were added for the current parameter.
    in_param: [usize; 2],
    /// Invisible error sets in positive positions of parameter types (functions the body can
    /// call). Included in the declared E.
    pub(crate) positive: Row,
    /// Likewise for invisible effect sets; included in the declared R.
    pub(crate) positive_uses: Row,
    /// Whether invisible effect sets get an `Immediate` bound. Callbacks of external functions
    /// must be Immediate (spec 9.5).
    pub(crate) immediate: bool,
}

impl Hidden {
    pub(crate) fn new(base: usize) -> Self {
        Hidden {
            base,
            params: Vec::new(),
            param: SmolStr::default(),
            in_param: [0, 0],
            positive: Row::empty(),
            positive_uses: Row::empty(),
            immediate: false,
        }
    }

    pub(crate) fn start_param(&mut self, name: SmolStr) {
        self.param = name;
        self.in_param = [0, 0];
    }

    /// Adds an invisible set type parameter: an effect set (`R(f)`) if `effects`, otherwise an
    /// error set (`E(f)`).
    fn fresh(&mut self, positive: bool, effects: bool) -> Row {
        let (prefix, count) = if effects {
            ("R", &mut self.in_param[1])
        } else {
            ("E", &mut self.in_param[0])
        };
        *count += 1;
        let name = match *count {
            1 => format!("{prefix}({})", self.param),
            n => format!("{prefix}({}, {n})", self.param),
        };
        let i = (self.base + self.params.len()) as u32;
        let mut param = TyParam::set(name);
        if effects && self.immediate {
            param.bounds.push(SmolStr::new(IMMEDIATE));
        }
        self.params.push(param);
        let row = Row::from_elems([RowElem::Bound(i)]);
        if positive {
            if effects {
                self.positive_uses.extend(&row);
            } else {
                self.positive.extend(&row);
            }
        }
        row
    }
}

/// Context for reading type expressions: type parameters (`params[i]` is `Bound(i)`), their
/// kinds, and what an omitted `fails` means.
pub(crate) struct Scope<'a> {
    pub(crate) params: &'a [DefId],
    pub(crate) kinds: &'a [ParamKind],
    pub(crate) omitted: Omitted<'a>,
    /// Whether set type parameters may be written. Type parameters of declarations such as
    /// `type` cannot be sets (not supported yet); the same holds inside effect operation
    /// declarations.
    pub(crate) sets: bool,
    /// What `Self` stands for: `Bound(0)` in a trait's functions, the impl's type in an impl.
    /// `None` elsewhere, where name resolution has reported `Self` (E0219).
    pub(crate) self_ty: Option<&'a Ty>,
}

impl Scope<'_> {
    /// The set for an omitted `fails` clause (`use` clause if `effects`).
    fn omitted_row(&mut self, positive: bool, effects: bool) -> Row {
        match &mut self.omitted {
            Omitted::Empty => Row::empty(),
            Omitted::Hidden(hidden) => hidden.fresh(positive, effects),
            Omitted::Fresh(ctx) => Row::var(ctx.new_row_var()),
        }
    }
}

/// Built from type, enum and error definitions.
pub struct TyTable {
    /// Used to look up names when displaying types.
    pub cons: TyCons,
    by_def: ArenaMap<DefId, TyConId>,
    /// Prelude's Bool enum. Not registered in TyCons; maps to Ty::BOOL.
    bool_def: Option<DefId>,
    /// The inverse of `by_def`.
    def_of: ArenaMap<TyConId, DefId>,
    /// The set kinds of type and enum type parameters (`type Handler[E, R]` in spec 9.2).
    sets: DeclSets,
}

impl TyTable {
    pub fn new(program: &Program) -> Self {
        let mut cons = TyCons::new();
        let mut by_def = ArenaMap::default();
        let mut def_of = ArenaMap::default();
        let bool_def = program.prelude_def("Bool");
        let sets = decl_sets(program);

        for (def, data) in program.defs.iter() {
            let kind = match data.kind {
                DefKind::Type => TyConKind::Type,
                DefKind::Enum => TyConKind::Enum,
                DefKind::Error => {
                    cons.set_tag_name(def, data.name.clone());
                    TyConKind::Error
                }
                DefKind::Effect => {
                    cons.set_tag_name(def, data.name.clone());
                    continue;
                }
                DefKind::Handler => TyConKind::Handler,
                _ => continue,
            };
            if bool_def == Some(def) {
                continue;
            }

            let id = cons.alloc(TyConData {
                name: data.name.clone(),
                kind,
                params: decl_params(program, def, sets.get(def).map_or(&[], Vec::as_slice)),
            });

            by_def.insert(def, id);
            def_of.insert(id, def);
        }

        TyTable {
            cons,
            by_def,
            bool_def,
            def_of,
            sets,
        }
    }

    pub(crate) fn decl_sets(&self) -> &DeclSets {
        &self.sets
    }

    /// The set kind of the `i`-th type parameter of a type, enum or built-in effect, if it is a
    /// set.
    pub(crate) fn set_kind(&self, def: DefId, i: usize) -> Option<SetKind> {
        self.sets.get(def).and_then(|s| s.get(i).copied().flatten())
    }

    /// `None` for Bool and for definitions other than type, enum, error and handler.
    pub fn con(&self, def: DefId) -> Option<TyConId> {
        self.by_def.get(def).copied()
    }

    pub fn def(&self, id: TyConId) -> DefId {
        self.def_of[id]
    }

    /// Named error type → that error's definition.
    pub(crate) fn error_tags(&self) -> FxHashMap<TyConId, DefId> {
        self.cons
            .iter()
            .filter(|(_, data)| data.kind == TyConKind::Error)
            .map(|(id, _)| (id, self.def_of[id]))
            .collect()
    }

    /// `params` are the declaration's type parameters; the one at position i becomes
    /// `Ty::Bound(i)`. Reads the type as inside a declaration (omitted `fails` on a function type
    /// is empty; type parameters are ordinary types).
    pub fn lower(
        &self,
        program: &Program,
        params: &[DefId],
        ty: TypeId,
        diags: &mut Vec<Diagnostic>,
    ) -> Ty {
        let kinds = vec![ParamKind::Type; params.len()];
        let mut scope = Scope {
            params,
            kinds: &kinds,
            omitted: Omitted::Empty,
            sets: false,
            self_ty: None,
        };
        self.lower_in(program, &mut scope, ty, true, diags)
    }

    /// Converts in the context `scope`. `positive` tells whether the position receives values as
    /// seen from the body (it flips inside function parameter types); it decides whether
    /// invisible set type parameters join the declared E.
    pub(crate) fn lower_in(
        &self,
        program: &Program,
        scope: &mut Scope<'_>,
        ty: TypeId,
        positive: bool,
        diags: &mut Vec<Diagnostic>,
    ) -> Ty {
        let type_ref = &program.types[ty];
        match &type_ref.kind {
            TypeKind::Missing => Ty::Error,
            // Sets may only be written as set type arguments of named types (`lower_set_arg`).
            TypeKind::ErrorSet(_) | TypeKind::EffectSet(_) => {
                diags.push(Diagnostic {
                    span: type_ref.span,
                    kind: DiagnosticKind::SetAsType,
                });
                Ty::Error
            }
            TypeKind::Unit => Ty::UNIT,
            TypeKind::SelfType => scope.self_ty.cloned().unwrap_or(Ty::Error),
            TypeKind::Tuple(elems) => Ty::tuple(
                elems
                    .iter()
                    .map(|&e| self.lower_in(program, scope, e, positive, diags))
                    .collect::<Vec<_>>(),
            ),
            TypeKind::Fn {
                params: fn_params,
                ret,
                fails,
                uses,
            } => {
                // Invisible set type parameters are numbered from the outer function type (for
                // `f: fn(fn() -> A) -> B`, f's own E is `E(f)`).
                let fails = match fails {
                    Some(set) => self.lower_error_set(program, scope, set, diags),
                    None => scope.omitted_row(positive, false),
                };
                let uses = match uses {
                    Some(set) => self.lower_effect_set(program, scope, set, diags),
                    None => scope.omitted_row(positive, true),
                };
                let ret_ty = match ret {
                    Some(ret) => self.lower_in(program, scope, *ret, positive, diags),
                    None => Ty::Error,
                };
                let param_tys: Vec<Ty> = fn_params
                    .iter()
                    .map(|&p| self.lower_in(program, scope, p, !positive, diags))
                    .collect();
                Ty::func_sets(param_tys, ret_ty, fails, uses)
            }
            TypeKind::Path { res, args } => {
                self.lower_path(program, scope, type_ref, *res, args, positive, diags)
            }
        }
    }

    /// The set of a `fails` clause. Errors become tags, set type parameters `RowElem::Bound`, and
    /// type parameters also used as values the error type (`RowElem::Of`).
    pub(crate) fn lower_error_set(
        &self,
        program: &Program,
        scope: &Scope<'_>,
        set: &ErrorSet,
        diags: &mut Vec<Diagnostic>,
    ) -> Row {
        let mut row = Row::empty();
        for &(res, span) in &set.items {
            let Res::Def(def) = res else {
                continue;
            };
            match program.defs[def].kind {
                DefKind::Error => row.insert(RowElem::Tag(Tag::new(def))),
                DefKind::TypeParam => {
                    let Some(i) = scope.params.iter().position(|&p| p == def) else {
                        continue;
                    };
                    match scope.kinds.get(i) {
                        // Type parameters of declarations such as `type` cannot be sets (not
                        // supported yet).
                        Some(ParamKind::Set) if !scope.sets => {
                            diags.push(Diagnostic {
                                span: Some(span),
                                kind: DiagnosticKind::Unsupported {
                                    what: "error set type parameters on type declarations",
                                },
                            });
                        }
                        Some(ParamKind::Set) => row.insert(RowElem::Bound(i as u32)),
                        _ => row.insert(RowElem::Of(Ty::Bound(i as u32))),
                    }
                }
                _ => {}
            }
        }
        row
    }

    /// The set of a `use` clause. Effects become tags, set type parameters `RowElem::Bound`.
    pub(crate) fn lower_effect_set(
        &self,
        program: &Program,
        scope: &Scope<'_>,
        set: &EffectSet,
        diags: &mut Vec<Diagnostic>,
    ) -> Row {
        let mut row = Row::empty();
        for item in &set.items {
            let (res, span) = (item.res, item.span);
            let Res::Def(def) = res else {
                continue;
            };
            match program.defs[def].kind {
                DefKind::Effect => {
                    if let Some(tag) =
                        self.effect_tag(program, scope, def, &item.args, Some(span), None, diags)
                    {
                        row.insert(RowElem::Tag(tag));
                    }
                }
                DefKind::TypeParam => {
                    let Some(i) = scope.params.iter().position(|&p| p == def) else {
                        continue;
                    };
                    match scope.kinds.get(i) {
                        Some(ParamKind::Set) if scope.sets => row.insert(RowElem::Bound(i as u32)),
                        Some(ParamKind::Set) => diags.push(Diagnostic {
                            span: Some(span),
                            kind: DiagnosticKind::Unsupported {
                                what: "effect set type parameters on type declarations",
                            },
                        }),
                        // A type parameter also written as a value type is not an effect set.
                        _ => diags.push(Diagnostic {
                            span: Some(span),
                            kind: DiagnosticKind::Unsupported {
                                what: "type parameters used both as a type and in `use`",
                            },
                        }),
                    }
                }
                _ => {}
            }
        }
        row
    }

    /// The tag of an effect with its type arguments (spec 8.1). Type arguments are read as
    /// ordinary types; omitted `fails` and `use` on function types inside them are empty (as in
    /// declarations).
    ///
    /// Missing type arguments on an effect with type parameters are E0423 (`handler` names the
    /// handler when written in a handler head); a wrong count is E0304. Both return a tag with the
    /// arguments filled with `Ty::Error`, which unifies with anything so errors do not cascade.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn effect_tag(
        &self,
        program: &Program,
        scope: &Scope<'_>,
        effect: DefId,
        args: &[TypeId],
        span: Option<Span>,
        handler: Option<SmolStr>,
        diags: &mut Vec<Diagnostic>,
    ) -> Option<Tag> {
        let params = effect_params(program, effect);
        if params.is_empty() && args.is_empty() {
            return Some(Tag::new(effect));
        }
        let name = &program.defs[effect].name;
        if args.is_empty() {
            diags.push(Diagnostic {
                span,
                kind: DiagnosticKind::MissingEffectTypeArgs {
                    effect: name.clone(),
                    params: params
                        .iter()
                        .map(|tp| program.defs[tp.def].name.clone())
                        .collect(),
                    handler,
                },
            });
            return Some(Tag::with_args(effect, vec![Ty::Error; params.len()]));
        }
        if !check_arity(name, params.len(), args, span, diags) {
            return Some(Tag::with_args(effect, vec![Ty::Error; params.len()]));
        }
        let mut inner = Scope {
            params: scope.params,
            kinds: scope.kinds,
            omitted: Omitted::Empty,
            sets: scope.sets,
            self_ty: None,
        };
        // Set type parameters of built-in effects (the `E` and `R` of `Scope[E, R]`) take sets,
        // written like set type arguments of named types (spec 8.1, 9.2).
        let args = args
            .iter()
            .enumerate()
            .map(|(i, &a)| match self.set_kind(effect, i) {
                Some(kind) => self.lower_set_arg(program, &inner, effect, i, kind, a, diags),
                None => self.lower_in(program, &mut inner, a, true, diags),
            })
            .collect();
        Some(Tag::with_args(effect, args))
    }

    pub fn lower_all(
        &self,
        program: &Program,
        params: &[DefId],
        tys: impl IntoIterator<Item = TypeId>,
        diags: &mut Vec<Diagnostic>,
    ) -> Vec<Ty> {
        tys.into_iter()
            .map(|ty| self.lower(program, params, ty, diags))
            .collect()
    }

    /// `Ty::Error` if no type is written.
    pub fn lower_or_error(
        &self,
        program: &Program,
        params: &[DefId],
        ty: Option<TypeId>,
        diags: &mut Vec<Diagnostic>,
    ) -> Ty {
        match ty {
            Some(ty) => self.lower(program, params, ty, diags),
            None => Ty::Error,
        }
    }

    /// The type a type, enum, error or handler definition names, with type arguments filled as
    /// `Bound(0)`, `Bound(1)`, .... Bool is `Ty::BOOL`. `None` for other definitions.
    pub fn self_ty(&self, def: DefId) -> Option<Ty> {
        if self.bool_def == Some(def) {
            return Some(Ty::BOOL);
        }

        let id = self.con(def)?;
        let n = self.cons[id].params.len();

        Some(Ty::named(
            id,
            (0..n).map(|i| match self.set_kind(def, i) {
                Some(kind) => Ty::Set(kind, Row::from_elems([RowElem::Bound(i as u32)])),
                None => Ty::Bound(i as u32),
            }),
        ))
    }

    /// Named types (`Int`, `List[A]`, `Option[Int]`, `A`).
    #[allow(clippy::too_many_arguments)]
    fn lower_path(
        &self,
        program: &Program,
        scope: &mut Scope<'_>,
        type_ref: &TypeRef,
        res: Res,
        args: &[TypeId],
        positive: bool,
        diags: &mut Vec<Diagnostic>,
    ) -> Ty {
        let params = scope.params;
        let Res::Def(def) = res else {
            return Ty::Error;
        };
        let data = &program.defs[def];
        let span = type_ref.span;
        if data.kind == DefKind::Effect {
            diags.push(Diagnostic {
                span,
                kind: DiagnosticKind::EffectAsType {
                    name: data.name.clone(),
                },
            });
            return Ty::Error;
        }

        let expected = match data.kind {
            DefKind::BuiltinType if data.name == "List" => 1,
            DefKind::Type | DefKind::Enum | DefKind::Error => match self.by_def.get(def) {
                Some(&id) => self.cons[id].params.len(),
                None => 0,
            },
            _ => 0,
        };

        if !check_arity(&data.name, expected, args, span, diags) {
            return Ty::Error;
        }

        if self.bool_def == Some(def) {
            return Ty::BOOL;
        }

        match data.kind {
            DefKind::TypeParam => params
                .iter()
                .position(|&p| p == def)
                .map_or(Ty::Error, |i| Ty::Bound(i as u32)),
            DefKind::BuiltinType => match data.name.as_str() {
                "Int" => Ty::INT,
                "Int64" => Ty::INT64,
                "Float" => Ty::FLOAT,
                "String" => Ty::STRING,
                "Never" => Ty::Never,
                "List" => Ty::list(self.lower_in(program, scope, args[0], positive, diags)),
                _ => {
                    diags.push(Diagnostic {
                        span,
                        kind: DiagnosticKind::Unsupported {
                            what: "builtin type",
                        },
                    });
                    Ty::Error
                }
            },
            DefKind::Type | DefKind::Enum | DefKind::Error => {
                let Some(&id) = self.by_def.get(def) else {
                    return Ty::Error;
                };
                let args: Vec<Ty> = args
                    .iter()
                    .enumerate()
                    .map(|(i, &a)| match self.set_kind(def, i) {
                        Some(kind) => self.lower_set_arg(program, scope, def, i, kind, a, diags),
                        None => self.lower_in(program, scope, a, positive, diags),
                    })
                    .collect();
                Ty::named(id, args)
            }
            _ => Ty::Error,
        }
    }
}

/// Pushes the diagnostic for something that does not fit a set type parameter (E0323). Takes the
/// span and a description of what was written.
type ReportMismatch<'a> = dyn Fn(Option<Span>, SmolStr, &mut Vec<Diagnostic>) + 'a;

impl TyTable {
    /// Converts what is written as a set type argument of a named type into a set
    /// (`Handler[NotFound, Db]`, `Handler[NotFound | Timeout, { Db, Log }]`, `Handler[E, R]`,
    /// `Handler[{}, {}]`). Elements may be tags of the right kind (error or effect names) and set
    /// type parameters. Error sets are joined with `|`; effect sets are wrapped in braces. An empty
    /// `{}` fits either position. Anything else is E0323, and that element is skipped.
    #[allow(clippy::too_many_arguments)]
    fn lower_set_arg(
        &self,
        program: &Program,
        scope: &Scope<'_>,
        decl: DefId,
        index: usize,
        kind: SetKind,
        arg: TypeId,
        diags: &mut Vec<Diagnostic>,
    ) -> Ty {
        let mismatch = |span: Option<Span>, found: SmolStr, diags: &mut Vec<Diagnostic>| {
            diags.push(Diagnostic {
                span,
                kind: DiagnosticKind::SetArgMismatch {
                    param: type_param_defs(program, decl)
                        .get(index)
                        .map(|&p| program.defs[p].name.clone())
                        .unwrap_or_default(),
                    ty: program.defs[decl].name.clone(),
                    effects: kind == SetKind::Effects,
                    found,
                },
            });
        };
        let type_ref = &program.types[arg];
        let items: Vec<(Res, &[TypeId], Span)> = match (&type_ref.kind, kind) {
            (TypeKind::ErrorSet(set), SetKind::Errors) => set
                .items
                .iter()
                .map(|&(res, span)| (res, &[][..], span))
                .collect(),
            (TypeKind::EffectSet(set), SetKind::Effects) => set
                .items
                .iter()
                .map(|item| (item.res, item.args.as_slice(), item.span))
                .collect(),
            (TypeKind::EffectSet(set), SetKind::Errors) if set.items.is_empty() => Vec::new(),
            (TypeKind::ErrorSet(_), SetKind::Effects) => {
                mismatch(type_ref.span, "an error set".into(), diags);
                return Ty::Set(kind, Row::empty());
            }
            (TypeKind::EffectSet(_), SetKind::Errors) => {
                mismatch(type_ref.span, "an effect set".into(), diags);
                return Ty::Set(kind, Row::empty());
            }
            (TypeKind::Missing, _) => return Ty::Set(kind, Row::empty()),
            (TypeKind::Path { res, args }, _)
                if args.is_empty()
                    || (kind == SetKind::Effects
                        && matches!(res, Res::Def(d) if program.defs[*d].kind == DefKind::Effect)) =>
            {
                return Ty::Set(
                    kind,
                    self.set_elem(
                        program,
                        scope,
                        kind,
                        (*res, args),
                        type_ref.span,
                        &mismatch,
                        diags,
                    ),
                );
            }
            (
                TypeKind::Path {
                    res: Res::Def(def), ..
                },
                _,
            ) => {
                let found = format!("type `{}`", program.defs[*def].name);
                mismatch(type_ref.span, found.into(), diags);
                return Ty::Set(kind, Row::empty());
            }
            _ => {
                mismatch(type_ref.span, "a type".into(), diags);
                return Ty::Set(kind, Row::empty());
            }
        };
        let mut row = Row::empty();
        for (res, args, span) in items {
            row.extend(&self.set_elem(
                program,
                scope,
                kind,
                (res, args),
                Some(span),
                &mismatch,
                diags,
            ));
        }
        Ty::Set(kind, row)
    }

    /// One set element: a tag of the right kind or a set type parameter. `elem` is the written
    /// name and the effect's type arguments.
    #[allow(clippy::too_many_arguments)]
    fn set_elem(
        &self,
        program: &Program,
        scope: &Scope<'_>,
        kind: SetKind,
        elem: (Res, &[TypeId]),
        span: Option<Span>,
        mismatch: &ReportMismatch<'_>,
        diags: &mut Vec<Diagnostic>,
    ) -> Row {
        let (res, args) = elem;
        // Name resolution has reported this.
        let Res::Def(def) = res else {
            return Row::empty();
        };
        let data = &program.defs[def];
        match (data.kind, kind) {
            (DefKind::Error, SetKind::Errors) => Row::tag(def),
            (DefKind::Effect, SetKind::Effects) => {
                match self.effect_tag(program, scope, def, args, span, None, diags) {
                    Some(tag) => Row::of_tag(tag),
                    None => Row::empty(),
                }
            }
            (DefKind::TypeParam, _) => match scope.params.iter().position(|p| *p == def) {
                Some(i) if scope.kinds.get(i) == Some(&ParamKind::Set) && scope.sets => {
                    Row::from_elems([RowElem::Bound(i as u32)])
                }
                Some(i) if scope.kinds.get(i) == Some(&ParamKind::Set) => {
                    diags.push(Diagnostic {
                        span,
                        kind: DiagnosticKind::Unsupported {
                            what: "set type parameters here",
                        },
                    });
                    Row::empty()
                }
                _ => {
                    mismatch(
                        span,
                        format!("type parameter `{}`", data.name).into(),
                        diags,
                    );
                    Row::empty()
                }
            },
            (DefKind::Effect, _) => {
                mismatch(span, format!("effect `{}`", data.name).into(), diags);
                Row::empty()
            }
            (DefKind::Error, _) => {
                mismatch(span, format!("error `{}`", data.name).into(), diags);
                Row::empty()
            }
            _ => {
                mismatch(span, format!("type `{}`", data.name).into(), diags);
                Row::empty()
            }
        }
    }
}

/// Type parameters of a type or enum declaration. Set type parameters become `ParamKind::Set`,
/// with an Immediate bound on effect sets if written (spec 9.5).
fn decl_params(program: &Program, def: DefId, sets: &[Option<SetKind>]) -> Vec<TyParam> {
    let type_params: &[TypeParam] = match program.item(def) {
        Some(Item::Type(TypeItem { type_params, .. }))
        | Some(Item::Enum(EnumItem { type_params, .. })) => type_params,
        _ => &[],
    };
    type_params
        .iter()
        .enumerate()
        .map(|(i, tp)| {
            let name = program.defs[tp.def].name.clone();
            match sets.get(i).copied().flatten() {
                Some(kind) => {
                    let mut p = TyParam::set(name);
                    if kind == SetKind::Effects && !immediate_bounds(program, tp).is_empty() {
                        p.bounds.push(SmolStr::new(IMMEDIATE));
                    }
                    p
                }
                None => TyParam::new(name),
            }
        })
        .collect()
}

/// Returns true if the type argument count is `expected`; otherwise pushes E0304 and returns
/// false.
///
/// Checked here rather than in name resolution, because the type checker is what knows the
/// arity of built-in types (`List`).
fn check_arity(
    name: &SmolStr,
    expected: usize,
    args: &[TypeId],
    span: Option<Span>,
    diags: &mut Vec<Diagnostic>,
) -> bool {
    if args.len() != expected {
        diags.push(Diagnostic {
            span,
            kind: DiagnosticKind::WrongTypeArgCount {
                name: name.clone(),
                expected,
                found: args.len(),
            },
        });
        false
    } else {
        true
    }
}

/// Type parameter definitions of a type, enum or effect; empty for anything else.
pub(crate) fn type_param_defs(program: &Program, def: DefId) -> Vec<DefId> {
    let type_params: &[TypeParam] = match program.item(def) {
        Some(Item::Type(TypeItem { type_params, .. }))
        | Some(Item::Enum(EnumItem { type_params, .. }))
        | Some(Item::Effect(EffectItem { type_params, .. })) => type_params,
        _ => &[],
    };

    type_params.iter().map(|tp| tp.def).collect()
}

/// The name of the built-in Immediate bound (spec 9.5).
pub(crate) const IMMEDIATE: &str = "Immediate";

/// Effect type parameters (spec 8.1). Empty for effects without them and for non-effects.
pub(crate) fn effect_params(program: &Program, effect: DefId) -> &[TypeParam] {
    match program.item(effect) {
        Some(Item::Effect(e)) => &e.type_params,
        _ => &[],
    }
}

/// The names given to schemes and type declarations, from type parameter definitions.
pub(crate) fn ty_params(program: &Program, defs: &[DefId]) -> Vec<TyParam> {
    defs.iter()
        .map(|&def| TyParam::new(program.defs[def].name.clone()))
        .collect()
}

/// The spans of a type parameter's bounds that refer to the built-in Immediate.
pub(crate) fn immediate_bounds(program: &Program, tp: &TypeParam) -> Vec<Span> {
    let immediate = program.prelude_def(IMMEDIATE);
    tp.bounds
        .iter()
        .filter(|(res, _)| matches!(res, Res::Def(d) if Some(*d) == immediate))
        .map(|&(_, span)| span)
        .collect()
}

/// Whether an effect is a built-in effect of the bundled sources (`@intrinsic effect Scope`,
/// spec 11.2). Only built-in effects may have set type parameters and operation type parameters,
/// and no handler may implement them (spec 8.1).
pub(crate) fn is_intrinsic_effect(program: &Program, effect: DefId) -> bool {
    matches!(program.item(effect), Some(Item::Effect(e)) if e.is_intrinsic)
}

/// Whether an effect is Immediate (spec 9.5): it has no suspend fn and is not Async. An effect
/// whose declaration is unknown (name resolution has reported it) counts as Immediate, so no
/// further error follows from it.
pub(crate) fn is_immediate_effect(program: &Program, effect: DefId) -> bool {
    if program.prelude_def("Async") == Some(effect) {
        return false;
    }
    // A label is Immediate when its base is (issue #255).
    program
        .effect_ops(effect)
        .iter()
        .all(|&op| !matches!(program.item(op), Some(Item::Op(o)) if o.sig.is_suspend))
}

/// The set kind of each type parameter of a named type declaration (`None` if not a set). Keyed
/// by type and enum definitions.
pub(crate) type DeclSets = ArenaMap<DefId, Vec<Option<SetKind>>>;

/// Decides type parameter kinds from where they are used (spec 9.2). A type parameter written only
/// in `fails` or `use` clauses is a set; otherwise an ordinary type (written both as a value type
/// and in `fails`, it is the type of a single error, spec 7.2). Type parameters written as set
/// type arguments of named types (the `E` of `Handler[E, R]`) are sets too. `types` are the type
/// expressions where type parameters may occur (parameters, return type, field types).
pub(crate) fn classify_params(
    program: &Program,
    decls: &DeclSets,
    params: &[DefId],
    types: impl IntoIterator<Item = TypeId>,
) -> Vec<ParamKind> {
    classify_sets(program, decls, params, types)
        .into_iter()
        .map(|set| match set {
            Some(_) => ParamKind::Set,
            None => ParamKind::Type,
        })
        .collect()
}

/// Decides like [`classify_params`], returning the set kind (error or effect set) for sets. A type
/// parameter written in both is given the kind found first.
pub(crate) fn classify_sets(
    program: &Program,
    decls: &DeclSets,
    params: &[DefId],
    types: impl IntoIterator<Item = TypeId>,
) -> Vec<Option<SetKind>> {
    classify_fn_sets(program, decls, params, types, None)
}

/// Decides like [`classify_sets`], also counting the set type arguments of built-in effects in a
/// fn's own `use` clause (the `E` and `R` of `fn deferred[A, E, R]() -> Deferred[A] use
/// Scope[E, R]`, spec 11.2).
pub(crate) fn classify_fn_sets(
    program: &Program,
    decls: &DeclSets,
    params: &[DefId],
    types: impl IntoIterator<Item = TypeId>,
    uses: Option<&EffectSet>,
) -> Vec<Option<SetKind>> {
    let mut marks = Marks {
        in_type: vec![false; params.len()],
        in_set: vec![None; params.len()],
    };
    for ty in types {
        mark_params(program, decls, params, ty, &mut marks);
    }
    let position = |res: &Res| match res {
        Res::Def(def) => params.iter().position(|p| p == def),
        _ => None,
    };
    for item in uses.iter().flat_map(|u| &u.items) {
        let Res::Def(effect) = item.res else {
            continue;
        };
        let Some(sets) = decls.get(effect) else {
            continue;
        };
        for (j, &a) in item.args.iter().enumerate() {
            if let Some(kind) = sets.get(j).copied().flatten() {
                mark_set_arg(program, &position, kind, a, &mut marks);
            }
        }
    }
    marks
        .in_type
        .iter()
        .zip(&marks.in_set)
        .map(|(&t, &s)| if t { None } else { s })
        .collect()
}

pub(crate) struct Marks {
    in_type: Vec<bool>,
    in_set: Vec<Option<SetKind>>,
}

impl Marks {
    fn set(&mut self, i: usize, kind: SetKind) {
        self.in_set[i].get_or_insert(kind);
    }
}

fn mark_params(
    program: &Program,
    decls: &DeclSets,
    params: &[DefId],
    ty: TypeId,
    marks: &mut Marks,
) {
    let position = |res: &Res| match res {
        Res::Def(def) => params.iter().position(|p| p == def),
        _ => None,
    };
    match &program.types[ty].kind {
        TypeKind::Path { res, args } => {
            if let Some(i) = position(res) {
                marks.in_type[i] = true;
            }
            let sets = match res {
                Res::Def(def) => decls.get(*def),
                _ => None,
            };
            for (j, &a) in args.iter().enumerate() {
                // A type parameter written as a set type argument is a set.
                if let Some(kind) = sets.and_then(|s| s.get(j).copied().flatten())
                    && let TypeKind::Path { res, args } = &program.types[a].kind
                    && args.is_empty()
                    && let Some(i) = position(res)
                {
                    marks.set(i, kind);
                    continue;
                }
                mark_params(program, decls, params, a, marks);
            }
        }
        TypeKind::Tuple(elems) => {
            for &e in elems {
                mark_params(program, decls, params, e, marks);
            }
        }
        TypeKind::Fn {
            params: fn_params,
            ret,
            fails,
            uses,
        } => {
            for &p in fn_params {
                mark_params(program, decls, params, p, marks);
            }
            if let Some(ret) = ret {
                mark_params(program, decls, params, *ret, marks);
            }
            for (res, _) in fails.iter().flat_map(|f| &f.items) {
                if let Some(i) = position(res) {
                    marks.set(i, SetKind::Errors);
                }
            }
            for item in uses.iter().flat_map(|u| &u.items) {
                mark_effect(program, decls, params, item, marks);
            }
        }
        TypeKind::ErrorSet(set) => {
            for (res, _) in &set.items {
                if let Some(i) = position(res) {
                    marks.set(i, SetKind::Errors);
                }
            }
        }
        TypeKind::EffectSet(set) => {
            for item in &set.items {
                mark_effect(program, decls, params, item, marks);
            }
        }
        TypeKind::SelfType | TypeKind::Unit | TypeKind::Missing => {}
    }
}

/// Elements of an effect set. A type parameter itself is a set; an effect's type arguments (the
/// `S` of `State[S]`, spec 8.1) are ordinary types, except at the set type parameters of a
/// built-in effect (the `E` and `R` of `Scope[E, R]`).
pub(crate) fn mark_effect(
    program: &Program,
    decls: &DeclSets,
    params: &[DefId],
    item: &EffectRef,
    marks: &mut Marks,
) {
    let position = |res: &Res| match res {
        Res::Def(def) => params.iter().position(|p| p == def),
        _ => None,
    };
    if let Some(i) = position(&item.res) {
        marks.set(i, SetKind::Effects);
    }
    let sets = match item.res {
        Res::Def(def) => decls.get(def),
        _ => None,
    };
    for (j, &a) in item.args.iter().enumerate() {
        if let Some(kind) = sets.and_then(|s| s.get(j).copied().flatten()) {
            mark_set_arg(program, &position, kind, a, marks);
            continue;
        }
        mark_params(program, decls, params, a, marks);
    }
}

/// Type parameters written in a set type argument of a built-in effect (`Scope[E, R]`,
/// `Scope[E | Timeout, { R, Log }]`) are sets of that kind.
fn mark_set_arg(
    program: &Program,
    position: &dyn Fn(&Res) -> Option<usize>,
    kind: SetKind,
    arg: TypeId,
    marks: &mut Marks,
) {
    match &program.types[arg].kind {
        TypeKind::Path { res, args } if args.is_empty() => {
            if let Some(i) = position(res) {
                marks.set(i, kind);
            }
        }
        TypeKind::ErrorSet(set) => {
            for (res, _) in &set.items {
                if let Some(i) = position(res) {
                    marks.set(i, kind);
                }
            }
        }
        TypeKind::EffectSet(set) => {
            for item in &set.items {
                if let Some(i) = position(&item.res) {
                    marks.set(i, kind);
                }
            }
        }
        _ => {}
    }
}

/// Decides the set kinds of type and enum type parameters. Starts from field types; a parameter
/// passed as a set type argument of a named type (`type Server[E](h: Handler[E, {}])`) is a set
/// too, so this repeats until nothing changes.
fn decl_sets(program: &Program) -> DeclSets {
    let decls: Vec<(DefId, Vec<DefId>, Vec<TypeId>)> = program
        .defs
        .iter()
        .filter(|(_, d)| matches!(d.kind, DefKind::Type | DefKind::Enum))
        .map(|(def, _)| {
            (
                def,
                type_param_defs(program, def),
                field_types(program, def),
            )
        })
        .collect();
    let mut sets = DeclSets::default();
    // Built-in effects decide the kinds of their type parameters from the operation signatures,
    // like the field types of a type (`Scope[E, R]`, spec 11.2). Other effects take ordinary
    // types only (E0424, spec 8.1).
    for (def, _) in program.defs.iter() {
        if !is_intrinsic_effect(program, def) {
            continue;
        }
        let params = type_param_defs(program, def);
        let types = effect_op_types(program, def);
        let kinds = classify_sets(program, &sets, &params, types);
        sets.insert(def, kinds);
    }
    loop {
        let mut changed = false;
        for (def, params, types) in &decls {
            let kinds = classify_sets(program, &sets, params, types.iter().copied());
            if sets.get(*def) != Some(&kinds) {
                sets.insert(*def, kinds);
                changed = true;
            }
        }
        if !changed {
            return sets;
        }
    }
}

/// The parameter and return type expressions of an effect's operations.
pub(crate) fn effect_op_types(program: &Program, effect: DefId) -> Vec<TypeId> {
    let ops: &[DefId] = match program.item(effect) {
        Some(Item::Effect(e)) => &e.ops,
        _ => &[],
    };
    ops.iter()
        .flat_map(|&op| match program.item(op) {
            Some(Item::Op(o)) => o
                .sig
                .params
                .iter()
                .filter_map(|p| p.ty)
                .chain(o.sig.ret)
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect()
}

/// The field type expressions of a type or enum's constructors.
fn field_types(program: &Program, def: DefId) -> Vec<TypeId> {
    let fields = |f: &Fields| -> Vec<TypeId> {
        match f {
            Fields::None => Vec::new(),
            Fields::Positional(tys) => tys.clone(),
            Fields::Named(fs) => fs.iter().map(|f| f.ty).collect(),
        }
    };
    let ctor_fields = |ctor: DefId| match program.item(ctor) {
        Some(Item::Ctor(c)) => fields(&c.fields),
        _ => Vec::new(),
    };
    match program.item(def) {
        Some(Item::Type(t)) => t.ctor.map(ctor_fields).unwrap_or_default(),
        Some(Item::Enum(e)) => e.variants.iter().flat_map(|&v| ctor_fields(v)).collect(),
        _ => Vec::new(),
    }
}

/// Whether the type expression contains a function type with `fails` omitted. If the return type
/// does, its E is inferred from the body (spec 9.2).
pub(crate) fn has_omitted_fails(program: &Program, ty: TypeId) -> bool {
    match &program.types[ty].kind {
        TypeKind::Path { args, .. } => args.iter().any(|&a| has_omitted_fails(program, a)),
        TypeKind::Tuple(elems) => elems.iter().any(|&e| has_omitted_fails(program, e)),
        TypeKind::Fn { fails: None, .. } => true,
        TypeKind::Fn { params, ret, .. } => {
            params.iter().any(|&p| has_omitted_fails(program, p))
                || ret.is_some_and(|r| has_omitted_fails(program, r))
        }
        TypeKind::SelfType
        | TypeKind::Unit
        | TypeKind::Missing
        | TypeKind::ErrorSet(_)
        | TypeKind::EffectSet(_) => false,
    }
}

/// Whether the type expression contains a function type with `use` omitted. If the return type
/// does, its R is inferred from the body (spec 9.2).
pub(crate) fn has_omitted_uses(program: &Program, ty: TypeId) -> bool {
    match &program.types[ty].kind {
        TypeKind::Path { args, .. } => args.iter().any(|&a| has_omitted_uses(program, a)),
        TypeKind::Tuple(elems) => elems.iter().any(|&e| has_omitted_uses(program, e)),
        TypeKind::Fn { uses: None, .. } => true,
        TypeKind::Fn { params, ret, .. } => {
            params.iter().any(|&p| has_omitted_uses(program, p))
                || ret.is_some_and(|r| has_omitted_uses(program, r))
        }
        TypeKind::SelfType
        | TypeKind::Unit
        | TypeKind::Missing
        | TypeKind::ErrorSet(_)
        | TypeKind::EffectSet(_) => false,
    }
}
