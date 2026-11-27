//! Strongly connected components of a graph of definitions (Tarjan).

use emela_resolve::hir::DefId;
use rustc_hash::FxHashMap;

/// The strongly connected components of the graph, each sorted in definition order. A component
/// comes after every component reachable from it.
pub(crate) fn strongly_connected(
    nodes: &[DefId],
    edges: &FxHashMap<DefId, Vec<DefId>>,
) -> Vec<Vec<DefId>> {
    let mut tarjan = Tarjan {
        edges,
        index: FxHashMap::default(),
        low: FxHashMap::default(),
        on_stack: FxHashMap::default(),
        stack: Vec::new(),
        next: 0,
        groups: Vec::new(),
    };
    for &def in nodes {
        if !tarjan.index.contains_key(&def) {
            tarjan.visit(def);
        }
    }
    tarjan.groups
}

/// Tarjan's strongly connected components. A component comes after every component reachable
/// from it (callees first).
struct Tarjan<'g> {
    edges: &'g FxHashMap<DefId, Vec<DefId>>,
    index: FxHashMap<DefId, usize>,
    low: FxHashMap<DefId, usize>,
    on_stack: FxHashMap<DefId, bool>,
    stack: Vec<DefId>,
    next: usize,
    groups: Vec<Vec<DefId>>,
}

impl Tarjan<'_> {
    fn visit(&mut self, v: DefId) {
        self.index.insert(v, self.next);
        self.low.insert(v, self.next);
        self.next += 1;
        self.stack.push(v);
        self.on_stack.insert(v, true);

        let edges = self.edges;
        for &w in edges.get(&v).map(Vec::as_slice).unwrap_or(&[]) {
            if !self.index.contains_key(&w) {
                self.visit(w);
                let low = self.low[&v].min(self.low[&w]);
                self.low.insert(v, low);
            } else if self.on_stack.get(&w).copied().unwrap_or(false) {
                let low = self.low[&v].min(self.index[&w]);
                self.low.insert(v, low);
            }
        }

        if self.low[&v] == self.index[&v] {
            let mut group = Vec::new();
            loop {
                let w = self.stack.pop().expect("v is on the stack");
                self.on_stack.insert(w, false);
                group.push(w);
                if w == v {
                    break;
                }
            }
            // Within a group, infer in definition order so diagnostics come in a stable order.
            group.sort_by_key(|def| def.into_raw());
            self.groups.push(group);
        }
    }
}
