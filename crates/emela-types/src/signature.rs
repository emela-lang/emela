//! Declared types: the types ([`Scheme`]) of fns, constructors and consts, and parameter labels.
//!
//! Call inference looks up the callee's type and labels here. Fns without a written return type,
//! fns with a body but no `use` clause, and consts without a written type are added after their
//! bodies are inferred. Effect operation types also live here, for calls through capability
//! values.

use emela_resolve::hir::{
    DefId, DefKind, EffectItem, Fields, FnItem, FnSig, Item, LabelOf, LocalKind, Param, Program,
    Res, TypeId, TypeKind, TypeParam,
};
use la_arena::ArenaMap;
use smol_str::SmolStr;

use crate::diagnostic::{Diagnostic, DiagnosticKind, ImmediateSource, LabelBaseReason};
use crate::lower::{
    Hidden, IMMEDIATE, Omitted, Scope, TyTable, classify_fn_sets, classify_params, classify_sets,
    effect_op_types, effect_params, has_omitted_fails, has_omitted_uses, immediate_bounds,
    is_immediate_effect, is_intrinsic_effect, ty_params, type_param_defs,
};
use crate::row::{Row, RowElem, SetKind, Tag};
use crate::scheme::Scheme;
use crate::ty::{ParamKind, Ty, TyParam};

pub struct Signature {
    pub scheme: Scheme,
    /// One label per parameter; `None` for positional-only fields. Empty for non-functions.
    pub labels: Vec<Option<SmolStr>>,
}

/// Also holds the effect tags that handlers implement.
pub struct Signatures {
    by_def: ArenaMap<DefId, Signature>,
    /// Handler → the effect it implements and its type arguments (`implements State[Int]`, spec
    /// 8.1). Async handlers (E0413) and handlers whose effect is unknown are absent.
    handler_effects: ArenaMap<DefId, Tag>,
    /// Label → the effect it stands for (`effect AuditLog of Console`, issue #255). Labels whose
    /// base cannot be used (E0429) are absent.
    label_bases: ArenaMap<DefId, Tag>,
    /// Effect → its labels declared `default`, in definition order. A `with` providing the effect
    /// also provides them (issue #255).
    default_labels: ArenaMap<DefId, Vec<DefId>>,
}

impl Signatures {
    pub fn new() -> Self {
        Signatures {
            by_def: ArenaMap::default(),
            handler_effects: ArenaMap::default(),
            label_bases: ArenaMap::default(),
            default_labels: ArenaMap::default(),
        }
    }

    pub fn handler_effect(&self, handler: DefId) -> Option<&Tag> {
        self.handler_effects.get(handler)
    }

    /// The effect a label stands for. `None` for effects that are not labels.
    pub fn label_base(&self, label: DefId) -> Option<&Tag> {
        self.label_bases.get(label)
    }

    /// The effect whose operations a capability of `tag` has: the base for a label, else `tag`.
    pub fn base_of<'a>(&'a self, tag: &'a Tag) -> &'a Tag {
        self.label_base(tag.def).unwrap_or(tag)
    }

    /// The labels of `effect` (with its type arguments) declared `default`.
    pub fn default_labels<'a>(&'a self, effect: &'a Tag) -> impl Iterator<Item = DefId> + 'a {
        self.default_labels
            .get(effect.def)
            .into_iter()
            .flatten()
            .copied()
            .filter(move |&l| self.label_base(l) == Some(effect))
    }

    pub fn insert(&mut self, def: DefId, signature: Signature) {
        self.by_def.insert(def, signature);
    }

    pub fn get(&self, def: DefId) -> Option<&Signature> {
        self.by_def.get(def)
    }

