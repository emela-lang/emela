//! Layers (spec 8.5), and the expansion and initialization order of handlers listed in `with`
//! (spec 8.3, 14.2).
//!
//! - A layer is a bundle of handlers. Its contents are handlers or other layers, flattened in
//!   source order. Layers that include each other are E0422; expansion stops at the second
//!   encounter.
//! - `with H1, ..., Hn` is the list of handlers with layers expanded. For handlers of the same
//!   effect the later one wins, and earlier ones are removed (never initialized).
//! - Initialization order follows dependencies ("deps(H)" in spec 9.4). A handler depending on a
//!   handler in the same `with` that provides another effect is initialized after it. Without
//!   cycles, the order is the first one that preserves source order.
//! - Errors inside a layer (two handlers providing the same effect, E0421; dependency cycles,
//!   E0411; mutual inclusion, E0422) are reported once, at the layer declaration
//!   ([`check_layers`]), and not repeated at `with`.

use emela_resolve::hir::{DefId, DefKind, ExprId, Item, Program, Res, Span, TypeId, WithItem};
use la_arena::ArenaMap;
use rustc_hash::{FxHashMap, FxHashSet};
use smol_str::SmolStr;

use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::infer::substitute_bound;
use crate::lower::{Omitted, Scope, TyTable};
use crate::row::{Row, RowElem, Tag};
use crate::signature::{Signatures, is_label};
use crate::ty::{Ty, TyCons};

/// What provides an effect listed in `with`: a handler, or the provider of an effect already in
/// scope (a label in `with AuditLog`, or the right of `by` in `with AuditLog by Console`, issue
/// #255).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Supplier {
    Handler(DefId),
    Effect(Tag),
    /// An item that opens a handler member (`x = Files.Open(p)`, `x = fs.Open(p)`,
    /// `[effect.with]`). It provides nothing by name; `WithBind::effect` is the effect of the
    /// capability it binds.
    Member {
        member: DefId,
        provider: MemberProvider,
        /// The index of the item in the `with`.
        item: usize,
    },
}

/// Whose handler member an item of `with` opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemberProvider {
    /// The innermost provider of the effect (`Files.Open(p)`).
    Effect(Tag),
    /// The handler that provides the capability (`fs.Open(p)`).
    Cap(ExprId),
}

/// Where the arguments of a handler's init are written (`[effect.handler]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgsSource {
    /// The item of the `with` with this index (`ProgramTypes::with_args`).
    Item(usize),
    /// The `member`-th item of `layer` (`ProgramTypes::layer_args`).
    Layer { layer: DefId, member: usize },
}

/// One provider listed in a `with` (or layer).
#[derive(Clone, Debug)]
pub(crate) struct Entry {
    pub(crate) supplier: Supplier,
    /// Where the arguments of the handler's init are written.
    pub(crate) args: ArgsSource,
    /// The effect provided and its type arguments. Effects with different type arguments are
    /// different effects (spec 8.1).
    pub(crate) effect: Tag,
    /// Where it was written in the `with` (layer). For handlers expanded from a layer, where the
    /// layer's name was written.
    pub(crate) span: Span,
    /// Index of the name in the `with` (layer) it came from.
    pub(crate) origin: usize,
    /// Whether that name is a layer.
    pub(crate) from_layer: bool,
    /// Whether it provides its effect by name. Only an unbound item does; an item bound
    /// to a name (`x = H`) is initialized for its name and provides nothing by name
    /// (`[effect.with]`).
    pub(crate) named: bool,
    /// The effects whose requirements the supplier leaves to the outside: every effect a handler
    /// implements (`[effect.handler]`, `[effect.with]`), or `effect` alone. A handler of several
    /// effects has one entry for each, which share this.
    pub(crate) own: Vec<Tag>,
    /// The type arguments of a handler with type parameters (`[effect.with]`): `effect` and
    /// `own` are instantiated with them. Empty otherwise.
    pub(crate) ty_args: Vec<Ty>,
}

