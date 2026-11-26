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

/// What omitted `fails` and `uses` clauses on a function type mean, decided by where they are
/// written (spec 9.2).
pub(crate) enum Omitted<'a> {
    /// Inside a declaration (fields, consts, ...): empty.
    Empty,
    /// The field types of a type, enum or error declaration: omitted clauses are empty, and each
    /// capability set is a hidden set type parameter of the declaration ([`FieldCaps`]).
    Fields(&'a mut FieldCaps),
    /// Inside a top-level fn's parameter types: adds an invisible set type parameter.
    Hidden(&'a mut Hidden),
    /// Return type and binding annotations, lambda parameters: inferred from the body, so a fresh
    /// set variable. Lambdas are not generalized, so their parameters get set variables rather
    /// than invisible set type parameters.
    Fresh(&'a mut InferCtx),
    /// A local fn's parameter types (`[fn.local]`): a fresh set variable, as for `Fresh`, since a
    /// local fn is not generalized. The ones in positive positions (functions the body can call)
    /// are also added to `positive` (error sets) and `positive_uses` (effect sets), to be
    /// included in the declared E and R as [`Hidden`] does for a top-level fn.
    Local {
        ctx: &'a mut InferCtx,
        positive: &'a mut Row,
        positive_uses: &'a mut Row,
    },
}

/// Invisible set type parameters inside parameter types (spec 9.2), placed after the written
/// type parameters.
pub(crate) struct Hidden {
    /// The number of written type parameters; invisible ones are indexed after these.
    base: usize,
    pub(crate) params: Vec<TyParam>,
    /// The current parameter's name, used to name invisible set type parameters (`E(f)`, `R(f)`).
    param: SmolStr,
    /// How many error sets, effect sets and capability sets were added for the current
    /// parameter.
    in_param: [usize; 3],
    /// Invisible error sets in positive positions of parameter types (functions the body can
    /// call). Included in the declared E.
    pub(crate) positive: Row,
    /// Likewise for invisible effect sets; included in the declared R.
    pub(crate) positive_uses: Row,
    /// Whether invisible effect sets get an `Immediate` bound. Callbacks of external functions
    /// must be Immediate (spec 9.5).
    pub(crate) immediate: bool,
    /// The capability sets of the capability types in the parameter types, with the effects of
    /// each (`[type.rules]`). The result type of the function takes its sets from them.
    pub(crate) caps: Vec<(Vec<DefId>, Row)>,
}

impl Hidden {
    pub(crate) fn new(base: usize) -> Self {
        Hidden {
            base,
            params: Vec::new(),
            param: SmolStr::default(),
            in_param: [0, 0, 0],
            positive: Row::empty(),
            positive_uses: Row::empty(),
            immediate: false,
            caps: Vec::new(),
        }
    }

    pub(crate) fn start_param(&mut self, name: SmolStr) {
        self.param = name;
        self.in_param = [0, 0, 0];
    }

    /// Adds the invisible set type parameter of a capability type `use E1 + ... + En` written in
    /// a parameter type (`C(db)`, `[type.rules]`). The function's R includes it, as it does the
    /// sets of a written `uses` clause (`[type.inference]`).
    fn fresh_caps(&mut self, effects: &[Tag]) -> Row {
        self.in_param[2] += 1;
        let name = match self.in_param[2] {
            1 => format!("C({})", self.param),
            n => format!("C({}, {n})", self.param),
        };
        let i = (self.base + self.params.len()) as u32;
        let mut param = TyParam::set(name);
        param.caps = Some(effects.to_vec());
        self.params.push(param);
        let row = Row::from_elems([RowElem::Bound(i)]);
        self.positive_uses.extend(&row);
        self.caps
            .push((effects.iter().map(|t| t.def).collect(), row.clone()));
        row
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

/// The hidden capability set type parameters of a type, enum or error declaration
/// (`[type.rules]`), placed after its written type parameters: first the set of a
/// capability-bound type (`uses E`), then one for each place a capability type or a type with
/// hidden sets of its own is written in the fields, in the order of the fields.
#[derive(Clone, Debug, Default)]
pub struct DeclCaps {
    /// The number of written type parameters.
    pub visible: usize,
    /// The effect of a capability-bound type: its `uses E`.
    pub(crate) bound: Option<DefId>,
    /// The effects of each hidden set.
    pub(crate) effects: Vec<Vec<Tag>>,
}

/// Gives the capability sets in the field types of `owner` its hidden set type parameters
/// ([`DeclCaps`]).
pub(crate) struct FieldCaps {
    /// The declaration whose fields are read; `None` for parameters.
    owner: Option<DefId>,
    /// The index of the next hidden set type parameter.
    next: u32,
    /// While the sets are being found: the effects and the name of each one found. Otherwise the
    /// sets are known, and `end` is past the last one.
    found: Option<Vec<(Vec<Tag>, SmolStr)>>,
    end: u32,
    /// The name of the field being read, for naming its sets (`C(read)`), and how many it has.
    field: SmolStr,
    in_field: usize,
}

impl FieldCaps {
    /// Finds the capability sets of parameters (of an init or an operation), numbered from
    /// `next`; [`Self::found`] gives them.
    pub(crate) fn finding(owner: Option<DefId>, next: u32) -> Self {
        FieldCaps {
            owner,
            next,
            found: Some(Vec::new()),
            end: 0,
            field: SmolStr::default(),
            in_field: 0,
        }
    }

    /// The effects and names of the sets found.
    pub(crate) fn found(self) -> Vec<(Vec<Tag>, SmolStr)> {
        self.found.unwrap_or_default()
    }

    /// For the constructor of `owner`, whose hidden sets are known.
    pub(crate) fn new(table: &TyTable, owner: DefId) -> Self {
        let (next, end) = match table.decl_caps(owner) {
            Some(caps) => (
                (caps.visible + usize::from(caps.bound.is_some())) as u32,
                (caps.visible + caps.effects.len()) as u32,
            ),
            None => (0, 0),
        };
        FieldCaps {
            owner: Some(owner),
            next,
            found: None,
            end,
            field: SmolStr::default(),
            in_field: 0,
        }
    }

    pub(crate) fn start_field(&mut self, name: SmolStr) {
        self.field = name;
        self.in_field = 0;
    }

    fn next_caps(&mut self, effects: &[Tag]) -> Row {
        if let Some(found) = &mut self.found {
            self.in_field += 1;
            let name = match self.in_field {
                1 => format!("C({})", self.field),
                n => format!("C({}, {n})", self.field),
            };
            found.push((effects.to_vec(), name.into()));
        } else if self.next >= self.end {
            return Row::empty();
        }
        let row = Row::from_elems([RowElem::Bound(self.next)]);
        self.next += 1;
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
    /// In the result type of a function: the capability sets of its parameter types
    /// ([`Hidden::caps`]). A capability type there records the union of the sets of the same
    /// effects, or the name of the effect when there is none (`[type.rules]`).
    pub(crate) result_caps: Option<&'a [(Vec<DefId>, Row)]>,
    /// Inside the parameter types of a function type: how many capability sets that function
    /// type has quantified so far (`[type.rules]`). A capability type there is quantified at it.
    pub(crate) quant: Option<u32>,
}

impl Scope<'_> {
    /// The capability set of a capability type of `effects` written here (`[type.rules]`).
    fn caps_row(&mut self, effects: &[Tag]) -> Row {
        if let Some(n) = &mut self.quant {
            let i = *n;
            *n += 1;
            return Row::from_elems([quant_elem(i, effects)]);
        }
        if let Some(params) = self.result_caps {
            let mut row = Row::empty();
            for (defs, set) in params {
                if effects.iter().any(|t| defs.contains(&t.def)) {
                    row.extend(set);
                }
            }
            if row.is_empty() {
                row = Row::from_elems(effects.iter().cloned().map(RowElem::Tag));
            }
            return row;
        }
        match &mut self.omitted {
            Omitted::Empty => Row::empty(),
            Omitted::Fields(fields) => fields.next_caps(effects),
            Omitted::Hidden(hidden) => hidden.fresh_caps(effects),
            Omitted::Fresh(ctx) => Row::var(ctx.new_row_var()),
            Omitted::Local {
                ctx, positive_uses, ..
            } => {
                let row = Row::var(ctx.new_row_var());
                positive_uses.extend(&row);
                row
            }
        }
    }

    /// The set for an omitted `fails` clause (`uses` clause if `effects`).
    fn omitted_row(&mut self, positive: bool, effects: bool) -> Row {
        match &mut self.omitted {
            Omitted::Empty | Omitted::Fields(_) => Row::empty(),
            Omitted::Hidden(hidden) => hidden.fresh(positive, effects),
            Omitted::Fresh(ctx) => Row::var(ctx.new_row_var()),
            Omitted::Local {
                ctx,
                positive: fails,
                positive_uses: uses,
            } => {
                let row = Row::var(ctx.new_row_var());
                if positive {
                    if effects {
                        uses.extend(&row);
                    } else {
                        fails.extend(&row);
                    }
                }
                row
            }
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
    /// Inner handlers (`[effect.handler]`), whose type parameters are their outer handler's.
    inner_handlers: rustc_hash::FxHashSet<DefId>,
    /// The hidden capability sets of declarations that have them (`[type.rules]`).
    caps: ArenaMap<DefId, DeclCaps>,
    /// The order the hidden sets of declarations were found in: one whose fields refer to
    /// another comes after it, unless they refer to each other.
    caps_order: ArenaMap<DefId, u32>,
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
                // The External of a module is shown with its module (`Log.External`,
                // `[effect.external]`).
                DefKind::Effect => {
                    let name = match program.external_module(def) {
                        Some(m) => match program.modules.get(m) {
                            Some(items) => format!("{}.{}", items.name, data.name).into(),
                            None => data.name.clone(),
                        },
                        None => data.name.clone(),
                    };
                    cons.set_tag_name(def, name);
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

        let inner_handlers = program
            .defs
            .iter()
            .filter(|(_, d)| {
                d.kind == DefKind::Handler
                    && d.parent
                        .is_some_and(|p| program.defs[p].kind == DefKind::Handler)
            })
            .map(|(def, _)| def)
            .collect();
        let mut table = TyTable {
            cons,
            by_def,
            bool_def,
            def_of,
            sets,
            inner_handlers,
            caps: ArenaMap::default(),
            caps_order: ArenaMap::default(),
        };
        table.find_decl_caps(program);
        table
    }

    /// The hidden capability sets of a declaration, if it has any.
    pub fn decl_caps(&self, def: DefId) -> Option<&DeclCaps> {
        self.caps.get(def)
    }

    /// The effect of a capability-bound type (`[type.rules]`).
    pub fn bound_effect(&self, def: DefId) -> Option<DefId> {
        self.caps.get(def).and_then(|c| c.bound)
    }

    /// Finds the hidden capability sets of every type, enum and error declaration
    /// ([`DeclCaps`]). A declaration whose fields have a type with hidden sets of its own gets a
    /// set for each of them, so the declarations are read in the order of their references. A
    /// declaration that refers to itself passes its own sets; one that refers to a declaration
    /// that refers back to it gives that one empty sets.
    fn find_decl_caps(&mut self, program: &Program) {
        let decls: Vec<DefId> = program
            .defs
            .iter()
            .filter(|(def, d)| {
                matches!(
                    d.kind,
                    DefKind::Type | DefKind::Enum | DefKind::Error | DefKind::Handler
                ) && self.by_def.get(*def).is_some()
            })
            .map(|(def, _)| def)
            .collect();
        let mut order = Vec::new();
        let mut state: FxHashMap<DefId, bool> = FxHashMap::default();
        for &def in &decls {
            visit_decl(program, def, &mut state, &mut order);
        }
        for (i, &def) in order.iter().enumerate() {
            self.caps_order.insert(def, i as u32);
        }
        for def in order {
            let Some(&id) = self.by_def.get(def) else {
                continue;
            };
            let visible = self.cons[id].params.len();
            let bound = match program.item(def) {
                Some(Item::Type(t)) => t.bound.as_ref(),
                Some(Item::Enum(e)) => e.bound.as_ref(),
                _ => None,
            }
            .and_then(|b| match b.res {
                Res::Def(e) if program.defs[e].kind == DefKind::Effect => Some(e),
                _ => None,
            });
            let mut fields = FieldCaps {
                owner: Some(def),
                next: (visible + usize::from(bound.is_some())) as u32,
                found: Some(Vec::new()),
                end: 0,
                field: SmolStr::default(),
                in_field: 0,
            };
            self.lower_fields(program, def, def, &mut fields, &mut Vec::new());
            let found = fields.found.unwrap_or_default();
            if bound.is_none() && found.is_empty() {
                continue;
            }
            let mut effects = Vec::new();
            let mut params = Vec::new();
            if let Some(e) = bound {
                effects.push(vec![Tag::new(e)]);
                let mut p = TyParam::set("C");
                p.caps = Some(vec![Tag::new(e)]);
                params.push(p);
            }
            for (tags, name) in found {
                let mut p = TyParam::set(name);
                p.caps = Some(tags.clone());
                params.push(p);
                effects.push(tags);
            }
            let mut kinds = self.sets.get(def).cloned().unwrap_or_default();
            kinds.resize(visible, None);
            kinds.extend(params.iter().map(|_| Some(SetKind::Caps)));
            self.sets.insert(def, kinds);
            self.cons.params_mut(id).extend(params);
            self.caps.insert(
                def,
                DeclCaps {
                    visible,
                    bound,
                    effects,
                },
            );
        }
    }

    /// Reads the field types of `owner` with `fields` giving its hidden sets, and returns those
    /// of the constructor `ctor`. The fields of the other constructors are read too, for the
    /// numbering of the sets, without their diagnostics.
    pub(crate) fn lower_fields(
        &self,
        program: &Program,
        owner: DefId,
        ctor: DefId,
        fields: &mut FieldCaps,
        diags: &mut Vec<Diagnostic>,
    ) -> Vec<Ty> {
        let defs = type_param_defs(program, owner);
        let kinds: Vec<ParamKind> = match self.by_def.get(owner) {
            Some(&id) => self.cons[id].params.iter().map(|p| p.kind).collect(),
            None => Vec::new(),
        };
        let mut out = Vec::new();
        let mut others = Vec::new();
        for (of, name, ty) in decl_fields(program, owner) {
            fields.start_field(name);
            let mut scope = Scope {
                params: &defs,
                kinds: &kinds,
                omitted: Omitted::Fields(fields),
                sets: true,
                self_ty: None,
                result_caps: None,
                quant: None,
            };
            if of == ctor {
                out.push(self.lower_in(program, &mut scope, ty, true, diags));
            } else {
                self.lower_in(program, &mut scope, ty, true, &mut others);
            }
        }
        out
    }

    pub(crate) fn decl_sets(&self) -> &DeclSets {
        &self.sets
    }

    /// The set kind of the `i`-th type parameter of a type, enum or built-in effect, if it is a
    /// set.
    pub(crate) fn set_kind(&self, def: DefId, i: usize) -> Option<SetKind> {
        self.sets.get(def).and_then(|s| s.get(i).copied().flatten())
    }

    /// The kinds of the `n` type parameters of `def`: a set where [`Self::set_kind`] says so
    /// (the `E` and `R` of the built-in handler `Group[E, R]`, `[task.group]`).
    pub(crate) fn param_kinds(&self, def: DefId, n: usize) -> Vec<ParamKind> {
        (0..n)
            .map(|i| match self.set_kind(def, i) {
                Some(_) => ParamKind::Set,
                None => ParamKind::Type,
            })
            .collect()
    }

    /// `None` for Bool and for definitions other than type, enum, error and handler.
    pub fn con(&self, def: DefId) -> Option<TyConId> {
        self.by_def.get(def).copied()
    }

    pub fn def(&self, id: TyConId) -> DefId {
        self.def_of[id]
    }

    /// The type parameters of a handler that an item of a `with`, of a layer or the right of
    /// `by` gives type arguments to (`[effect.with]`), with the traits of their bounds. Empty for
    /// an inner handler: its declaration is read with the type parameters of its outer handler,
    /// whose instance opens it.
    pub fn handler_params(&self, handler: DefId) -> &[TyParam] {
        if self.inner_handlers.contains(&handler) {
            return &[];
        }
        let Some(id) = self.con(handler) else {
            return &[];
        };
        // The hidden capability sets of its fields come after them (`[type.rules]`): each item
        // decides them as it decides those of the parameters of init.
        let params = self.cons[id].params.as_slice();
        let visible = self.caps.get(handler).map_or(params.len(), |c| c.visible);
        &params[..visible]
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
            result_caps: None,
            quant: None,
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
                let outer = scope.quant.take();
                let ret_ty = match ret {
                    Some(ret) => self.lower_in(program, scope, *ret, positive, diags),
                    None => Ty::Error,
                };
                // A capability type in the parameter types is quantified at this function type;
                // its R has the quantified sets (`[type.rules]`).
                scope.quant = Some(0);
                let mut quantified = Vec::new();
                let param_tys: Vec<Ty> = fn_params
                    .iter()
                    .map(|&p| {
                        let ty = self.lower_in(program, scope, p, !positive, diags);
                        collect_quant(&ty, &mut quantified);
                        ty
                    })
                    .collect();
                scope.quant = outer;
                let mut uses = uses;
                for q in quantified {
                    uses.insert(q);
                }
                Ty::func_sets(param_tys, ret_ty, fails, uses)
            }
            TypeKind::Path { res, args } => {
                self.lower_path(program, scope, type_ref, *res, args, positive, diags)
            }
            // `use E1 + ... + En` (`[effect.use]`): the type of the capability values of those
            // effects, the same type as that of the expression `use E` for one. Name resolution
            // has already reported a name that is not an effect.
            TypeKind::Cap(effects) => {
                let mut tags = Vec::with_capacity(effects.len());
                for effect in effects {
                    let Res::Def(def) = effect.res else {
                        return Ty::Error;
                    };
                    if program.defs[def].kind != DefKind::Effect {
                        return Ty::Error;
                    }
                    match self.effect_tag(
                        program,
                        scope,
                        def,
                        &effect.args,
                        Some(effect.span),
                        None,
                        diags,
                    ) {
                        Some(tag) => tags.push(tag),
                        None => return Ty::Error,
                    }
                }
                let caps = scope.caps_row(&tags);
                Ty::cap(tags, caps)
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

    /// The set of a `uses` clause. Effects become tags, set type parameters `RowElem::Bound`.
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
    /// ordinary types; omitted `fails` and `uses` on function types inside them are empty (as in
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
            result_caps: scope.result_caps,
            quant: None,
        };
        // Set type parameters of built-in effects (the `E` and `R` of `TaskGroup[E, R]`) take sets,
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
            DefKind::Type | DefKind::Enum | DefKind::Error => match self.caps.get(def) {
                Some(caps) => caps.visible,
                None => match self.by_def.get(def) {
                    Some(&id) => self.cons[id].params.len(),
                    None => 0,
                },
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
                let mut args: Vec<Ty> = args
                    .iter()
                    .enumerate()
                    .map(|(i, &a)| match self.set_kind(def, i) {
                        Some(kind) => self.lower_set_arg(program, scope, def, i, kind, a, diags),
                        None => self.lower_in(program, scope, a, positive, diags),
                    })
                    .collect();
                self.hidden_args(scope, def, &mut args);
                Ty::named(id, args)
            }
            _ => Ty::Error,
        }
    }
}

impl TyTable {
    /// Adds the hidden capability sets of `def` to the written type arguments `args`, each as a
    /// capability type of its effects written at the same place would have (`[type.rules]`).
    /// In the fields of `def` itself they are its own sets; in the fields of a declaration whose
    /// sets were found before those of `def`, they are empty ([`Self::find_decl_caps`]).
    fn hidden_args(&self, scope: &mut Scope<'_>, def: DefId, args: &mut Vec<Ty>) {
        let Some(caps) = self.caps.get(def) else {
            return;
        };
        for (j, effects) in caps.effects.iter().enumerate() {
            let row = match &scope.omitted {
                Omitted::Fields(f) if f.owner == Some(def) => {
                    Row::from_elems([RowElem::Bound((caps.visible + j) as u32)])
                }
                Omitted::Fields(FieldCaps {
                    owner: Some(owner), ..
                }) if self.caps_order.get(def) >= self.caps_order.get(*owner) => Row::empty(),
                _ => scope.caps_row(effects),
            };
            args.push(Ty::Set(SetKind::Caps, row));
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
        // The type parameters of a handler are ordinary types (E0424), with the traits of their
        // bounds, which are checked at each item (`[effect.with]`).
        Some(Item::Handler(_)) => {
            return handler_type_params(program, def)
                .iter()
                .enumerate()
                .map(|(i, tp)| {
                    let name = program.defs[tp.def].name.clone();
                    // The set type parameters of a built-in handler (`Group[E, R]`).
                    if sets.get(i).copied().flatten().is_some() {
                        return TyParam::set(name);
                    }
                    let mut p = TyParam::new(name);
                    crate::signature::with_traits(program, &mut p, tp);
                    p
                })
                .collect();
        }
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
        Some(Item::Handler(_)) => handler_type_params(program, def),
        _ => &[],
    };

    type_params.iter().map(|tp| tp.def).collect()
}

/// The type parameters a handler's declaration is read with (`[effect.handler]`): its own, or for
/// an inner handler, those of its outer handler. The type parameters written on an inner handler
/// are E0460 and are not among them.
pub(crate) fn handler_type_params(program: &Program, handler: DefId) -> &[TypeParam] {
    let mut def = handler;
    while let Some(outer) = program.defs[def].parent
        && program.defs[outer].kind == DefKind::Handler
    {
        def = outer;
    }
    match program.item(def) {
        Some(Item::Handler(h)) => &h.type_params,
        _ => &[],
    }
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
    prelude_bounds(program, tp, IMMEDIATE)
}

/// The spans of a type parameter's bounds that refer to the Prelude's definition `name`.
pub(crate) fn prelude_bounds(program: &Program, tp: &TypeParam, name: &str) -> Vec<Span> {
    let def = program.prelude_def(name);
    tp.bounds
        .iter()
        .filter(|(res, _)| matches!(res, Res::Def(d) if Some(*d) == def))
        .map(|&(_, span)| span)
        .collect()
}

/// Whether an effect is a built-in effect of the bundled sources (`@intrinsic effect TaskGroup`,
/// spec 11.2). Only built-in effects may have set type parameters and operation type parameters,
/// and no handler may implement them (spec 8.1).
pub(crate) fn is_intrinsic_effect(program: &Program, effect: DefId) -> bool {
    matches!(program.item(effect), Some(Item::Effect(e)) if e.is_intrinsic)
}

/// Whether an effect is Immediate (spec 9.5): it has no suspend fn, no suspend handler member, and
/// is not Async. An effect whose declaration is unknown (name resolution has reported it) counts as
/// Immediate, so no further error follows from it. Lowering decides which code may suspend by the
/// same function.
pub fn is_immediate_effect(program: &Program, effect: DefId) -> bool {
    if program.prelude_def("Async") == Some(effect) {
        return false;
    }
    // A label is Immediate when its base is (issue #255). A suspend handler member may suspend
    // when it is opened and released (`[type.effect-bound]`).
    program
        .effect_ops(effect)
        .iter()
        .all(|&op| !matches!(program.item(op), Some(Item::Op(o)) if o.sig.is_suspend))
        && program
            .effect_members(effect)
            .iter()
            .all(|&m| !matches!(program.item(m), Some(Item::Member(m)) if m.sig.is_suspend))
}

/// The set kind of each type parameter of a named type declaration (`None` if not a set). Keyed
/// by type and enum definitions.
pub(crate) type DeclSets = ArenaMap<DefId, Vec<Option<SetKind>>>;

/// Decides type parameter kinds from where they are used (spec 9.2). A type parameter written only
/// in `fails` or `uses` clauses is a set; otherwise an ordinary type (written both as a value type
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
/// fn's own `uses` clause (the `E` and `R` of
/// `fn crawl[E, R](tasks: use TaskGroup[E, R]) -> ()`, `[task.group]`).
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

/// For each of `params`, whether it is written in `types` as an ordinary type, and the kind of
/// set it is written as, if any ([`classify_sets`] keeps only the second when the first is
/// false).
pub(crate) fn mark_all(
    program: &Program,
    decls: &DeclSets,
    params: &[DefId],
    types: impl IntoIterator<Item = TypeId>,
) -> Vec<(bool, Option<SetKind>)> {
    let mut marks = Marks {
        in_type: vec![false; params.len()],
        in_set: vec![None; params.len()],
    };
    for ty in types {
        mark_params(program, decls, params, ty, &mut marks);
    }
    marks.in_type.into_iter().zip(marks.in_set).collect()
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
        TypeKind::Cap(effects) => {
            for effect in effects {
                mark_effect(program, decls, params, effect, marks);
            }
        }
        TypeKind::SelfType | TypeKind::Unit | TypeKind::Missing => {}
    }
}

/// Elements of an effect set. A type parameter itself is a set; an effect's type arguments (the
/// `S` of `State[S]`, spec 8.1) are ordinary types, except at the set type parameters of a
/// built-in effect (the `E` and `R` of `TaskGroup[E, R]`).
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

/// Type parameters written in a set type argument of a built-in effect (`TaskGroup[E, R]`,
/// `TaskGroup[E | Timeout, { R, Log }]`) are sets of that kind.
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
    // like the field types of a type (`TaskGroup[E, R]`, spec 11.2). Other effects take ordinary
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
            break;
        }
    }
    // Built-in handlers decide them from the effects they implement and the parameters of their
    // init (`Group[E, R]: TaskGroup[E, R]`, `[task.group]`). They are the only handlers with set
    // type parameters.
    for (def, _) in program.defs.iter() {
        let Some(Item::Handler(h)) = program.item(def) else {
            continue;
        };
        if !h.is_intrinsic || h.type_params.is_empty() {
            continue;
        }
        let Some(span) = h.effects.first().map(|e| e.span) else {
            continue;
        };
        let params: Vec<DefId> = h.type_params.iter().map(|tp| tp.def).collect();
        let effects = EffectSet {
            items: h.effects.clone(),
            span,
        };
        let types = h
            .fields
            .iter()
            .map(|f| f.ty)
            .chain(h.init_params.iter().filter_map(|p| p.ty));
        let kinds = classify_fn_sets(program, &sets, &params, types, Some(&effects));
        if kinds.iter().any(Option::is_some) {
            sets.insert(def, kinds);
        }
    }
    sets
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

/// Visits the declarations that the fields of `def` refer to before `def`, then adds `def` to
/// `order`. `state` is false while a declaration is being visited, true once added.
fn visit_decl(
    program: &Program,
    def: DefId,
    state: &mut FxHashMap<DefId, bool>,
    order: &mut Vec<DefId>,
) {
    if state.contains_key(&def) {
        return;
    }
    state.insert(def, false);
    let mut refs = Vec::new();
    for (_, _, ty) in decl_fields(program, def) {
        type_refs(program, ty, &mut refs);
    }
    for r in refs {
        if matches!(
            program.defs[r].kind,
            DefKind::Type | DefKind::Enum | DefKind::Error
        ) {
            visit_decl(program, r, state, order);
        }
    }
    state.insert(def, true);
    order.push(def);
}

/// The declarations named in the type expression `ty`.
fn type_refs(program: &Program, ty: TypeId, out: &mut Vec<DefId>) {
    match &program.types[ty].kind {
        TypeKind::Path { res, args } => {
            if let Res::Def(d) = res {
                out.push(*d);
            }
            for &a in args {
                type_refs(program, a, out);
            }
        }
        TypeKind::Tuple(elems) => {
            for &e in elems {
                type_refs(program, e, out);
            }
        }
        TypeKind::Fn { params, ret, .. } => {
            for &p in params.iter().chain(ret) {
                type_refs(program, p, out);
            }
        }
        _ => {}
    }
}

/// The fields of the constructors of a type, enum or error, with their constructor (the error
/// itself for an error) and their names (the index for an unnamed one).
fn decl_fields(program: &Program, def: DefId) -> Vec<(DefId, SmolStr, TypeId)> {
    let fields = |ctor: DefId, f: &Fields| -> Vec<(DefId, SmolStr, TypeId)> {
        match f {
            Fields::None => Vec::new(),
            Fields::Positional(tys) => tys
                .iter()
                .enumerate()
                .map(|(i, &t)| (ctor, SmolStr::new(i.to_string()), t))
                .collect(),
            Fields::Named(fs) => fs.iter().map(|f| (ctor, f.name.clone(), f.ty)).collect(),
        }
    };
    let ctor_fields = |ctor: DefId| match program.item(ctor) {
        Some(Item::Ctor(c)) => fields(ctor, &c.fields),
        _ => Vec::new(),
    };
    match program.item(def) {
        Some(Item::Type(t)) => t.ctor.map(ctor_fields).unwrap_or_default(),
        Some(Item::Enum(e)) => e.variants.iter().flat_map(|&v| ctor_fields(v)).collect(),
        Some(Item::Error(e)) => fields(def, &e.fields),
        Some(Item::Handler(h)) => h.ctor.map(ctor_fields).unwrap_or_default(),
        _ => Vec::new(),
    }
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
        TypeKind::Cap(effects) => effects
            .iter()
            .flat_map(|e| &e.args)
            .any(|&a| has_omitted_fails(program, a)),
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

/// Whether the type expression contains a function type with `uses` omitted. If the return type
/// does, its R is inferred from the body (spec 9.2).
pub(crate) fn has_omitted_uses(program: &Program, ty: TypeId) -> bool {
    match &program.types[ty].kind {
        TypeKind::Path { args, .. } => args.iter().any(|&a| has_omitted_uses(program, a)),
        TypeKind::Tuple(elems) => elems.iter().any(|&e| has_omitted_uses(program, e)),
        TypeKind::Cap(effects) => effects
            .iter()
            .flat_map(|e| &e.args)
            .any(|&a| has_omitted_uses(program, a)),
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

/// The element of the `i`-th capability set quantified at a function type, for a capability
/// type of `effects` (`[type.rules]`).
pub(crate) fn quant_elem(i: u32, effects: &[Tag]) -> RowElem {
    RowElem::Cap(crate::row::CapElem {
        id: crate::row::CapId::Quant(i),
        name: SmolStr::new("any"),
        tag: effects[0].clone(),
    })
}

/// The quantified sets in `ty`, outside the function types inside it, which bind their own.
fn collect_quant(ty: &Ty, out: &mut Vec<RowElem>) {
    match ty {
        Ty::Fn(_) => {}
        Ty::Cap(_, caps) | Ty::Set(SetKind::Caps, caps) => {
            for e in caps.elems() {
                if matches!(e, RowElem::Cap(c) if matches!(c.id, crate::row::CapId::Quant(_)))
                    && !out.contains(e)
                {
                    out.push(e.clone());
                }
            }
            ty.for_each_child(|child| collect_quant(child, out));
        }
        _ => ty.for_each_child(|child| collect_quant(child, out)),
    }
}