    /// Collects the declared types of fns, constructors, consts and effect operations.
    pub fn collect(program: &Program, ty_table: &TyTable, diags: &mut Vec<Diagnostic>) -> Self {
        let mut sigs = Signatures::new();

        for (def, _) in program.defs.iter() {
            let sig = match program.item(def) {
                Some(Item::Fn(item)) => {
                    check_bounds(program, ty_table, def, item, diags);
                    fn_signature(program, ty_table, item, diags)
                }
                Some(Item::Ctor(item)) => {
                    ctor_signature(program, ty_table, item.owner, &item.fields, diags)
                }
                Some(Item::Error(item)) => {
                    ctor_signature(program, ty_table, def, &item.fields, diags)
                }
                Some(Item::TraitFn(item)) => program.defs[def]
                    .parent
                    .map(|t| trait_fn_signature(program, ty_table, t, item, diags)),
                None if program.defs[def].kind == DefKind::TraitFn => {
                    prelude_trait_fn_signature(program, ty_table, def)
                }
                Some(Item::Const(item)) => item.ty.map(|ty| Signature {
                    scheme: Scheme::mono(ty_table.lower(program, &[], ty, diags)),
                    labels: Vec::new(),
                }),
                Some(Item::Op(item)) => {
                    let effect = program.defs[def].parent;
                    Some(op_signature(program, ty_table, effect, &item.sig, diags))
                }
                Some(Item::Effect(item)) => {
                    check_effect_params(program, ty_table, def, diags);
                    if let Some(label) = &item.label
                        && let Some(base) = label_base(program, ty_table, def, label, diags)
                    {
                        if label.default {
                            match sigs.default_labels.get_mut(base.def) {
                                Some(labels) => labels.push(def),
                                None => {
                                    sigs.default_labels.insert(base.def, vec![def]);
                                }
                            }
                        }
                        sigs.label_bases.insert(def, base);
                    }
                    None
                }
                Some(Item::Handler(item)) => {
                    if let Res::Def(effect) = item.effect
                        && is_label(program, effect)
                    {
                        let base = match program.item(effect) {
                            Some(Item::Effect(EffectItem {
                                label:
                                    Some(LabelOf {
                                        base: Res::Def(b), ..
                                    }),
                                ..
                            })) => program.defs[*b].name.clone(),
                            _ => SmolStr::new("?"),
                        };
                        diags.push(Diagnostic {
                            span: program.defs[def].span,
                            kind: DiagnosticKind::HandlerOfLabel {
                                handler: program.defs[def].name.clone(),
                                label: program.defs[effect].name.clone(),
                                base,
                            },
                        });
                    } else if let Res::Def(effect) = item.effect
                        && program.prelude_def("Async") != Some(effect)
                        && !is_intrinsic_effect(program, effect)
                    {
                        let scope = Scope {
                            params: &[],
                            kinds: &[],
                            omitted: Omitted::Empty,
                            sets: false,
                            self_ty: None,
                        };
                        // Generic handlers with type parameters left over are not kept
                        // (spec 8.1); omitting them is E0423.
                        let span = program.defs[def].span;
                        let name = program.defs[def].name.clone();
                        if let Some(tag) = ty_table.effect_tag(
                            program,
                            &scope,
                            effect,
                            &item.effect_args,
                            span,
                            Some(name),
                            diags,
                        ) {
                            sigs.handler_effects.insert(def, tag);
                        }
                    }
                    None
                }
                _ => None,
            };

            if let Some(sig) = sig {
                sigs.insert(def, sig);
            }
        }

        sigs
    }
}

/// Whether `effect` is declared as a label (`effect AuditLog of Console`).
pub(crate) fn is_label(program: &Program, effect: DefId) -> bool {
    matches!(program.item(effect), Some(Item::Effect(e)) if e.label.is_some())
}

/// The base of label `def` as a tag with its type arguments (`State[Int]`), or `None` with a
/// diagnostic if it cannot be used (E0429). A label has no type parameters of its own, so the
/// arguments are concrete; omitting them is E0423.
fn label_base(
    program: &Program,
    ty_table: &TyTable,
    def: DefId,
    label: &LabelOf,
    diags: &mut Vec<Diagnostic>,
) -> Option<Tag> {
    let Res::Def(base) = label.base else {
        return None;
    };
    if program.defs[base].kind != DefKind::Effect {
        return None;
    }
    let reason = if is_label(program, base) {
        let inner = program
            .label_base(base)
            .map(|b| program.defs[b].name.clone());
        Some(LabelBaseReason::Label(inner.unwrap_or_else(|| "?".into())))
    } else if program.prelude_def("Async") == Some(base) || is_intrinsic_effect(program, base) {
        Some(LabelBaseReason::BuiltIn)
    } else {
        None
    };
    if let Some(reason) = reason {
        diags.push(Diagnostic {
            span: Some(label.span),
            kind: DiagnosticKind::LabelBase {
                label: program.defs[def].name.clone(),
                base: program.defs[base].name.clone(),
                reason,
            },
        });
        return None;
    }
    let scope = Scope {
        params: &[],
        kinds: &[],
        omitted: Omitted::Empty,
        sets: false,
        self_ty: None,
    };
    ty_table.effect_tag(
        program,
        &scope,
        base,
        &label.args,
        Some(label.span),
        None,
        diags,
    )
}