/// The type arguments of the handlers listed in a `with` (`[effect.with]`) and in the layers it
/// lists (`[effect.layer]`). Empty for a handler without type parameters.
#[derive(Clone, Debug, Default)]
pub(crate) struct Insts {
    /// Per item of the `with`, those of the handler that supplies it (the right of `by`, or the
    /// item). Missing items have none.
    pub(crate) items: Vec<Vec<Ty>>,
    /// Per layer, per member.
    pub(crate) layers: FxHashMap<DefId, Vec<Vec<Ty>>>,
}

impl Insts {
    /// The type arguments of `items` (per item) and of the members of the layers they list,
    /// nested ones included, looked up with `layer`.
    pub(crate) fn new(
        program: &Program,
        items: &[WithItem],
        item_args: Vec<Vec<Ty>>,
        mut layer: impl FnMut(DefId) -> Option<Vec<Vec<Ty>>>,
    ) -> Self {
        let mut layers = FxHashMap::default();
        let mut todo: Vec<DefId> = items
            .iter()
            .filter_map(|i| match i.res {
                Res::Def(d) if matches!(program.item(d), Some(Item::Layer(_))) => Some(d),
                _ => None,
            })
            .collect();
        while let Some(l) = todo.pop() {
            if layers.contains_key(&l) {
                continue;
            }
            if let Some(Item::Layer(item)) = program.item(l) {
                todo.extend(item.members.iter().filter_map(|m| match m.res {
                    Res::Def(d) if matches!(program.item(d), Some(Item::Layer(_))) => Some(d),
                    _ => None,
                }));
            }
            layers.insert(l, layer(l).unwrap_or_default());
        }
        Insts {
            items: item_args,
            layers,
        }
    }

    fn of(&self, source: ArgsSource) -> &[Ty] {
        match source {
            ArgsSource::Item(i) => self.items.get(i).map_or(&[], Vec::as_slice),
            ArgsSource::Layer { layer, member } => self
                .layers
                .get(&layer)
                .and_then(|m| m.get(member))
                .map_or(&[], Vec::as_slice),
        }
    }
}

/// The effects of `handler` (provides(H), `[type.rules]`) instantiated with `args`. For a handler
/// with type parameters whose type arguments are not known (an error elsewhere), the type
/// parameters become broken types.
pub(crate) fn handler_tags(
    sigs: &Signatures,
    ty_table: &TyTable,
    handler: DefId,
    args: &[Ty],
) -> Vec<Tag> {
    let own = sigs.handler_effects(handler);
    let n = handler_arity(ty_table, handler);
    if n == 0 {
        return own.to_vec();
    }
    let errors;
    let args = if args.len() == n {
        args
    } else {
        errors = vec![Ty::Error; n];
        &errors
    };
    own.iter()
        .map(|t| t.map_args(|a| substitute_bound(a, args)))
        .collect()
}

/// The number of type parameters of a handler (`[effect.handler]`). Zero for an inner handler,
/// whose type arguments are those of its outer handler.
pub(crate) fn handler_arity(ty_table: &TyTable, handler: DefId) -> usize {
    ty_table.handler_params(handler).len()
}

/// A provider of a `with`, as lowering needs it (issue #255).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WithBind {
    /// The effect provided.
    pub effect: Tag,
    pub supplier: Supplier,
    /// The type arguments of a handler with type parameters (`[effect.with]`). Empty otherwise.
    pub ty_args: Vec<Ty>,
    /// Where the arguments of the handler's init are written.
    pub args: ArgsSource,
    /// Whether it provides `effect` by name ([`Entry::named`]). False for a bound item,
    /// including one that opens a handler member.
    pub named: bool,
}

/// The providers of a `with`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WithInfo {
    /// In initialization order (spec 14.2). Layers are expanded, and providers overridden by
    /// later ones in the same `with` are removed.
    pub binds: Vec<WithBind>,
    /// Labels declared `default` that the `with` provides, with the index in `binds` of the
    /// provider of their base. Only labels in the body's R.
    pub defaults: Vec<(Tag, usize)>,
    /// Whether each item of the `with`, in source order, initializes a handler
    /// ([`item_initialized`]). Only the arguments of those items are evaluated
    /// (`[effect.with]`).
    pub initialized: Vec<bool>,
}

