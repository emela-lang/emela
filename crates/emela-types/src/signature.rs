//! Declared types: the types ([`Scheme`]) of fns, constructors and consts, and parameter labels.
//!
//! Call inference looks up the callee's type and labels here. Fns without a written return type,
//! fns with a body but no `uses` clause, and consts without a written type are added after their
//! bodies are inferred. Effect operation types also live here, for calls through capability
//! values.

use emela_resolve::hir::{
    DefId, DefKind, DefModule, EffectItem, Fields, FnItem, FnSig, HandlerItem, Item, LabelOf,
    LocalKind, MemberItem, Param, Program, Res, Span, TypeId, TypeKind,
};
use la_arena::ArenaMap;
use smol_str::SmolStr;

use crate::diagnostic::{Diagnostic, DiagnosticKind, ImmediateSource, LabelBaseReason};
use crate::lower::{
    FieldCaps, Hidden, Omitted, Scope, TyTable, classify_fn_sets, classify_params, classify_sets,
    effect_op_types, effect_params, has_omitted_fails, has_omitted_uses, immediate_bounds,
    is_immediate_effect, is_intrinsic_effect, prelude_bounds, ty_params, type_param_defs,
    with_traits,
};
use crate::row::{Row, RowElem, SetKind, Tag};
use crate::scheme::Scheme;
use crate::ty::{CAPABILITY, IMMEDIATE, ParamKind, Ty, TyParam};

pub struct Signature {
    pub scheme: Scheme,
    /// One label per parameter; `None` for positional-only fields. Empty for non-functions.
    pub labels: Vec<Option<SmolStr>>,
}

/// The parameters of a handler's init (`init(binding: String)`, `[effect.handler]`). Handlers
/// are not generic, so the types are concrete.
#[derive(Clone, Debug, Default)]
pub struct InitParams {
    pub tys: Vec<Ty>,
    pub labels: Vec<Option<SmolStr>>,
    /// The number of capability sets in `tys`, `Bound(0)` and on: each item of a with that runs
    /// the init decides them (`[effect.handler]`).
    pub caps: usize,
}

/// Also holds the effect tags that handlers implement and the parameters of their inits.
pub struct Signatures {
    by_def: ArenaMap<DefId, Signature>,
    /// Handler → the parameters of its init. Absent for a handler whose init takes none.
    init_params: ArenaMap<DefId, InitParams>,
    /// Handler → the effects it implements and their type arguments (`H: State[Int]`, spec 8.1),
    /// in the order written (`[effect.handler]`). Host effects, built-in effects, labels
    /// (E4003), unknown effects and an effect listed a second time (E4012) are left out; a
    /// handler with none left is absent.
    handler_effects: ArenaMap<DefId, Vec<Tag>>,
    /// Label → the effect it stands for (`effect AuditLog of Console`, issue #255). Labels whose
    /// base cannot be used (E4201) are absent.
    label_bases: ArenaMap<DefId, Tag>,
    /// Effect → its labels declared `default`, in definition order. A `with` providing the effect
    /// also provides them (issue #255).
    default_labels: ArenaMap<DefId, Vec<DefId>>,
}

impl Signatures {
    pub fn new() -> Self {
        Signatures {
            by_def: ArenaMap::default(),
            init_params: ArenaMap::default(),
            handler_effects: ArenaMap::default(),
            label_bases: ArenaMap::default(),
            default_labels: ArenaMap::default(),
        }
    }

    /// provides(H) (`[type.rules]`): the effects `handler` implements, in the order written.
    /// Empty for a handler that implements none that can be provided.
    pub fn handler_effects(&self, handler: DefId) -> &[Tag] {
        self.handler_effects.get(handler).map_or(&[], Vec::as_slice)
    }

    /// The effect of `handler` that declares the operation or handler member `op`, with the type
    /// arguments written in the handler's head (`[effect.handler]`).
    pub fn handler_effect_of(&self, program: &Program, handler: DefId, op: DefId) -> Option<&Tag> {
        let effect = program.defs[op].parent?;
        self.handler_effects(handler)
            .iter()
            .find(|t| t.def == effect)
    }