impl Default for Signatures {
    fn default() -> Self {
        Self::new()
    }
}
/// `None` if no return type is written (it is inferred from the body). Also `None` if the
/// return type contains a function type with `fails` or `use` omitted, since that E and R are
/// inferred from the body. A fn with a body and no `use` clause is `None` too, since R is
/// inferred (spec 9.3). For a fn without a body (`@external`, `@intrinsic`), an omitted `use`
/// means an empty R.
fn fn_signature(
    program: &Program,
    ty_table: &TyTable,
    item: &FnItem,
    diags: &mut Vec<Diagnostic>,
) -> Option<Signature> {
    let ret = item.sig.ret?;
    if has_omitted_fails(program, ret)
        || has_omitted_uses(program, ret)
        || (item.body.is_some() && item.sig.uses.is_none())
    {
        return None;
    }
    let mut params = FnParams::new(program, ty_table, item);
    let arg_tys = params.lower_params(program, ty_table, &item.sig.params, diags);
    let labels = param_labels(program, &item.sig.params);
    let mut scope = params.scope(Omitted::Empty);
    let ret_ty = ty_table.lower_in(program, &mut scope, ret, true, diags);
    let fails = params.declared_fails(program, ty_table, &item.sig, diags);
    let mut uses = params.declared_uses(program, ty_table, &item.sig, diags);
    // `@external suspend fn` requires Async (spec 12.2).
    if item.sig.is_suspend
        && let Some(async_) = program.prelude_def("Async")
    {
        uses.insert(RowElem::Tag(Tag::new(async_)));
    }

    Some(Signature {
        scheme: Scheme::new(
            params.all_params(),
            Ty::func_sets(arg_tys, ret_ty, fails, uses),
        ),
        labels,
    })
}

/// The type of a trait's function (spec 10.1): `Self` is the first type parameter, constrained by
/// the trait, and `self` has its type. A function without a return type returns `()`. E and R are
/// the written `fails` and `use` clauses, empty if omitted (spec 10.4).
fn trait_fn_signature(
    program: &Program,
    ty_table: &TyTable,
    trait_def: DefId,
    item: &FnItem,
    diags: &mut Vec<Diagnostic>,
) -> Signature {
    let mut params = FnParams::with_self(program, ty_table, item, Some(trait_def));
    let arg_tys = params.lower_params(program, ty_table, &item.sig.params, diags);
    let labels = param_labels(program, &item.sig.params);
    let mut scope = params.scope(Omitted::Empty);
    let ret_ty = match item.sig.ret {
        Some(ret) => ty_table.lower_in(program, &mut scope, ret, true, diags),
        None => Ty::UNIT,
    };
    let fails = params.declared_fails(program, ty_table, &item.sig, diags);
    let uses = params.declared_uses(program, ty_table, &item.sig, diags);
    Signature {
        scheme: Scheme::new(
            params.all_params(),
            Ty::func_sets(arg_tys, ret_ty, fails, uses),
        ),
        labels,
    }
}

/// The functions of the Prelude's traits, which have no source (`emela_resolve::builtins`):
/// `Eq.eq(self, other: Self) -> Bool`, `Ord.compare(self, other: Self) -> Ordering` and
/// `Show.show(self) -> String`.
fn prelude_trait_fn_signature(
    program: &Program,
    ty_table: &TyTable,
    def: DefId,
) -> Option<Signature> {
    let trait_def = program.defs[def].parent?;
    let ordering = program
        .prelude_def("Ordering")
        .and_then(|d| ty_table.con(d))
        .map_or(Ty::Error, |id| Ty::named(id, []));
    let (params, ret) = match program.defs[def].name.as_str() {
        "eq" => (2, Ty::BOOL),
        "compare" => (2, ordering),
        "show" => (1, Ty::STRING),
        _ => return None,
    };
    let labels = ["self", "other"][..params]
        .iter()
        .map(|&l| Some(SmolStr::new(l)))
        .collect();
    Some(Signature {
        scheme: Scheme::new(
            vec![self_param(program, trait_def)],
            Ty::func(vec![Ty::Bound(0); params], ret),
        ),
        labels,
    })
}