/// A `with` whose initialization order and default labels are decided after sets are solved.
pub(crate) struct PendingWith {
    pub(crate) expr: ExprId,
    pub(crate) body: ExprId,
    pub(crate) entries: Vec<Entry>,
    pub(crate) deps: Vec<Row>,
    pub(crate) defaults: Vec<(Tag, usize)>,
    pub(crate) initialized: Vec<bool>,
}

impl Entry {
    pub(crate) fn handler(&self) -> Option<DefId> {
        match self.supplier {
            Supplier::Handler(h) => Some(h),
            Supplier::Effect(_) | Supplier::Member { .. } => None,
        }
    }

    /// The supplier's name in diagnostics.
    fn name(&self, program: &Program) -> SmolStr {
        match &self.supplier {
            Supplier::Handler(h) => program.defs[*h].name.clone(),
            Supplier::Effect(t) => program.defs[t.def].name.clone(),
            Supplier::Member { member, .. } => program.defs[*member].name.clone(),
        }
    }
}

/// A handler in a layer, flattened ([`flatten`]).
pub(crate) struct Flat {
    pub(crate) handler: DefId,
    /// The index of the name in the layer it came from.
    pub(crate) origin: usize,
    /// The layer item that names the handler, which holds the arguments of its init: an item of
    /// a nested layer for a handler that one brings in.
    pub(crate) args: ArgsSource,
}

/// The handlers in a layer, flattened in source order. Stops at the second encounter of layers
/// that include each other.
pub(crate) fn flatten(program: &Program, layer: DefId) -> Vec<Flat> {
    fn go(
        program: &Program,
        layer: DefId,
        visiting: &mut Vec<DefId>,
        out: &mut Vec<(DefId, ArgsSource)>,
    ) -> bool {
        if visiting.contains(&layer) {
            return false;
        }
        let Some(Item::Layer(item)) = program.item(layer) else {
            return true;
        };
        visiting.push(layer);
        for (member, m) in item.members.iter().enumerate() {
            let Res::Def(d) = m.res else {
                continue;
            };
            match program.item(d) {
                Some(Item::Handler(_)) => out.push((d, ArgsSource::Layer { layer, member })),
                Some(Item::Layer(_)) => {
                    go(program, d, visiting, out);
                }
                _ => {}
            }
        }
        visiting.pop();
        true
    }
    let Some(Item::Layer(item)) = program.item(layer) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (i, m) in item.members.iter().enumerate() {
        let Res::Def(d) = m.res else {
            continue;
        };
        match program.item(d) {
            Some(Item::Handler(_)) => out.push(Flat {
                handler: d,
                origin: i,
                args: ArgsSource::Layer { layer, member: i },
            }),
            Some(Item::Layer(_)) => {
                let mut inner = Vec::new();
                go(program, d, &mut vec![layer], &mut inner);
                out.extend(inner.into_iter().map(|(handler, args)| Flat {
                    handler,
                    origin: i,
                    args,
                }));
            }
            _ => {}
        }
    }
    out
}