    /// The parameters of a handler's init. `None` if it takes none (or has no init).
    pub fn init_params(&self, handler: DefId) -> Option<&InitParams> {
        self.init_params.get(handler)
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
            check_borrowed(program, def, diags);
            let sig = match program.item(def) {
                Some(Item::Fn(item)) => {
                    check_bounds(program, ty_table, def, item, diags);
                    if item.is_external {
                        check_external_uses(program, def, item, diags);
                    }
                    fn_signature(program, ty_table, def, item, diags)
                }
                Some(Item::Ctor(item)) => {
                    ctor_signature(program, ty_table, item.owner, def, &item.fields, diags)
                }
                Some(Item::Error(item)) => {
                    ctor_signature(program, ty_table, def, def, &item.fields, diags)
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
                    let name = &program.defs[def].name;
                    Some(op_signature(
                        program, ty_table, effect, name, &item.sig, diags,
                    ))
                }
                Some(Item::Member(item)) => {
                    let effect = program.defs[def].parent;
                    Some(member_signature(program, ty_table, effect, item, diags))
                }
                Some(Item::Effect(item)) => {
                    check_effect_params(program, ty_table, def, diags);
                    // Only the compiler declares effects without operations: the host effects,
                    // which are in the Prelude, and External (`[effect.decl]`). A label has the
                    // operations of its base. An effect with handler members has something to
                    // provide.
                    if item.ops.is_empty()
                        && item.members.is_empty()
                        && item.label.is_none()
                        && matches!(program.defs[def].module, DefModule::Source(_))
                        && !program.is_external_effect(def)
                    {
                        diags.push(Diagnostic {
                            span: program.defs[def].span,
                            kind: DiagnosticKind::EffectWithoutOps {
                                effect: program.defs[def].name.clone(),
                            },
                        });
                    }
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
                    check_handler_params(program, ty_table, def, item, diags);
                    let defs = type_param_defs(program, def);
                    if !item.init_params.is_empty() {
                        let init =
                            init_params(program, ty_table, def, &defs, &item.init_params, diags);
                        sigs.init_params.insert(def, init);
                    }
                    let mut tags: Vec<Tag> = Vec::new();
                    for effect_ref in &item.effects {
                        let Res::Def(effect) = effect_ref.res else {
                            continue;
                        };
                        if is_label(program, effect) {
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
                            continue;
                        }
                        // A built-in effect is provided only by its built-in handler.
                        if program.is_provided_by_host(effect)
                            || (is_intrinsic_effect(program, effect) && !item.is_intrinsic)
                        {
                            continue;
                        }
                        let kinds = ty_table.param_kinds(def, defs.len());
                        let scope = Scope {
                            params: &defs,
                            kinds: &kinds,
                            omitted: Omitted::Empty,
                            sets: item.is_intrinsic,
                            self_ty: None,
                            result_caps: None,
                            quant: None,
                        };
                        // The type arguments are written; they may name the handler's own type
                        // parameters (`[effect.decl]`). Omitting them is E4008.
                        let span = program.defs[def].span;
                        let name = program.defs[def].name.clone();
                        let Some(tag) = ty_table.effect_tag(
                            program,
                            &scope,
                            effect,
                            &effect_ref.args,
                            span,
                            Some(name),
                            diags,
                        ) else {
                            continue;
                        };
                        check_listed_effect(program, def, &tags, &tag, effect_ref.span, diags);
                        if !tags.contains(&tag) {
                            tags.push(tag);
                        }
                    }
                    if !tags.is_empty() {
                        sigs.handler_effects.insert(def, tags);
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

/// The uses clause of an external function contains only ProcessHost and PageHost (E5002,
/// `[ext.call]`). Async is declared with `suspend fn` instead, and a set type parameter or an
/// effect of the program would let the host function run outside a handler.
/// `@borrowed` can be written only on a parameter of function type of an external function
/// (E5004, `[ext.boundary]`).
fn check_borrowed(program: &Program, def: DefId, diags: &mut Vec<Diagnostic>) {
    let (params, external): (Vec<&Param>, bool) = match program.item(def) {
        Some(Item::Fn(f) | Item::TraitFn(f)) => (f.sig.params.iter().collect(), f.is_external),
        Some(Item::Op(o)) => (o.sig.params.iter().collect(), false),
        Some(Item::Member(m)) => (m.sig.params.iter().collect(), false),
        Some(Item::Handler(h)) => (
            h.init_params
                .iter()
                .chain(h.ops.iter().flat_map(|o| &o.params))
                .collect(),
            false,
        ),
        _ => return,
    };
    for p in params {
        let Some(span) = p.borrowed else {
            continue;
        };
        let fn_typed =
            p.ty.is_some_and(|t| matches!(program.types[t].kind, TypeKind::Fn { .. }));
        if !(external && fn_typed) {
            diags.push(Diagnostic {
                span: Some(span),
                kind: DiagnosticKind::BorrowedPosition,
            });
        }
    }
}

fn check_external_uses(program: &Program, def: DefId, item: &FnItem, diags: &mut Vec<Diagnostic>) {
    let Some(uses) = &item.sig.uses else {
        return;
    };
    for effect in &uses.items {
        let allowed = match effect.res {
            Res::Def(d) => program.is_host_effect(d) && program.defs[d].name != "Async",
            // An unresolved name is reported by name resolution.
            Res::Err => true,
            _ => false,
        };
        if !allowed {
            diags.push(Diagnostic {
                span: Some(effect.span),
                kind: DiagnosticKind::ExternalUses {
                    function: program.defs[def].name.clone(),
                    external: matches!(effect.res, Res::Def(d) if program.is_external_effect(d)),
                },
            });
        }
    }
}

/// Whether `effect` is declared as a label (`effect AuditLog of Console`).
pub(crate) fn is_label(program: &Program, effect: DefId) -> bool {
    matches!(program.item(effect), Some(Item::Effect(e)) if e.label.is_some())
}

/// The base of label `def` as a tag with its type arguments (`State[Int]`), or `None` with a
/// diagnostic if it cannot be used (E4201). A label has no type parameters of its own, so the
/// arguments are concrete; omitting them is E4008.
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
    } else if program.is_provided_by_host(base) || is_intrinsic_effect(program, base) {
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
        result_caps: None,
        quant: None,
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
/// return type contains a function type with `fails` or `uses` omitted, since that E and R are
/// inferred from the body, except for an external function, which has no body: there they are
/// empty (`[ext.boundary]`, a hidden set variable is taken as empty). A fn with a body and no
/// `uses` clause is `None` too, since R is inferred (spec 9.3). For a fn without a body
/// (`@external`, `@intrinsic`), an omitted `use` means an empty R.
///
/// An external function has the External of its module in R and in the functions it returns
/// ([`add_external`], `[effect.external]`).
fn fn_signature(
    program: &Program,
    ty_table: &TyTable,
    def: DefId,
    item: &FnItem,
    diags: &mut Vec<Diagnostic>,
) -> Option<Signature> {
    let ret = item.sig.ret?;
    if (!item.is_external && (has_omitted_fails(program, ret) || has_omitted_uses(program, ret)))
        || (item.body.is_some() && item.sig.uses.is_none())
    {
        return None;
    }
    let mut params = FnParams::new(program, ty_table, item);
    let arg_tys = params.lower_params(program, ty_table, &item.sig.params, diags);
    let labels = param_labels(program, &item.sig.params);
    let mut scope = params.result_scope(Omitted::Empty);
    let ret_ty = ty_table.lower_in(program, &mut scope, ret, true, diags);
    let fails = params.declared_fails(program, ty_table, &item.sig, diags);
    let mut uses = params.declared_uses(program, ty_table, &item.sig, diags);
    let mut ret_ty = ret_ty;
    if item.is_external {
        add_external(program, def, item, &mut uses, &mut ret_ty);
    }
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

/// Adds the External of the module that declares the external function `def` to its R `uses`,
/// and, with the host effects of its uses clause, to the R of the functions it returns: `ret`
/// when it is a function type, and the return types of those function types in turn
/// (`[effect.external]`). The host may return a host function or a callback it received
/// earlier. The parameters of those function types are callbacks that Emela passes to the host
/// and are left as written (`[ext.boundary]`).
///
/// An external function declared with `uses {}` that is not a `suspend fn` adds nothing
/// (`[ext.call]`).
fn add_external(program: &Program, def: DefId, item: &FnItem, uses: &mut Row, ret: &mut Ty) {
    let declared_pure = item.sig.uses.as_ref().is_some_and(|u| u.items.is_empty());
    if declared_pure && !item.sig.is_suspend {
        return;
    }
    let DefModule::Source(module) = program.defs[def].module else {
        return;
    };
    let Some(external) = program.external_effect(module) else {
        return;
    };
    let mut added = uses.clone();
    added.insert(RowElem::Tag(Tag::new(external)));
    *uses = added.clone();
    let mut ret = ret;
    while let Ty::Fn(f) = ret {
        for e in added.elems() {
            f.uses.insert(e.clone());
        }
        ret = &mut f.ret;
    }
}

/// The type of a trait's function (spec 10.1): `Self` is the first type parameter, constrained by
/// the trait, and `self` has its type. A function without a return type returns `()`. E and R are
/// the written `fails` and `uses` clauses, empty if omitted (spec 10.4).
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
    let mut scope = params.result_scope(Omitted::Empty);
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

/// Checks the bounds on a fn's type parameters (spec 9.5, `[trait.builtin]`).
///
/// - Immediate can only be put on effect sets (type parameters written in a `uses` clause)
///   (E4304).
/// - Capability can only be put on ordinary type parameters, not on error sets or effect sets
///   (E4408).
/// - Bounds other than Immediate (traits) on an effect set are "not supported yet". Trait bounds
///   on ordinary type parameters are checked where the fn is referenced (spec 10.3).
/// - A `uses` clause written in a function-typed parameter of an external function must be
///   Immediate (E4302). An omitted `uses` clause is an invisible set and gets the Immediate bound
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
    let capability = program.prelude_def(CAPABILITY);
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
        // Capability constrains only ordinary type parameters (E4408, `[trait.builtin]`).
        if let Some(set) = if in_uses {
            Some(SetKind::Effects)
        } else {
            sets[i]
        } {
            for span in prelude_bounds(program, tp, CAPABILITY) {
                diags.push(Diagnostic {
                    span: Some(span),
                    kind: DiagnosticKind::CapabilityOnSet {
                        param: program.defs[tp.def].name.clone(),
                        set,
                    },
                });
            }
        }
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
            if matches!(res, Res::Def(d) if Some(d) != immediate && Some(d) != capability) {
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
    // The `uses` clauses of an external function's callbacks. An effect other than a host
    // effect cannot be there at all (`[ext.boundary]`); a host effect and a set must be
    // Immediate.
    struct Found {
        callback: Vec<SmolStr>,
        effects: Vec<SmolStr>,
        sets: Vec<SmolStr>,
    }
    fn walk(program: &Program, ty: TypeId, constrained: &[DefId], found: &mut Found) {
        match &program.types[ty].kind {
            TypeKind::Path { args, .. } => {
                for &a in args {
                    walk(program, a, constrained, found);
                }
            }
            TypeKind::Tuple(elems) => {
                for &e in elems {
                    walk(program, e, constrained, found);
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
                        DefKind::Effect if !program.is_host_effect(d) => &mut found.callback,
                        DefKind::Effect if !is_immediate_effect(program, d) => &mut found.effects,
                        DefKind::TypeParam if !constrained.contains(&d) => &mut found.sets,
                        _ => continue,
                    };
                    if !list.contains(&name) {
                        list.push(name);
                    }
                }
                for &p in params {
                    walk(program, p, constrained, found);
                }
                if let Some(r) = ret {
                    walk(program, *r, constrained, found);
                }
            }
            TypeKind::SelfType
            | TypeKind::Unit
            | TypeKind::Missing
            | TypeKind::Cap(_)
            | TypeKind::ErrorSet(_)
            | TypeKind::EffectSet(_) => {}
        }
    }
    for p in &sig.params {
        let Some(ty) = p.ty else {
            continue;
        };
        let mut found = Found {
            callback: Vec::new(),
            effects: Vec::new(),
            sets: Vec::new(),
        };
        walk(program, ty, &constrained, &mut found);
        let Found {
            callback,
            effects,
            sets,
        } = found;
        if !callback.is_empty() {
            diags.push(Diagnostic {
                span: program.types[ty].span,
                kind: DiagnosticKind::CallbackEffects {
                    externals: Vec::new(),
                    effects: callback,
                    callee: None,
                    bound_item: true,
                },
            });
        }
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
/// `fails` and `uses` in function-typed parameters mean empty (spec 9.2). The operation's `fails`
/// clause is its E. The operation type's R is empty; calls through a capability value add the
/// effect. Operations cannot have a `uses` clause (E1211 in the syntax stage). An omitted return
/// type is E1101 in the parser; it is read as `()` here to go on.
///
/// Effect type parameters (the `S` of `effect State[S]`) become scheme type parameters as
/// `Bound(i)`. Capability values and handler implementations substitute the tag's type
/// arguments.
fn op_signature(
    program: &Program,
    ty_table: &TyTable,
    effect: Option<DefId>,
    name: &SmolStr,
    sig: &FnSig,
    diags: &mut Vec<Diagnostic>,
) -> Signature {
    let effect_defs: Vec<DefId> = effect
        .map(|e| effect_params(program, e).iter().map(|tp| tp.def).collect())
        .unwrap_or_default();
    // The operation's own type parameters (`start[A]`, spec 8.1) follow the effect's. Only
    // built-in effects may have them (E1213, `check_effect_params`); they are kept either way so
    // errors do not cascade.
    let mut defs = effect_defs.clone();
    defs.extend(sig.type_params.iter().map(|tp| tp.def));
    // A type parameter of a user effect used as a set is E4009 (`check_effect_params`). Read it
    // as a set here so errors do not cascade. The set type parameters of a built-in effect
    // (`TaskGroup[E, R]`) are decided with the whole declaration (`decl_sets`).
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
        result_caps: None,
        quant: None,
    };
    // A capability set in a parameter type is decided at each call (`[effect.handler]`): a set
    // type parameter after the others, in the R of the operation as for a function
    // (`[type.inference]`).
    let mut param_caps = FieldCaps::finding(None, defs.len() as u32);
    let params: Vec<Ty> = sig
        .params
        .iter()
        .map(|p| match p.ty {
            Some(ty) => {
                param_caps.start_field(program.locals[p.local].name.clone());
                let mut scope = Scope {
                    params: &defs,
                    kinds: &kinds,
                    omitted: Omitted::Fields(&mut param_caps),
                    sets: true,
                    self_ty: None,
                    result_caps: None,
                    quant: None,
                };
                ty_table.lower_in(program, &mut scope, ty, true, diags)
            }
            None => param_ty(program, ty_table, &defs, p, diags),
        })
        .collect();
    let param_caps = param_caps.found();
    let caps_uses =
        Row::from_elems((0..param_caps.len()).map(|i| RowElem::Bound((defs.len() + i) as u32)));
    // A task started through a built-in effect may use the effect itself (to start a sibling),
    // and that is provided by the same group, not from outside (`[task.group]`): a function-typed
    // parameter whose `use` names the effect's set type parameter also gets the effect with its
    // own type parameters (`start[A](f: fn() -> A fails E uses { TaskGroup[E, R], R })`).
    // Inclusion then only requires the task's `TaskGroup` arguments to be contained in the
    // group's.
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
    // A capability-bound type of the operation's own effect in the result records the receiver
    // (`[effect.handler]`): a set type parameter after the others, which a call fills with the
    // set of the capability it is called on, and a handler with its `self`.
    let receiver = (defs.len() + param_caps.len()) as u32;
    let receiver_caps: Vec<(Vec<DefId>, Row)> = effect
        .map(|e| (vec![e], Row::from_elems([RowElem::Bound(receiver)])))
        .into_iter()
        .collect();
    scope.result_caps = Some(&receiver_caps);
    let ret = match sig.ret {
        Some(ret) => {
            let ty = ty_table.lower_in(program, &mut scope, ret, true, diags);
            if let Some(found) = caps_in_op_result(program, ty_table, effect, ret, &ty) {
                diags.push(Diagnostic {
                    span: program.types[ret].span,
                    kind: DiagnosticKind::CapabilityInOpResult {
                        op: name.clone(),
                        ty: found,
                    },
                });
            }
            ty
        }
        None => Ty::UNIT,
    };
    scope.result_caps = None;
    let fails = match &sig.fails {
        Some(set) => ty_table.lower_error_set(program, &scope, set, diags),
        None => Row::empty(),
    };
    // An operation's `uses` clause is E1211 in the syntax stage (spec 8.1); not read here.
    let mut ty_params: Vec<TyParam> = ty_params(program, &defs)
        .into_iter()
        .zip(&kinds)
        .map(|(p, &kind)| TyParam { kind, ..p })
        .collect();
    for (tags, name) in param_caps {
        let mut p = TyParam::set(name);
        p.caps = Some(tags);
        ty_params.push(p);
    }
    if let Some(e) = effect
        && mentions_bound(&ret, receiver)
    {
        let mut p = TyParam::set("C(self)");
        p.caps = Some(vec![Tag::new(e)]);
        ty_params.push(p);
    }
    Signature {
        scheme: Scheme::new(ty_params, Ty::func_sets(params, ret, fails, caps_uses)),
        labels: param_labels(program, &sig.params),
    }
}

/// What makes the result type `ty` of an operation of `effect` E4406 (`[effect.handler]`), as
/// written: a capability type, or a type with hidden capability sets of another effect.
fn caps_in_op_result(
    program: &Program,
    ty_table: &TyTable,
    effect: Option<DefId>,
    written: TypeId,
    ty: &Ty,
) -> Option<SmolStr> {
    if let Some(cap) = written_cap(program, written) {
        return Some(cap);
    }
    let mut found = None;
    ty.visit(&mut |t| {
        if found.is_some() {
            return;
        }
        if let Ty::Named(con, _) = t {
            let def = ty_table.def(*con);
            if let Some(caps) = ty_table.decl_caps(def)
                && caps
                    .effects
                    .iter()
                    .flatten()
                    .any(|tag| Some(tag.def) != effect)
            {
                found = Some(program.defs[def].name.clone());
            }
        }
    });
    found
}

/// The first capability type `use E` written in the type expression `ty`, as written.
fn written_cap(program: &Program, ty: TypeId) -> Option<SmolStr> {
    match &program.types[ty].kind {
        TypeKind::Cap(effects) => Some(SmolStr::new(format!(
            "use {}",
            effects
                .iter()
                .filter_map(|e| match e.res {
                    Res::Def(d) => Some(program.defs[d].name.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" + ")
        ))),
        TypeKind::Path { args, .. } => args.iter().find_map(|&a| written_cap(program, a)),
        TypeKind::Tuple(elems) => elems.iter().find_map(|&e| written_cap(program, e)),
        TypeKind::Fn { params, ret, .. } => params
            .iter()
            .chain(ret)
            .find_map(|&t| written_cap(program, t)),
        _ => None,
    }
}

/// Whether a capability set in `ty` has the set type parameter `i`.
fn mentions_bound(ty: &Ty, i: u32) -> bool {
    let mut found = false;
    ty.visit(&mut |t| match t {
        Ty::Set(SetKind::Caps, row) | Ty::Cap(_, row) => {
            found |= row.elems().contains(&RowElem::Bound(i));
        }
        _ => {}
    });
    found
}

/// The checks on one more effect `tag` listed after the `:` of `handler`, after `earlier`
/// (`[effect.handler]`): the same effect listed again is E4012, and an operation or handler
/// member whose name an earlier effect also has is E4012, since it would not be decided which
/// implementation is which. Both are reported at the effect written.
fn check_listed_effect(
    program: &Program,
    handler: DefId,
    earlier: &[Tag],
    tag: &Tag,
    span: Span,
    diags: &mut Vec<Diagnostic>,
) {
    let handler_name = program.defs[handler].name.clone();
    if earlier.contains(tag) {
        diags.push(Diagnostic {
            span: Some(span),
            kind: DiagnosticKind::RepeatedHandlerEffect {
                handler: handler_name,
                effect: program.defs[tag.def].name.clone(),
            },
        });
        return;
    }
    let names = |effect: DefId| {
        program
            .effect_ops(effect)
            .iter()
            .chain(program.effect_members(effect))
            .map(|&d| program.defs[d].name.clone())
            .collect::<Vec<_>>()
    };
    let own = names(tag.def);
    for other in earlier {
        for name in names(other.def) {
            if own.contains(&name) {
                diags.push(Diagnostic {
                    span: Some(span),
                    kind: DiagnosticKind::SharedOperationName {
                        handler: handler_name.clone(),
                        op: name,
                        first: program.defs[other.def].name.clone(),
                        second: program.defs[tag.def].name.clone(),
                    },
                });
            }
        }
    }
}

/// The type of opening a handler member (`[effect.decl]`, `[type.rules]`): a function from the
/// parameters of the init to the capability of the handler opened, with the member's `fails`
/// as its E and the effect that declares the member as its R. The scheme's parameters are the
/// effect's type parameters, as for an operation. Opening through a capability (`c.Open(p)`)
/// replaces the R with the capability.
fn member_signature(
    program: &Program,
    ty_table: &TyTable,
    effect: Option<DefId>,
    item: &MemberItem,
    diags: &mut Vec<Diagnostic>,
) -> Signature {
    let mut sig = op_signature(
        program,
        ty_table,
        effect,
        &SmolStr::default(),
        &item.sig,
        diags,
    );
    let defs: Vec<DefId> = effect
        .map(|e| effect_params(program, e).iter().map(|tp| tp.def).collect())
        .unwrap_or_default();
    let kinds = vec![ParamKind::Type; defs.len()];
    let scope = Scope {
        params: &defs,
        kinds: &kinds,
        omitted: Omitted::Empty,
        sets: false,
        self_ty: None,
        result_caps: None,
        quant: None,
    };
    // The capability opened has every effect after the `:` (`[grammar.decl]`).
    let mut provided = Some(Vec::with_capacity(item.effects.len()));
    for effect in &item.effects {
        let tag = match effect.res {
            Res::Def(f) => ty_table.effect_tag(
                program,
                &scope,
                f,
                &effect.args,
                Some(effect.span),
                None,
                diags,
            ),
            _ => None,
        };
        match (tag, provided.as_mut()) {
            (Some(tag), Some(tags)) => tags.push(tag),
            _ => provided = None,
        }
    }
    let provided = provided.filter(|tags| !tags.is_empty());
    if let Ty::Fn(f) = &mut sig.scheme.ty {
        // The capability of the item that opens it (`[effect.with]`); the item gives its set.
        f.ret = provided.map_or(Ty::Error, |tags| Ty::cap(tags, Row::empty()));
        if let Some(e) = effect {
            let args = (0..defs.len()).map(|i| Ty::Bound(i as u32)).collect();
            f.uses = Row::of_tag(Tag::with_args(e, args));
        }
    }
    sig
}

/// A built-in effect applied to its own type parameters (`TaskGroup[E, R]` as `Bound(0)`,
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
///
/// Without it, the task's `TaskGroup[?e, ?r]` would become a lower bound of `?r` itself: the
/// group would demand `TaskGroup` from outside (E4301, and the checker could overflow its stack),
/// and in nested groups the task's `TaskGroup` would be met by the outer group while lowering
/// points it at the inner one.
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
/// the `fails` or `uses` of function types in an operation's parameters and return type becomes
/// a set, so it is E4009. Operations cannot have their own type parameters (E1213).
///
/// Built-in effects (`TaskGroup`, `[task.group]`) may have both.
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

/// The type parameters of a handler (`[effect.handler]`). Each is an ordinary type: one that
/// appears only in the `fails` or `uses` of a function type would be a set (E4009). Each appears
/// in the effects after the colon, in the type of a field or in the type of a parameter of init
/// (E4013): the type of a handler cannot be written, so one that appears nowhere else would tell
/// nothing apart. An inner handler cannot have its own (E1213); it reads the declaration with
/// those of its outer handler.
fn check_handler_params(
    program: &Program,
    ty_table: &TyTable,
    handler: DefId,
    item: &HandlerItem,
    diags: &mut Vec<Diagnostic>,
) {
    let inner = program.defs[handler]
        .parent
        .is_some_and(|p| program.defs[p].kind == DefKind::Handler);
    if inner {
        if let Some(first) = item.type_params.first() {
            diags.push(Diagnostic {
                span: program.defs[first.def].span,
                kind: DiagnosticKind::InnerHandlerTypeParams {
                    handler: program.defs[handler].name.clone(),
                },
            });
        }
        return;
    }
    // A built-in handler may have set type parameters (`Group[E, R]`, `[task.group]`).
    if item.type_params.is_empty() || item.is_intrinsic {
        return;
    }
    let defs: Vec<DefId> = item.type_params.iter().map(|tp| tp.def).collect();
    let types: Vec<TypeId> = item
        .effects
        .iter()
        .flat_map(|e| e.args.iter().copied())
        .chain(item.fields.iter().map(|f| f.ty))
        .chain(item.init_params.iter().filter_map(|p| p.ty))
        .collect();
    let marks = crate::lower::mark_all(program, ty_table.decl_sets(), &defs, types);
    for (tp, (in_type, in_set)) in item.type_params.iter().zip(marks) {
        let param = program.defs[tp.def].name.clone();
        let kind = if in_type {
            continue;
        } else if in_set.is_some() {
            DiagnosticKind::HandlerParamNotType {
                handler: program.defs[handler].name.clone(),
                param,
            }
        } else {
            DiagnosticKind::UnusedHandlerParam {
                handler: program.defs[handler].name.clone(),
                param,
            }
        };
        diags.push(Diagnostic {
            span: program.defs[tp.def].span,
            kind,
        });
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
        // non-effect-set type parameter is made E4304 by [`check_bounds`], and a trait on an
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
            result_caps: None,
            quant: None,
        }
    }

    /// The scope of the result type: a capability type there takes its set from the parameters
    /// (`[type.rules]`).
    pub(crate) fn result_scope<'a>(&'a self, omitted: Omitted<'a>) -> Scope<'a> {
        Scope {
            result_caps: Some(&self.hidden.caps),
            ..self.scope(omitted)
        }
    }

    /// Function types with `fails` omitted get an invisible set type parameter. Parameters
    /// without a type push E3006 (spec 6.1) and become `Ty::Error`.
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
                    result_caps: None,
                    quant: None,
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

    /// The declared R: the written `uses` clause united with the invisible effect sets in positive
    /// positions of the parameter types. Unused when no `uses` clause is written, since R is then
    /// inferred from the body.
    pub(crate) fn declared_uses(
        &self,
        program: &Program,
        ty_table: &TyTable,
        sig: &FnSig,
        diags: &mut Vec<Diagnostic>,
    ) -> Row {
        // A capability type in the type arguments of an effect takes its set from the
        // parameters, as in the result type (`[type.rules]`).
        let written = match &sig.uses {
            Some(set) => {
                ty_table.lower_effect_set(program, &self.result_scope(Omitted::Empty), set, diags)
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
/// that handler's `init` (E4005).
///
/// A constructor without fields (`None`, `Empty`) is a value, not a function, so its type is the
/// constructed type itself.
fn ctor_signature(
    program: &Program,
    ty_table: &TyTable,
    owner: DefId,
    ctor: DefId,
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
    // Omitted `fails` and `uses` in fields' function types mean empty (spec 9.2). The kinds of
    // type parameters (whether sets) are as the type table decided (`type Handler[E, R]`).
    let params: Vec<TyParam> = match ty_table.con(owner) {
        Some(id) => ty_table.cons[id].params.clone(),
        None => ty_params(program, &defs),
    };
    // The capability sets in the fields are the hidden set type parameters of the type
    // (`[type.rules]`).
    let ty = match fields {
        Fields::None => ret_ty,
        _ if ty_table.con(owner).is_some() => {
            let mut caps = FieldCaps::new(ty_table, owner);
            Ty::func(
                ty_table.lower_fields(program, owner, ctor, &mut caps, diags),
                ret_ty,
            )
        }
        _ => {
            let kinds: Vec<ParamKind> = params.iter().map(|p| p.kind).collect();
            let mut scope = Scope {
                params: &defs,
                kinds: &kinds,
                omitted: Omitted::Empty,
                sets: true,
                self_ty: None,
                result_caps: None,
                quant: None,
            };
            Ty::func(
                field_tys
                    .iter()
                    .map(|&t| ty_table.lower_in(program, &mut scope, t, true, diags))
                    .collect::<Vec<_>>(),
                ret_ty,
            )
        }
    };

    Some(Signature {
        scheme: Scheme::new(params, ty),
        labels,
    })
}

/// The parameters of a handler's init. Their types MUST be written (E3006, `[effect.handler]`);
/// a missing one becomes `Ty::Error`. An omitted E or R in a function type is empty, as in the
/// type of a const. The handler's type parameters (`type_params`) are `Ty::Bound(i)`.
fn init_params(
    program: &Program,
    ty_table: &TyTable,
    handler: DefId,
    type_params: &[DefId],
    params: &[Param],
    diags: &mut Vec<Diagnostic>,
) -> InitParams {
    // A capability set in a parameter type is decided by each item of a with that runs the init
    // (`[effect.handler]`): `Bound(i)`, replaced at the item.
    let mut caps = FieldCaps::finding(None, 0);
    let tys = params
        .iter()
        .map(|p| match p.ty {
            Some(ty) => {
                caps.start_field(program.locals[p.local].name.clone());
                // The set type parameters of a built-in handler (`Background[E, R]`).
                let kinds = ty_table.param_kinds(handler, type_params.len());
                let mut scope = Scope {
                    params: type_params,
                    kinds: &kinds,
                    omitted: Omitted::Fields(&mut caps),
                    sets: kinds.contains(&ParamKind::Set),
                    self_ty: None,
                    result_caps: None,
                    quant: None,
                };
                ty_table.lower_in(program, &mut scope, ty, true, diags)
            }
            None => {
                let local = &program.locals[p.local];
                diags.push(Diagnostic {
                    span: Some(local.span),
                    kind: DiagnosticKind::MissingInitParamType {
                        handler: program.defs[handler].name.clone(),
                        param: local.name.clone(),
                    },
                });
                Ty::Error
            }
        })
        .collect();
    InitParams {
        tys,
        labels: param_labels(program, params),
        caps: caps.found().len(),
    }
}

/// The type of a top-level fn parameter. If none is written, pushes E3006 (spec 6.1) and returns
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

/// `Self` of a trait's functions, constrained by the trait.
pub(crate) fn self_param(program: &Program, trait_def: DefId) -> TyParam {
    TyParam {
        traits: vec![trait_def],
        bounds: vec![program.defs[trait_def].name.clone()],
        ..TyParam::new("Self")
    }
}