/// Checks the bounds on a fn's type parameters (spec 9.5). Only Immediate is handled.
///
/// - Immediate can only be put on effect sets (type parameters written in a `use` clause)
///   (E0322).
/// - Bounds other than Immediate (traits) on an effect set are "not supported yet". Trait bounds
///   on ordinary type parameters are checked where the fn is referenced (spec 10.3).
/// - A `use` clause written in a function-typed parameter of an external function must be
///   Immediate (E0416). An omitted `use` clause is an invisible set and gets the Immediate bound
///   ([`FnParams::new`]).
fn check_bounds(
    program: &Program,
    ty_table: &TyTable,
    def: DefId,
    item: &FnItem,
    diags: &mut Vec<Diagnostic>,
) {
    let sig = &item.sig;
    let types: Vec<TypeId> = sig
        .params
        .iter()
        .filter_map(|p| p.ty)
        .chain(sig.ret)
        .collect();
    let immediate = program.prelude_def(IMMEDIATE);
    let mut constrained = Vec::new();
    let defs: Vec<DefId> = sig.type_params.iter().map(|tp| tp.def).collect();
    let sets = classify_fn_sets(
        program,
        ty_table.decl_sets(),
        &defs,
        types.iter().copied(),
        sig.uses.as_ref(),
    );
    for (i, tp) in sig.type_params.iter().enumerate() {
        let in_uses = sets[i] == Some(SetKind::Effects)
            || sig
                .uses
                .iter()
                .flat_map(|u| &u.items)
                .any(|item| item.res == Res::Def(tp.def));
        let spans = immediate_bounds(program, tp);
        if !in_uses {
            for span in spans {
                diags.push(Diagnostic {
                    span: Some(span),
                    kind: DiagnosticKind::ImmediateOnNonEffect {
                        param: program.defs[tp.def].name.clone(),
                    },
                });
            }
            continue;
        }
        if !spans.is_empty() {
            constrained.push(tp.def);
        }
        for &(res, span) in &tp.bounds {
            if matches!(res, Res::Def(d) if Some(d) != immediate) {
                diags.push(Diagnostic {
                    span: Some(span),
                    kind: DiagnosticKind::Unsupported {
                        what: "trait bounds on effect sets",
                    },
                });
            }
        }
    }
    if !item.is_external {
        return;
    }
    // The `use` clauses of an external function's callbacks.
    fn walk(
        program: &Program,
        ty: TypeId,
        constrained: &[DefId],
        effects: &mut Vec<SmolStr>,
        sets: &mut Vec<SmolStr>,
    ) {
        match &program.types[ty].kind {
            TypeKind::Path { args, .. } => {
                for &a in args {
                    walk(program, a, constrained, effects, sets);
                }
            }
            TypeKind::Tuple(elems) => {
                for &e in elems {
                    walk(program, e, constrained, effects, sets);
                }
            }
            TypeKind::Fn {
                params, ret, uses, ..
            } => {
                for item in uses.iter().flat_map(|u| &u.items) {
                    let Res::Def(d) = item.res else {
                        continue;
                    };
                    let name = program.defs[d].name.clone();
                    let list = match program.defs[d].kind {
                        DefKind::Effect if !is_immediate_effect(program, d) => &mut *effects,
                        DefKind::TypeParam if !constrained.contains(&d) => &mut *sets,
                        _ => continue,
                    };
                    if !list.contains(&name) {
                        list.push(name);
                    }
                }
                for &p in params {
                    walk(program, p, constrained, effects, sets);
                }
                if let Some(r) = ret {
                    walk(program, *r, constrained, effects, sets);
                }
            }
            TypeKind::SelfType
            | TypeKind::Unit
            | TypeKind::Missing
            | TypeKind::ErrorSet(_)
            | TypeKind::EffectSet(_) => {}
        }
    }
    for p in &sig.params {
        let Some(ty) = p.ty else {
            continue;
        };
        let (mut effects, mut sets) = (Vec::new(), Vec::new());
        walk(program, ty, &constrained, &mut effects, &mut sets);
        if effects.is_empty() && sets.is_empty() {
            continue;
        }
        diags.push(Diagnostic {
            span: program.types[ty].span,
            kind: DiagnosticKind::NotImmediate {
                effects,
                sets,
                required: ImmediateSource::ExternalCallback {
                    callee: program.defs[def].name.clone(),
                },
            },
        });
    }
}