/// Expands the items listed in `with` into providers. Layers are expanded into their handlers;
/// a label `L` provides its base from `L`; `L by P` provides `L` from `P` (issue #255). Items
/// that do not fit are skipped ([`check_with_items`] reports them). For providers of the same
/// effect, the later one is kept (spec 8.3).
pub(crate) fn with_entries(
    program: &Program,
    sigs: &Signatures,
    ty_table: &TyTable,
    items: &[WithItem],
    insts: &Insts,
) -> Vec<Entry> {
    let mut all = Vec::new();
    for (origin, item) in items.iter().enumerate() {
        let Res::Def(def) = item.res else {
            continue;
        };
        let span = item.span;
        if let Some((by, _)) = item.by {
            let Some(base) = sigs.label_base(def) else {
                continue;
            };
            let ty_args = insts.of(ArgsSource::Item(origin));
            let by = by_tag(
                program,
                sigs,
                ty_table,
                by,
                &item.by_args,
                ty_args,
                &mut Vec::new(),
            );
            let Some(supplier) = by
                .filter(|(_, provided)| provided.contains(base))
                .map(|(s, _)| s)
            else {
                continue;
            };
            let ty_args = match supplier {
                Supplier::Handler(_) => ty_args.to_vec(),
                _ => Vec::new(),
            };
            all.push(Entry {
                supplier,
                args: ArgsSource::Item(origin),
                effect: Tag::new(def),
                span,
                origin,
                from_layer: false,
                named: true,
                own: vec![Tag::new(def)],
                ty_args,
            });
            continue;
        }
        let (members, from_layer) = match program.item(def) {
            Some(Item::Handler(_)) => (vec![(def, ArgsSource::Item(origin))], false),
            Some(Item::Layer(_)) => (
                flatten(program, def)
                    .into_iter()
                    .map(|f| (f.handler, f.args))
                    .collect(),
                true,
            ),
            Some(Item::Effect(_)) => {
                if let Some(base) = sigs.label_base(def) {
                    all.push(Entry {
                        supplier: Supplier::Effect(Tag::new(def)),
                        args: ArgsSource::Item(origin),
                        effect: base.clone(),
                        span,
                        origin,
                        from_layer: false,
                        named: true,
                        own: vec![base.clone()],
                        ty_args: Vec::new(),
                    });
                }
                continue;
            }
            _ => continue,
        };
        // A handler of several effects provides each of them (`[effect.with]`).
        for (handler, args) in members {
            let ty_args = insts.of(args);
            let own = handler_tags(sigs, ty_table, handler, ty_args);
            for effect in &own {
                all.push(Entry {
                    supplier: Supplier::Handler(handler),
                    args,
                    effect: effect.clone(),
                    span,
                    origin,
                    from_layer,
                    named: true,
                    own: own.clone(),
                    ty_args: ty_args.to_vec(),
                });
            }
        }
    }
    keep_last(all, |origin| items[origin].bind.is_some())
}

