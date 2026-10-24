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
use rustc_hash::FxHashSet;
use smol_str::SmolStr;

use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::lower::{Omitted, Scope, TyTable};
use crate::row::{Row, RowElem, Tag};
use crate::signature::{Signatures, is_label};
use crate::ty::TyCons;

/// What provides an effect listed in `with`: a handler, or the provider of an effect already in
/// scope (a label in `with AuditLog`, or the right of `by` in `with AuditLog by Console`, issue
/// #255).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Supplier {
    Handler(DefId),
    Effect(Tag),
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
}

/// A provider of a `with`, as lowering needs it (issue #255).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WithBind {
    /// The effect provided.
    pub effect: Tag,
    pub supplier: Supplier,
    /// Where the arguments of the handler's init are written.
    pub args: ArgsSource,
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
            Supplier::Effect(_) => None,
        }
    }

    /// The supplier's name in diagnostics.
    fn name(&self, program: &Program) -> SmolStr {
        match &self.supplier {
            Supplier::Handler(h) => program.defs[*h].name.clone(),
            Supplier::Effect(t) => program.defs[t.def].name.clone(),
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

/// The effect a handler implements, with its type arguments. `None` for Async handlers (E0413)
/// and handlers whose effect is unknown.
pub(crate) fn handler_effect(sigs: &Signatures, handler: DefId) -> Option<Tag> {
    sigs.handler_effect(handler).cloned()
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
            let by = by_tag(program, sigs, ty_table, by, &item.by_args, &mut Vec::new());
            let Some(supplier) = by.filter(|(_, provided)| provided == base).map(|(s, _)| s) else {
                continue;
            };
            all.push(Entry {
                supplier,
                args: ArgsSource::Item(origin),
                effect: Tag::new(def),
                span,
                origin,
                from_layer: false,
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
                    });
                }
                continue;
            }
            _ => continue,
        };
        for (handler, args) in members {
            let Some(effect) = handler_effect(sigs, handler) else {
                continue;
            };
            all.push(Entry {
                supplier: Supplier::Handler(handler),
                args,
                effect,
                span,
                origin,
                from_layer,
            });
        }
    }
    keep_last(all)
}

/// The right of `by` as a supplier, with the effect it provides (with type arguments): a
/// handler provides its effect; an effect `X` (or `X[Int]`) provides itself, or its base if it
/// is a label. `None` for anything else.
fn by_tag(
    program: &Program,
    sigs: &Signatures,
    ty_table: &TyTable,
    by: Res,
    args: &[TypeId],
    diags: &mut Vec<Diagnostic>,
) -> Option<(Supplier, Tag)> {
    let Res::Def(by) = by else {
        return None;
    };
    if let Some(effect) = sigs.handler_effect(by) {
        return Some((Supplier::Handler(by), effect.clone()));
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
    };
    let tag = ty_table.effect_tag(program, &scope, by, args, None, None, diags)?;
    let provided = sigs.base_of(&tag).clone();
    Some((Supplier::Effect(tag), provided))
}

/// Reports the items in `with` that do not fit (issue #255): an effect that is not a label
/// listed alone or left of `by` (E0430), a provider of another effect right of `by` (E0431), and
/// the same label given a provider twice (E0432).
pub(crate) fn check_with_items(
    program: &Program,
    sigs: &Signatures,
    ty_table: &TyTable,
    items: &[WithItem],
    diags: &mut Vec<Diagnostic>,
) {
    let name = |def: DefId| program.defs[def].name.clone();
    let mut plugged: Vec<DefId> = Vec::new();
    for item in items {
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
        let Some((by, by_span)) = item.by else {
            continue;
        };
        if plugged.contains(&def) {
            diags.push(Diagnostic {
                span: Some(item.span),
                kind: DiagnosticKind::LabelProvidedTwice { label: name(def) },
            });
        }
        plugged.push(def);
        let Some(base) = sigs.label_base(def) else {
            continue;
        };
        let mut arg_diags = Vec::new();
        let by_res = by;
        let Some((_, provided)) =
            by_tag(program, sigs, ty_table, by, &item.by_args, &mut arg_diags)
        else {
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
        if provided != *base {
            let Res::Def(by) = by_res else { continue };
            diags.push(Diagnostic {
                span: Some(by_span),
                kind: DiagnosticKind::LabelMismatch {
                    label: name(def),
                    base: effect_name(&ty_table.cons, base),
                    provider: name(by),
                    provided: effect_name(&ty_table.cons, &provided),
                },
            });
        }
    }
}

/// Whether the `origin`-th item of a `with` initializes a handler: it names one, and no later
/// item overrides it. The arguments of other items are not evaluated, and their E and R are not
/// those of the `with` (`[effect.with]`).
pub(crate) fn item_initialized(entries: &[Entry], origin: usize) -> bool {
    entries
        .iter()
        .any(|e| e.origin == origin && !e.from_layer && e.handler().is_some())
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
) -> Vec<ExprId> {
    let entries = with_entries(program, sigs, ty_table, items);
    items
        .iter()
        .enumerate()
        .filter(|&(i, _)| item_initialized(&entries, i))
        .flat_map(|(_, item)| item_args(item))
        .collect()
}

/// For handlers of the same effect, only the later one is kept.
fn keep_last(entries: Vec<Entry>) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        if !entries[i + 1..]
            .iter()
            .any(|later| later.effect == e.effect)
        {
            out.push(e.clone());
        }
    }
    out
}

/// `i` depends on `j`: `i`'s dependencies include `j`'s effect, and `j` provides a different
/// effect.
fn depends(entries: &[Entry], deps: &[Row], i: usize, j: usize) -> bool {
    j != i
        && entries[j].effect != entries[i].effect
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
/// of handlers already inferred.
pub(crate) fn check_layers(
    program: &Program,
    sigs: &Signatures,
    cons: &TyCons,
    handler_uses: &ArenaMap<DefId, Row>,
    diags: &mut Vec<Diagnostic>,
) -> ArenaMap<DefId, LayerInfo> {
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

        let flat = flatten(program, layer);
        let mut entries: Vec<Entry> = Vec::new();
        for Flat {
            handler,
            origin,
            args,
        } in flat
        {
            let Some(effect) = handler_effect(sigs, handler) else {
                continue;
            };
            let member = item.members[origin].res;
            let span = item.members[origin].span;
            let from_layer =
                matches!(member, Res::Def(m) if matches!(program.item(m), Some(Item::Layer(_))));
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
            });
        }

        let deps: Vec<Row> = entries
            .iter()
            .map(|e| {
                e.handler()
                    .and_then(|h| handler_uses.get(h).cloned())
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
                    RowElem::Tag(t) if *t != e.effect && provided.contains(t) => {}
                    elem => uses.insert(elem.clone()),
                }
            }
        }
        out.insert(
            layer,
            LayerInfo {
                handlers: order.iter().filter_map(|&i| entries[i].handler()).collect(),
                uses,
            },
        );
    }
    out
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