/// The type and labels of an effect operation (spec 8.1). Being inside a declaration, omitted
/// `fails` and `use` in function-typed parameters mean empty (spec 9.2). The operation's `fails`
/// clause is its E. The operation type's R is empty; calls through a capability value add the
/// effect. Operations cannot have a `use` clause (E0137 in the syntax stage).
///
/// Effect type parameters (the `S` of `effect State[S]`) become scheme type parameters as
/// `Bound(i)`. Capability values and handler implementations substitute the tag's type
/// arguments.
fn op_signature(
    program: &Program,
    ty_table: &TyTable,
    effect: Option<DefId>,
    sig: &FnSig,
    diags: &mut Vec<Diagnostic>,
) -> Signature {
    let effect_defs: Vec<DefId> = effect
        .map(|e| effect_params(program, e).iter().map(|tp| tp.def).collect())
        .unwrap_or_default();
    // The operation's own type parameters (`fork[A]`, spec 8.1) follow the effect's. Only
    // built-in effects may have them (E0426, `check_effect_params`); they are kept either way so
    // errors do not cascade.
    let mut defs = effect_defs.clone();
    defs.extend(sig.type_params.iter().map(|tp| tp.def));
    // A type parameter of a user effect used as a set is E0424 (`check_effect_params`). Read it
    // as a set here so errors do not cascade. The set type parameters of a built-in effect
    // (`Scope[E, R]`) are decided with the whole declaration (`decl_sets`).
    let types = sig.params.iter().filter_map(|p| p.ty).chain(sig.ret);
    let mut kinds = classify_params(program, ty_table.decl_sets(), &defs, types);
    if let Some(e) = effect {
        for (i, kind) in kinds.iter_mut().enumerate().take(effect_defs.len()) {
            if ty_table.set_kind(e, i).is_some() {
                *kind = ParamKind::Set;
            }
        }
    }
    let mut scope = Scope {
        params: &defs,
        kinds: &kinds,
        omitted: Omitted::Empty,
        sets: true,
        self_ty: None,
    };
    let params: Vec<Ty> = sig
        .params
        .iter()
        .map(|p| match p.ty {
            Some(ty) => ty_table.lower_in(program, &mut scope, ty, true, diags),
            None => param_ty(program, ty_table, &defs, p, diags),
        })
        .collect();
    // A child forked through a built-in effect may use the effect itself (to fork a sibling),
    // and that is provided by the same scope, not from outside (spec 11.2): a function-typed
    // parameter whose `use` names the effect's set type parameter also gets the effect with its
    // own type parameters (`fork[A](f: fn() -> A fails E use { Scope[E, R], R })`). Inclusion then
    // only requires the child's `Scope` arguments to be contained in the scope's.
    let params = match effect {
        Some(e) if is_intrinsic_effect(program, e) => {
            let own = own_tag(ty_table, e, effect_defs.len());
            params
                .into_iter()
                .map(|p| with_own_tag(p, &own, effect_defs.len()))
                .collect()
        }
        _ => params,
    };
    let ret = match sig.ret {
        Some(ret) => ty_table.lower_in(program, &mut scope, ret, true, diags),
        None => Ty::UNIT,
    };
    let fails = match &sig.fails {
        Some(set) => ty_table.lower_error_set(program, &scope, set, diags),
        None => Row::empty(),
    };
    // An operation's `use` clause is E0137 in the syntax stage (spec 8.1); not read here.
    let ty_params = ty_params(program, &defs)
        .into_iter()
        .zip(&kinds)
        .map(|(p, &kind)| TyParam { kind, ..p })
        .collect();
    Signature {
        scheme: Scheme::new(ty_params, Ty::func_fails(params, ret, fails)),
        labels: param_labels(program, &sig.params),
    }
}