/// The right of `by` as a supplier, with the effects it provides (with type arguments): a
/// handler provides every effect it implements, one of which serves the label
/// (`[effect.label]`), instantiated with `ty_args` if it has type parameters; an effect `X` (or
/// `X[Int]`) provides itself, or its base if it is a label. `None` for anything else.
fn by_tag(
    program: &Program,
    sigs: &Signatures,
    ty_table: &TyTable,
    by: Res,
    args: &[TypeId],
    ty_args: &[Ty],
    diags: &mut Vec<Diagnostic>,
) -> Option<(Supplier, Vec<Tag>)> {
    let Res::Def(by) = by else {
        return None;
    };
    if !sigs.handler_effects(by).is_empty() {
        let effects = handler_tags(sigs, ty_table, by, ty_args);
        return Some((Supplier::Handler(by), effects));
    }
    if program.defs[by].kind != DefKind::Effect {
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
    let tag = ty_table.effect_tag(program, &scope, by, args, None, None, diags)?;
    let provided = sigs.base_of(&tag).clone();
    Some((Supplier::Effect(tag), vec![provided]))
}

/// Reports the items in `with` that do not fit (issue #255): an effect that is not a label
/// listed alone or left of `by` (E0430), a provider of another effect right of `by` (E0431), and
/// the same label given a provider twice (E0432).
pub(crate) fn check_with_items(
    program: &Program,
    sigs: &Signatures,
    ty_table: &TyTable,
    items: &[WithItem],
    insts: &Insts,
    diags: &mut Vec<Diagnostic>,
) {
    let name = |def: DefId| program.defs[def].name.clone();
    let mut plugged: Vec<DefId> = Vec::new();
    for (origin, item) in items.iter().enumerate() {
        let Res::Def(def) = item.res else {
            continue;
        };
        if program.defs[def].kind != DefKind::Effect {
            continue;
        }
        // A label whose base cannot be used is reported at its declaration (E0429).
        if !is_label(program, def) {
            diags.push(Diagnostic {
                span: Some(item.span),
                kind: DiagnosticKind::NotALabel {
                    name: name(def),
                    by: item.by.is_some(),
                },
            });
            continue;
        }
        if let Some(local) = item.bind
            && let Some(provided) = unmade_label_item(program, sigs, ty_table, item, insts, origin)
        {
            let item_text = match item.by {
                Some((Res::Def(by), _)) => format!("{} by {}", name(def), name(by)),
                _ => name(def).to_string(),
            };
            diags.push(Diagnostic {
                span: Some(item.span),
                kind: DiagnosticKind::BoundLabelItem {
                    item: SmolStr::new(item_text),
                    name: program.locals[local].name.clone(),
                    effect: effect_name(&ty_table.cons, &provided),
                },
            });
        }
        let Some((by, by_span)) = item.by else {
            continue;
        };
        // Only the unbound items provide the label by name, so only they can provide it
        // twice (`[effect.label]`).
        if item.bind.is_none() {
            if plugged.contains(&def) {
                diags.push(Diagnostic {
                    span: Some(item.span),
                    kind: DiagnosticKind::LabelProvidedTwice { label: name(def) },
                });
            }
            plugged.push(def);
        }
        let Some(base) = sigs.label_base(def) else {
            continue;
        };
        let mut arg_diags = Vec::new();
        let by_res = by;
        let Some((_, provided)) = by_tag(
            program,
            sigs,
            ty_table,
            by,
            &item.by_args,
            insts.of(ArgsSource::Item(origin)),
            &mut arg_diags,
        ) else {
            continue;
        };
        // Missing or wrong type arguments are reported once (E0423, E0304).
        if !arg_diags.is_empty() {
            for mut d in arg_diags {
                d.span = Some(by_span);
                diags.push(d);
            }
            continue;
        }
        if !provided.contains(base) {
            let Res::Def(by) = by_res else { continue };
            let provided: Vec<SmolStr> = provided
                .iter()
                .map(|t| effect_name(&ty_table.cons, t))
                .collect();
            diags.push(Diagnostic {
                span: Some(by_span),
                kind: DiagnosticKind::LabelMismatch {
                    label: name(def),
                    base: effect_name(&ty_table.cons, base),
                    provider: name(by),
                    provided: SmolStr::new(provided.join(" + ")),
                },
            });
        }
    }
}

/// For a label item that makes no handler (`L`, or `L by P` where P is an effect or a label),
/// the effect it would provide by name if it had no name: L for `L by P`, the underlying effect
/// for `L`. Such an item cannot be bound to a name (E0447, `[effect.with]`). `None` for an item
/// whose right of `by` is a handler, or does not resolve.
fn unmade_label_item(
    program: &Program,
    sigs: &Signatures,
    ty_table: &TyTable,
    item: &WithItem,
    insts: &Insts,
    origin: usize,
) -> Option<Tag> {
    let Res::Def(def) = item.res else {
        return None;
    };
    let Some((by, _)) = item.by else {
        return sigs.label_base(def).cloned();
    };
    let (supplier, _) = by_tag(
        program,
        sigs,
        ty_table,
        by,
        &item.by_args,
        insts.of(ArgsSource::Item(origin)),
        &mut Vec::new(),
    )?;
    matches!(supplier, Supplier::Effect(_)).then(|| Tag::new(def))
}

/// Whether the `origin`-th item of a `with` initializes a handler: it names one, and no later
/// item overrides it. The arguments of other items are not evaluated, and their E and R are not
/// those of the `with` (`[effect.with]`).
pub(crate) fn item_initialized(entries: &[Entry], origin: usize) -> bool {
    entries
        .iter()
        .any(|e| e.origin == origin && !e.from_layer && e.handler().is_some())
}

/// Whether an item of `with` opens a handler member (`x = Files.Open(p)`, `x = fs.Open(p)`,
/// `[effect.with]`). It is not overridden by other items, and always evaluates its arguments.
pub(crate) fn is_member_item(program: &Program, item: &WithItem) -> bool {
    item.cap.is_some()
        || matches!(item.res, Res::Def(def) if program.defs[def].kind == DefKind::Member)
}

/// Whether the arguments of the `origin`-th item of a `with` are evaluated: it initializes a
/// handler ([`item_initialized`]) or opens a handler member.
pub(crate) fn item_evaluated(
    program: &Program,
    entries: &[Entry],
    items: &[WithItem],
    origin: usize,
) -> bool {
    item_initialized(entries, origin) || is_member_item(program, &items[origin])
}

/// The arguments written on an item of a `with`, after the item and after the right of `by`.
pub(crate) fn item_args(item: &WithItem) -> impl Iterator<Item = ExprId> + '_ {
    [&item.args, &item.by_init_args]
        .into_iter()
        .flatten()
        .flat_map(|a| a.args.iter().map(|arg| arg.value))
}

