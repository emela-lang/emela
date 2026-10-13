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

use emela_resolve::hir::{DefId, Item, Program, Res, Span};
use la_arena::ArenaMap;
use rustc_hash::FxHashSet;
use smol_str::SmolStr;

use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::row::{Row, RowElem, Tag};
use crate::signature::Signatures;
use crate::ty::TyCons;

/// One handler listed in a `with` (or layer).
#[derive(Clone, Debug)]
pub(crate) struct Entry {
    pub(crate) handler: DefId,
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

/// The handlers in a layer, flattened in source order, as (handler, index of the name in the
/// layer it came from). Stops at the second encounter of layers that include each other.
pub(crate) fn flatten(program: &Program, layer: DefId) -> Vec<(DefId, usize)> {
    fn go(
        program: &Program,
        layer: DefId,
        visiting: &mut Vec<DefId>,
        out: &mut Vec<DefId>,
    ) -> bool {
        if visiting.contains(&layer) {
            return false;
        }
        let Some(Item::Layer(item)) = program.item(layer) else {
            return true;
        };
        visiting.push(layer);
        for &(res, _) in &item.members {
            let Res::Def(m) = res else {
                continue;
            };
            match program.item(m) {
                Some(Item::Handler(_)) => out.push(m),
                Some(Item::Layer(_)) => {
                    go(program, m, visiting, out);
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
    for (i, &(res, _)) in item.members.iter().enumerate() {
        let Res::Def(m) = res else {
            continue;
        };
        match program.item(m) {
            Some(Item::Handler(_)) => out.push((m, i)),
            Some(Item::Layer(_)) => {
                let mut inner = Vec::new();
                go(program, m, &mut vec![layer], &mut inner);
                out.extend(inner.into_iter().map(|h| (h, i)));
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

/// Expands the names (handlers or layers) listed in `with` into handlers. For handlers of the
/// same effect, the later one is kept (spec 8.3).
pub(crate) fn with_entries(
    program: &Program,
    sigs: &Signatures,
    handlers: &[(Res, Span)],
) -> Vec<Entry> {
    let mut all = Vec::new();
    for (origin, &(res, span)) in handlers.iter().enumerate() {
        let Res::Def(def) = res else {
            continue;
        };
        let (members, from_layer) = match program.item(def) {
            Some(Item::Handler(_)) => (vec![def], false),
            Some(Item::Layer(_)) => (
                flatten(program, def).into_iter().map(|(h, _)| h).collect(),
                true,
            ),
            _ => continue,
        };
        for handler in members {
            let Some(effect) = handler_effect(sigs, handler) else {
                continue;
            };
            all.push(Entry {
                handler,
                effect,
                span,
                origin,
                from_layer,
            });
        }
    }
    keep_last(all)
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
    let mut names: Vec<SmolStr> = cycle
        .iter()
        .map(|&i| program.defs[entries[i].handler].name.clone())
        .collect();
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
        for (handler, origin) in flat {
            let Some(effect) = handler_effect(sigs, handler) else {
                continue;
            };
            let (member, span) = item.members[origin];
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
                            first: program.defs[first.handler].name.clone(),
                            second: program.defs[handler].name.clone(),
                        },
                    });
                }
                continue;
            }
            entries.push(Entry {
                handler,
                effect,
                span,
                origin,
                from_layer,
            });
        }

        let deps: Vec<Row> = entries
            .iter()
            .map(|e| handler_uses.get(e.handler).cloned().unwrap_or_default())
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
                handlers: order.iter().map(|&i| entries[i].handler).collect(),
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
        for &(res, _) in &item.members {
            let Res::Def(m) = res else {
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