/// A built-in effect applied to its own type parameters (`Scope[E, R]` as `Bound(0)`,
/// `Bound(1)`).
fn own_tag(ty_table: &TyTable, effect: DefId, n: usize) -> Tag {
    let args = (0..n)
        .map(|i| {
            let bound = i as u32;
            match ty_table.set_kind(effect, i) {
                Some(kind) => Ty::Set(kind, Row::from_elems([RowElem::Bound(bound)])),
                None => Ty::Bound(bound),
            }
        })
        .collect();
    Tag::with_args(effect, args)
}

/// Adds `own` to the `use` of a function type that names one of the effect's set type
/// parameters (the first `n` scheme parameters).
fn with_own_tag(ty: Ty, own: &Tag, n: usize) -> Ty {
    let Ty::Fn(mut f) = ty else {
        return ty;
    };
    let names_param = f
        .uses
        .elems()
        .iter()
        .any(|e| matches!(e, RowElem::Bound(i) if (*i as usize) < n));
    if names_param {
        f.uses.insert(RowElem::Tag(own.clone()));
    }
    Ty::Fn(f)
}

/// Effect type parameters are ordinary types only (spec 8.1). A type parameter written only in
/// the `fails` or `use` of function types in an operation's parameters and return type becomes
/// a set, so it is E0424. Operations cannot have their own type parameters (E0426).
///
/// Built-in effects (`Scope`, spec 11.2) may have both.
fn check_effect_params(
    program: &Program,
    ty_table: &TyTable,
    effect: DefId,
    diags: &mut Vec<Diagnostic>,
) {
    if is_intrinsic_effect(program, effect) {
        return;
    }
    let ops: &[DefId] = match program.item(effect) {
        Some(Item::Effect(e)) => &e.ops,
        _ => &[],
    };
    for &op in ops {
        let Some(Item::Op(o)) = program.item(op) else {
            continue;
        };
        if let Some(first) = o.sig.type_params.first() {
            diags.push(Diagnostic {
                span: program.defs[first.def].span,
                kind: DiagnosticKind::OpTypeParams {
                    op: program.defs[op].name.clone(),
                },
            });
        }
    }
    let params = effect_params(program, effect);
    if params.is_empty() {
        return;
    }
    let defs: Vec<DefId> = params.iter().map(|tp| tp.def).collect();
    let types = effect_op_types(program, effect);
    let sets = classify_sets(program, ty_table.decl_sets(), &defs, types);
    for (tp, set) in params.iter().zip(sets) {
        if set.is_some() {
            diags.push(Diagnostic {
                span: program.defs[tp.def].span,
                kind: DiagnosticKind::EffectParamNotType {
                    effect: program.defs[effect].name.clone(),
                    param: program.defs[tp.def].name.clone(),
                },
            });
        }
    }
}

pub(crate) fn param_labels(program: &Program, params: &[Param]) -> Vec<Option<SmolStr>> {
    params
        .iter()
        .map(|p| Some(program.locals[p.local].name.clone()))
        .collect()
}

/// A fn's type parameters: those written (their kind decided by where they are used) and the
/// invisible set type parameters inside parameter types.
pub(crate) struct FnParams {
    pub(crate) defs: Vec<DefId>,
    pub(crate) kinds: Vec<ParamKind>,
    declared: Vec<TyParam>,
    pub(crate) hidden: Hidden,
    /// `Bound(0)` for a trait's function, whose first type parameter is `Self`.
    pub(crate) self_ty: Option<Ty>,
}

impl FnParams {
    /// For an external function, invisible effect sets in parameter types get an `Immediate`
    /// bound (callbacks must be Immediate, spec 9.5).
    pub(crate) fn new(program: &Program, ty_table: &TyTable, item: &FnItem) -> Self {
        Self::with_self(program, ty_table, item, None)
    }