/// The arguments written on the items of a `with` that are evaluated: those of the items that
/// initialize a handler ([`item_initialized`]), in source order.
pub(crate) fn evaluated_args(
    program: &Program,
    sigs: &Signatures,
    ty_table: &TyTable,
    items: &[WithItem],
    insts: &Insts,
) -> Vec<ExprId> {
    let entries = with_entries(program, sigs, ty_table, items, insts);
    items
        .iter()
        .enumerate()
        .filter(|&(i, _)| item_evaluated(program, &entries, items, i))
        .flat_map(|(_, item)| item_args(item))
        .collect()
}

/// For handlers of the same effect, only the later unbound item is kept. An item bound
/// to a name (`bound`) is always kept for its name, but provides nothing by name
/// ([`Entry::named`], `[effect.with]`): it is never overridden, and it does not override an
/// earlier item.
fn keep_last(entries: Vec<Entry>, bound: impl Fn(usize) -> bool) -> Vec<Entry> {
    let by_name = |e: &Entry| e.from_layer || !bound(e.origin);
    let mut out: Vec<Entry> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        if !by_name(e) {
            out.push(Entry {
                named: false,
                ..e.clone()
            });
            continue;
        }
        let overridden = entries[i + 1..]
            .iter()
            .any(|later| by_name(later) && later.effect == e.effect);
        if !overridden {
            out.push(e.clone());
        }
    }
    out
}

/// `i` depends on `j`: `i`'s dependencies include `j`'s effect, and that is not one of `i`'s own
/// effects ([`Entry::own`]).
fn depends(entries: &[Entry], deps: &[Row], i: usize, j: usize) -> bool {
    j != i
        && entries[j].named
        && !entries[i].own.contains(&entries[j].effect)
        && deps[i].contains(&RowElem::Tag(entries[j].effect.clone()))
}

/// Initialization order, as indices into `entries`; dependencies come first. On a dependency
/// cycle, returns the indices along the cycle as `Err`.
pub(crate) fn init_order(entries: &[Entry], deps: &[Row]) -> Result<Vec<usize>, Vec<usize>> {
    if let Some(cycle) = find_cycle(entries.len(), |i| {
        (0..entries.len())
            .filter(|&j| depends(entries, deps, i, j))
            .collect()
    }) {
        return Err(cycle);
    }
    // Among those whose dependencies are all placed, pick the first in source order.
    let mut done = vec![false; entries.len()];
    let mut order = Vec::new();
    while order.len() < entries.len() {
        let next = (0..entries.len())
            .find(|&i| {
                !done[i] && (0..entries.len()).all(|j| done[j] || !depends(entries, deps, i, j))
            })
            .expect("acyclic, so an order exists");
        done[next] = true;
        order.push(next);
    }
    Ok(order)
}

/// The dependency-cycle diagnostic (E0411).
pub(crate) fn cycle_diagnostic(
    program: &Program,
    entries: &[Entry],
    cycle: &[usize],
) -> Diagnostic {
    let mut names: Vec<SmolStr> = cycle.iter().map(|&i| entries[i].name(program)).collect();
    names.push(names[0].clone());
    Diagnostic {
        span: Some(entries[cycle[0]].span),
        kind: DiagnosticKind::HandlerCycle { cycle: names },
    }
}

/// An expanded layer.
#[derive(Clone, Debug, Default)]
pub struct LayerInfo {
    /// Handlers in initialization order. Release runs in reverse.
    pub handlers: Vec<DefId>,
    /// Dependencies the layer leaves unmet. They remain in the R of `with` (spec 8.5).
    pub uses: Row,
}