    /// For a trait's function (`trait_def`), `Self` comes first: `Bound(0)`, constrained by the
    /// trait (spec 10.1). The trait's definition stands in for it in `defs`.
    pub(crate) fn with_self(
        program: &Program,
        ty_table: &TyTable,
        item: &FnItem,
        trait_def: Option<DefId>,
    ) -> Self {
        let sig = &item.sig;
        let own: Vec<DefId> = sig.type_params.iter().map(|tp| tp.def).collect();
        let defs: Vec<DefId> = trait_def.into_iter().chain(own.iter().copied()).collect();
        let types = sig.params.iter().filter_map(|p| p.ty).chain(sig.ret);
        let kinds = classify_fn_sets(
            program,
            ty_table.decl_sets(),
            &defs,
            types,
            sig.uses.as_ref(),
        )
        .into_iter()
        .map(|set| match set {
            Some(_) => ParamKind::Set,
            None => ParamKind::Type,
        })
        .collect::<Vec<_>>();
        // `classify_fn_sets` sees `Self` as an ordinary type parameter.
        let kinds: Vec<ParamKind> = match trait_def {
            Some(_) => std::iter::once(ParamKind::Type)
                .chain(kinds.into_iter().skip(1))
                .collect(),
            None => kinds,
        };
        let skip = defs.len() - own.len();
        // Immediate is kept on effect sets and traits on ordinary type parameters. Immediate on a
        // non-effect-set type parameter is made E0322 by [`check_bounds`], and a trait on an
        // effect set is "not supported yet".
        let own_params = ty_params(program, &own)
            .into_iter()
            .zip(&kinds[skip..])
            .zip(&sig.type_params)
            .map(|((p, &kind), tp)| {
                let mut p = TyParam { kind, ..p };
                match kind {
                    ParamKind::Set if !immediate_bounds(program, tp).is_empty() => {
                        p.bounds.push(SmolStr::new(IMMEDIATE));
                    }
                    ParamKind::Set => {}
                    ParamKind::Type => with_traits(program, &mut p, tp),
                }
                p
            });
        let declared = trait_def
            .map(|t| self_param(program, t))
            .into_iter()
            .chain(own_params)
            .collect();
        let mut hidden = Hidden::new(defs.len());
        hidden.immediate = item.is_external;
        FnParams {
            defs,
            kinds,
            declared,
            hidden,
            self_ty: trait_def.map(|_| Ty::Bound(0)),
        }
    }

    pub(crate) fn scope<'a>(&'a self, omitted: Omitted<'a>) -> Scope<'a> {
        Scope {
            params: &self.defs,
            kinds: &self.kinds,
            omitted,
            sets: true,
            self_ty: self.self_ty.as_ref(),
        }
    }

    /// Function types with `fails` omitted get an invisible set type parameter. Parameters
    /// without a type push E0316 (spec 6.1) and become `Ty::Error`.
    pub(crate) fn lower_params(
        &mut self,
        program: &Program,
        ty_table: &TyTable,
        params: &[Param],
        diags: &mut Vec<Diagnostic>,
    ) -> Vec<Ty> {
        params
            .iter()
            .map(|p| {
                // A trait's `self` has no written type.
                if let (None, Some(self_ty)) = (p.ty, &self.self_ty)
                    && program.locals[p.local].kind == LocalKind::SelfParam
                {
                    return self_ty.clone();
                }
                let Some(ty) = p.ty else {
                    return param_ty(program, ty_table, &self.defs, p, diags);
                };
                self.hidden
                    .start_param(program.locals[p.local].name.clone());
                let mut scope = Scope {
                    params: &self.defs,
                    kinds: &self.kinds,
                    omitted: Omitted::Hidden(&mut self.hidden),
                    sets: true,
                    self_ty: self.self_ty.as_ref(),
                };
                ty_table.lower_in(program, &mut scope, ty, true, diags)
            })
            .collect()
    }

    /// The declared E: the written `fails` clause (empty if none) united with the invisible sets
    /// in positive positions of the parameter types. Invisible sets cannot be written, so errors
    /// from calling a function parameter may always propagate (spec 9.2).
    pub(crate) fn declared_fails(
        &self,
        program: &Program,
        ty_table: &TyTable,
        sig: &FnSig,
        diags: &mut Vec<Diagnostic>,
    ) -> Row {
        let written = match &sig.fails {
            Some(set) => ty_table.lower_error_set(program, &self.scope(Omitted::Empty), set, diags),
            None => Row::empty(),
        };
        written.union(&self.hidden.positive)
    }

    /// The declared R: the written `use` clause united with the invisible effect sets in positive
    /// positions of the parameter types. Unused when no `use` clause is written, since R is then
    /// inferred from the body.
    pub(crate) fn declared_uses(
        &self,
        program: &Program,
        ty_table: &TyTable,
        sig: &FnSig,
        diags: &mut Vec<Diagnostic>,
    ) -> Row {
        let written = match &sig.uses {
            Some(set) => {
                ty_table.lower_effect_set(program, &self.scope(Omitted::Empty), set, diags)
            }
            None => Row::empty(),
        };
        written.union(&self.hidden.positive_uses)
    }

    /// The written type parameters followed by the invisible set type parameters.
    pub(crate) fn all_params(&self) -> Vec<TyParam> {
        self.declared
            .iter()
            .chain(&self.hidden.params)
            .cloned()
            .collect()
    }
}

/// The type and labels of a constructor. `owner` is the type it constructs (type, enum, error or
/// handler). A handler's constructor makes a handler value (spec 8.4) and may only be used in
/// that handler's `init` (E0418).
///
/// A constructor without fields (`None`, `Empty`) is a value, not a function, so its type is the
/// constructed type itself.
fn ctor_signature(
    program: &Program,
    ty_table: &TyTable,
    owner: DefId,
    fields: &Fields,
    diags: &mut Vec<Diagnostic>,
) -> Option<Signature> {
    let ret_ty = ty_table.self_ty(owner)?;
    let defs = type_param_defs(program, owner);

    let (field_tys, labels): (Vec<TypeId>, Vec<Option<SmolStr>>) = match fields {
        Fields::None => (Vec::new(), Vec::new()),
        Fields::Positional(tys) => (tys.clone(), vec![None; tys.len()]),
        Fields::Named(fs) => fs.iter().map(|f| (f.ty, Some(f.name.clone()))).unzip(),
    };
    // Omitted `fails` and `use` in fields' function types mean empty (spec 9.2). The kinds of
    // type parameters (whether sets) are as the type table decided (`type Handler[E, R]`).
    let params: Vec<TyParam> = match ty_table.con(owner) {
        Some(id) => ty_table.cons[id].params.clone(),
        None => ty_params(program, &defs),
    };
    let kinds: Vec<ParamKind> = params.iter().map(|p| p.kind).collect();
    let mut scope = Scope {
        params: &defs,
        kinds: &kinds,
        omitted: Omitted::Empty,
        sets: true,
        self_ty: None,
    };
    let ty = match fields {
        Fields::None => ret_ty,
        _ => Ty::func(
            field_tys
                .iter()
                .map(|&t| ty_table.lower_in(program, &mut scope, t, true, diags))
                .collect::<Vec<_>>(),
            ret_ty,
        ),
    };

    Some(Signature {
        scheme: Scheme::new(params, ty),
        labels,
    })
}

/// The type of a top-level fn parameter. If none is written, pushes E0316 (spec 6.1) and returns
/// `Ty::Error`.
pub(crate) fn param_ty(
    program: &Program,
    ty_table: &TyTable,
    type_params: &[DefId],
    param: &Param,
    diags: &mut Vec<Diagnostic>,
) -> Ty {
    match param.ty {
        Some(ty) => ty_table.lower(program, type_params, ty, diags),
        None => {
            let local = &program.locals[param.local];
            diags.push(Diagnostic {
                span: Some(local.span),
                kind: DiagnosticKind::MissingParamType {
                    param: local.name.clone(),
                },
            });
            Ty::Error
        }
    }
}

/// Adds the trait constraints written on an ordinary type parameter (`A: Ord + Show`). Immediate
/// is not a trait here; on an ordinary type parameter it is E0322 ([`check_bounds`]).
pub(crate) fn with_traits(program: &Program, param: &mut TyParam, tp: &TypeParam) {
    for &(res, _) in &tp.bounds {
        if let Res::Def(t) = res
            && program.defs[t].kind == DefKind::Trait
            && program.prelude_def(IMMEDIATE) != Some(t)
            && !param.traits.contains(&t)
        {
            param.traits.push(t);
            param.bounds.push(program.defs[t].name.clone());
        }
    }
}

/// `Self` of a trait's functions, constrained by the trait.
pub(crate) fn self_param(program: &Program, trait_def: DefId) -> TyParam {
    TyParam {
        traits: vec![trait_def],
        bounds: vec![program.defs[trait_def].name.clone()],
        ..TyParam::new("Self")
    }
}