/// Checks layer declarations and returns their expansions. `handler_uses` holds the dependencies
/// of handlers already inferred, and `layer_ty_args` the type arguments of the members of each
/// layer (`[effect.layer]`).
pub(crate) fn check_layers(
    program: &Program,
    sigs: &Signatures,
    ty_table: &TyTable,
    handler_uses: &ArenaMap<DefId, Row>,
    layer_ty_args: &ArenaMap<DefId, Vec<Vec<Ty>>>,
    diags: &mut Vec<Diagnostic>,
) -> ArenaMap<DefId, LayerInfo> {
    let cons = &ty_table.cons;
    let mut out = ArenaMap::default();
    let mut reported_cycles: FxHashSet<Vec<DefId>> = FxHashSet::default();
    for (layer, data) in program.defs.iter() {
        let Some(Item::Layer(item)) = program.item(layer) else {
            continue;
        };
        // Layers that include each other (E0422). Each cycle is reported once.
        if let Some(cycle) = layer_cycle(program, layer) {
            let mut key = cycle.clone();
            key.sort_by_key(|d| d.into_raw());
            if reported_cycles.insert(key) {
                let mut names: Vec<SmolStr> = cycle
                    .iter()
                    .map(|&l| program.defs[l].name.clone())
                    .collect();
                names.push(names[0].clone());
                diags.push(Diagnostic {
                    span: data.span,
                    kind: DiagnosticKind::LayerCycle { cycle: names },
                });
            }
        }

        // A layer has no names to bind, so it cannot open a handler member (`[effect.with]`).
        for m in &item.members {
            if let Res::Def(def) = m.res
                && program.defs[def].kind == DefKind::Member
            {
                diags.push(Diagnostic {
                    span: Some(m.span),
                    kind: DiagnosticKind::MemberInLayer {
                        member: program.defs[def].name.clone(),
                        layer: data.name.clone(),
                    },
                });
            }
        }
        let flat = flatten(program, layer);
        let mut entries: Vec<Entry> = Vec::new();
        for Flat {
            handler,
            origin,
            args,
        } in flat
        {
            let ty_args = match args {
                ArgsSource::Layer { layer, member } => layer_ty_args
                    .get(layer)
                    .and_then(|m| m.get(member))
                    .map_or(&[][..], Vec::as_slice),
                ArgsSource::Item(_) => &[],
            };
            let own = handler_tags(sigs, ty_table, handler, ty_args);
            // A handler of several effects counts as providing each of them (E0421,
            // `[effect.layer]`).
            for effect in own.iter().cloned() {
                let member = item.members[origin].res;
                let span = item.members[origin].span;
                let from_layer = matches!(member, Res::Def(m) if matches!(program.item(m), Some(Item::Layer(_))));
                // Two handlers for the same effect (E0421). A duplicate only inside a nested layer is
                // reported at that layer.
                if let Some(first) = entries.iter().find(|e| e.effect == effect) {
                    if !(from_layer && first.from_layer && first.origin == origin) {
                        diags.push(Diagnostic {
                            span: Some(span),
                            kind: DiagnosticKind::DuplicateLayerEffect {
                                layer: data.name.clone(),
                                effect: effect_name(cons, &effect),
                                first: first.name(program),
                                second: program.defs[handler].name.clone(),
                            },
                        });
                    }
                    continue;
                }
                entries.push(Entry {
                    supplier: Supplier::Handler(handler),
                    args,
                    effect,
                    span,
                    origin,
                    from_layer,
                    named: true,
                    own: own.clone(),
                    ty_args: ty_args.to_vec(),
                });
            }
        }

        let deps: Vec<Row> = entries
            .iter()
            .map(|e| {
                e.handler()
                    .and_then(|h| handler_uses.get(h))
                    .map(|r| instantiate_row(r, &e.ty_args))
                    .unwrap_or_default()
            })
            .collect();
        let order = match init_order(&entries, &deps) {
            Ok(order) => order,
            Err(cycle) => {
                // A cycle only inside a nested layer is reported at that layer.
                if !same_inner_layer(&entries, &cycle) {
                    diags.push(cycle_diagnostic(program, &entries, &cycle));
                }
                (0..entries.len()).collect()
            }
        };
        let provided: Vec<Tag> = entries.iter().map(|e| e.effect.clone()).collect();
        let mut uses = Row::empty();
        for (e, d) in entries.iter().zip(&deps) {
            // Dependencies on the handler's own effect are met from outside (spec 8.3).
            for elem in d.elems() {
                match elem {
                    RowElem::Tag(t) if !e.own.contains(t) && provided.contains(t) => {}
                    elem => uses.insert(elem.clone()),
                }
            }
        }
        out.insert(
            layer,
            LayerInfo {
                // A handler of several effects has an entry for each; it is initialized once,
                // where its first entry is placed.
                handlers: {
                    let mut hs: Vec<DefId> = Vec::new();
                    for h in order.iter().filter_map(|&i| entries[i].handler()) {
                        if !hs.contains(&h) {
                            hs.push(h);
                        }
                    }
                    hs
                },
                uses,
            },
        );
    }
    out
}

/// requires(H) of a handler with type parameters, instantiated with the type arguments of an
/// item (`[type.rules]`). As is without them.
pub(crate) fn instantiate_row(row: &Row, ty_args: &[Ty]) -> Row {
    if ty_args.is_empty() {
        return row.clone();
    }
    crate::infer::substitute_row(row, ty_args, &[])
}

/// The effect name for diagnostics; `State[Int]` if it has type arguments.
fn effect_name(cons: &TyCons, effect: &Tag) -> SmolStr {
    Row::of_tag(effect.clone())
        .display_effects(cons, &[])
        .to_string()
        .into()
}

/// Whether every handler along the cycle came from the same nested layer (already reported
/// there).
pub(crate) fn same_inner_layer(entries: &[Entry], cycle: &[usize]) -> bool {
    let first = &entries[cycle[0]];
    first.from_layer && cycle.iter().all(|&i| entries[i].origin == first.origin)
}

/// A cycle in the layer inclusion relation leading from `layer` back to itself.
fn layer_cycle(program: &Program, layer: DefId) -> Option<Vec<DefId>> {
    fn go(program: &Program, target: DefId, at: DefId, path: &mut Vec<DefId>) -> bool {
        let Some(Item::Layer(item)) = program.item(at) else {
            return false;
        };
        for member in &item.members {
            let Res::Def(m) = member.res else {
                continue;
            };
            if !matches!(program.item(m), Some(Item::Layer(_))) {
                continue;
            }
            if m == target {
                return true;
            }
            if path.contains(&m) {
                continue;
            }
            path.push(m);
            if go(program, target, m, path) {
                return true;
            }
            path.pop();
        }
        false
    }
    let mut path = vec![layer];
    go(program, layer, layer, &mut path).then_some(path)
}

/// Finds one cycle in a directed graph, as the vertices along it (the first vertex appears once).
pub(crate) fn find_cycle(n: usize, edges: impl Fn(usize) -> Vec<usize>) -> Option<Vec<usize>> {
    // 0: unvisited, 1: in progress, 2: done.
    fn visit(
        v: usize,
        edges: &dyn Fn(usize) -> Vec<usize>,
        state: &mut [u8],
        path: &mut Vec<usize>,
    ) -> Option<Vec<usize>> {
        state[v] = 1;
        path.push(v);
        for w in edges(v) {
            if state[w] == 1 {
                let start = path
                    .iter()
                    .position(|&p| p == w)
                    .expect("an in-progress vertex is on the path");
                return Some(path[start..].to_vec());
            }
            if state[w] == 0
                && let Some(cycle) = visit(w, edges, state, path)
            {
                return Some(cycle);
            }
        }
        path.pop();
        state[v] = 2;
        None
    }
    let mut state = vec![0u8; n];
    for v in 0..n {
        if state[v] == 0
            && let Some(cycle) = visit(v, &edges, &mut state, &mut Vec::new())
        {
            return Some(cycle);
        }
    }
    None
}
