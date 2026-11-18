//! The call graph and its strongly connected components.
//!
//! # Contract
//!
//! ```text
//! Analysis ID: AnalysisId::CallGraph
//! Domain / lattice: a graph whose nodes are functions and whose edges are calls, plus the list of
//!                   its strongly connected components. `Unknown` is the top element; an empty
//!                   graph is the bottom and says there are no calls.
//! Transfer: a direct call adds an edge to its callee; an indirect call, a call through a handler
//!           operation and a call of an external function are recorded as "unknown callee" so a
//!           consumer does not treat the graph as complete when it is not.
//! Soundness: every call the analysis reports exists on some path, and a component reports a cycle
//!            only when the calls in it can form one.
//! Deopt (unknown): a consumer that gets `Unknown` does not assume a function is recursive or
//!                  non-recursive, and does not inline a call whose callee it could not place.
//! Consumers: δ reduction (recursion and the cost model), CSE (a call is pure only when the callee
//!            is pure), devirtualisation and the SCC-based passes.
//! ```
//!
//! # Nodes and edges
//!
//! A node is a top-level function ([`FnId`]). An edge is a place where the caller mentions the
//! callee:
//!
//! - a [`Callee::Direct`] call, and
//! - a function value ([`Expr::Fn`]), which mentions the function without calling it yet.
//!
//! A [`Expr::Lambda`] is a nested function with no [`FnId`], so it is not a node; the lambda's
//! body still contributes the calls it makes from the enclosing function. Indirect calls,
//! operation calls and external functions have no node to point at, so this graph is a
//! *may-call* over the calls whose callee is a direct function; it is not a complete
//! interprocedural call graph. A consumer that needs completeness checks the expression for an
//! indirect call and deopts (see the contract above).
//!
//! # Determinism
//!
//! [`CallGraph::of_module`] lists every function of the module in `callees`, in module order, and
//! sorts and deduplicates each adjacency list by [`FnId`]. [`CallGraph::sccs`] visits nodes in
//! [`FnId`] order (which is `m.functions` order) and adjacency lists in their stored order, so two
//! runs over the same module produce the same vectors.
//!
//! # Status
//!
//! B2/PR-021 implements [`CallGraph::of_module`], [`CallGraph::sccs`],
//! [`CallGraph::is_recursive`] and [`CallGraph::reachable_from`]. The [`Analysis`] hook
//! [`analyze`] stays conservative for now: the cache is not wired into a pass yet, and the
//! concrete methods are what the δ and devirtualisation rules call.

use rustc_hash::{FxHashMap, FxHashSet};

use crate::ir::{Callee, Expr, FnId, Module};

use super::{Analysis, AnalysisId, Fact};

/// The calls of a module, by caller and by callee.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallGraph {
    /// For each caller, the functions it calls directly or mentions as a value, sorted and
    /// deduplicated. Every function of the module has an entry, possibly empty.
    pub callees: FxHashMap<FnId, Vec<FnId>>,
    /// For each callee, the callers that mention it, sorted and deduplicated.
    pub callers: FxHashMap<FnId, Vec<FnId>>,
}

impl CallGraph {
    /// The call graph of `m`.
    pub fn of_module(m: &Module) -> Self {
        let mut callees = FxHashMap::default();
        for (f, function) in m.functions.iter() {
            let mut targets = Vec::new();
            collect_calls(&function.body, &mut targets);
            sort_and_dedup(&mut targets);
            callees.insert(f, targets);
        }

        let mut callers: FxHashMap<FnId, Vec<FnId>> = FxHashMap::default();
        for (caller, _) in m.functions.iter() {
            for &callee in &callees[&caller] {
                callers.entry(callee).or_default().push(caller);
            }
        }
        for callers in callers.values_mut() {
            sort_and_dedup(callers);
        }

        Self { callees, callers }
    }

    /// The strongly connected components of the graph, each sorted by [`FnId`].
    ///
    /// A component of one function without a self-edge is a function that is not recursive. The
    /// order of the components follows the iterative Tarjan walk from the lowest node: a component
    /// is emitted when its root finishes, so a callee's component comes before its caller's (a
    /// reverse topological order). The walk keeps its own stack, so a long call chain cannot
    /// overflow the process stack.
    pub fn sccs(&self) -> Vec<Vec<FnId>> {
        // All nodes: every caller and every callee. A value in `callees` that is not a key (a
        // caller from another module or a hand-built graph) is still a node, so the walk never
        // indexes a missing entry.
        let mut nodes: Vec<FnId> = self
            .callees
            .keys()
            .chain(self.callees.values().flatten())
            .chain(self.callers.keys())
            .chain(self.callers.values().flatten())
            .copied()
            .collect();
        sort_and_dedup(&mut nodes);

        let index_of: FxHashMap<FnId, usize> = nodes
            .iter()
            .enumerate()
            .map(|(index, &f)| (f, index))
            .collect();
        let adjacency: Vec<Vec<usize>> = nodes
            .iter()
            .map(|f| {
                self.callees
                    .get(f)
                    .map(|targets| targets.iter().map(|t| index_of[t]).collect())
                    .unwrap_or_default()
            })
            .collect();

        let n = nodes.len();
        let mut index = vec![usize::MAX; n];
        let mut low = vec![0usize; n];
        let mut on_stack = vec![false; n];
        let mut next_index = 0usize;
        let mut stack: Vec<usize> = Vec::new();
        let mut components: Vec<Vec<FnId>> = Vec::new();

        for start in 0..n {
            if index[start] != usize::MAX {
                continue;
            }
            index[start] = next_index;
            low[start] = next_index;
            next_index += 1;
            stack.push(start);
            on_stack[start] = true;

            // (node, next edge to visit). An explicit stack replaces recursion, so a chain of
            // thousands of functions is a large vector, not a deep call stack.
            let mut frames: Vec<(usize, usize)> = vec![(start, 0)];
            while let Some(&(v, edge)) = frames.last() {
                if edge < adjacency[v].len() {
                    // Read the frame first, then advance it, so no mutable borrow of `frames`
                    // is held while the child frame is pushed.
                    let w = adjacency[v][edge];
                    frames.last_mut().expect("the frame is on the stack").1 += 1;
                    if index[w] == usize::MAX {
                        index[w] = next_index;
                        low[w] = next_index;
                        next_index += 1;
                        stack.push(w);
                        on_stack[w] = true;
                        frames.push((w, 0));
                    } else if on_stack[w] {
                        low[v] = low[v].min(index[w]);
                    }
                } else {
                    frames.pop();
                    if let Some(&(parent, _)) = frames.last() {
                        low[parent] = low[parent].min(low[v]);
                    }
                    if low[v] == index[v] {
                        let mut component = Vec::new();
                        loop {
                            let w = stack.pop().expect("the Tarjan stack holds the component");
                            on_stack[w] = false;
                            component.push(nodes[w]);
                            if w == v {
                                break;
                            }
                        }
                        sort_and_dedup(&mut component);
                        components.push(component);
                    }
                }
            }
        }

        components
    }

    /// Whether `f` is in a cycle: an SCC of more than one function, or a direct self-call.
    ///
    /// A function that is absent from the graph is not recursive.
    pub fn is_recursive(&self, f: FnId) -> bool {
        if self
            .callees
            .get(&f)
            .is_some_and(|targets| targets.contains(&f))
        {
            return true;
        }
        self.sccs()
            .iter()
            .any(|component| component.len() > 1 && component.contains(&f))
    }

    /// The functions reachable from `f` by following calls, including `f` itself.
    ///
    /// The walk follows `callees` only, so it visits the callees of a callee but never a caller.
    /// An id that is not in the graph yields just `{f}`; an edge to a callee that is not a key is
    /// still followed and included.
    pub fn reachable_from(&self, f: FnId) -> FxHashSet<FnId> {
        let mut reached = FxHashSet::default();
        reached.insert(f);
        let mut work = vec![f];
        while let Some(current) = work.pop() {
            if let Some(targets) = self.callees.get(&current) {
                for &target in targets {
                    if reached.insert(target) {
                        work.push(target);
                    }
                }
            }
        }
        reached
    }
}

/// The call-graph analysis.
pub struct CallGraphAnalysis;

impl Analysis for CallGraphAnalysis {
    type Output = Fact<CallGraph>;

    const ID: AnalysisId = AnalysisId::CallGraph;

    fn analyze(&self, _m: &Module) -> Self::Output {
        Fact::Unknown
    }
}

/// The call graph of `m`, or `Unknown` (see the module contract).
pub fn analyze(m: &Module) -> Fact<CallGraph> {
    CallGraphAnalysis.analyze(m)
}

/// Adds every function `e` calls directly or mentions as a value.
fn collect_calls(e: &Expr, out: &mut Vec<FnId>) {
    match e {
        Expr::Call {
            callee: Callee::Direct { f, .. },
            ..
        } => out.push(*f),
        Expr::Fn { f, .. } => out.push(*f),
        _ => {}
    }
    e.for_each_child(&mut |child| collect_calls(child, out));
}

/// Sorts by the arena index and removes duplicates. `FnId` order is module order.
fn sort_and_dedup(ids: &mut Vec<FnId>) {
    ids.sort_by_key(|f| f.into_raw());
    ids.dedup();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Expr, Lit, TypeId};

    /// A function with an empty parameter list and `body`.
    fn function(m: &mut Module, name: &str, body: Expr) -> FnId {
        let f = m.declare(name, false);
        m.define(f, Vec::new(), TypeId::UNIT, body);
        f
    }

    /// A function built directly in the arena, without `Module::declare`'s uniqueness scan (which
    /// is quadratic) so a long chain is cheap. It has the same empty signature as [`function`].
    fn raw_function(m: &mut Module, name: &str, body: Expr) -> FnId {
        m.functions.alloc(crate::ir::Function {
            name: name.to_owned(),
            exported: false,
            host_exports: Vec::new(),
            params: Vec::new(),
            ret: TypeId::UNIT,
            type_params: 0,
            set_params: Vec::new(),
            fails: false,
            body,
            external: None,
            suspends: false,
            constant: false,
            sets: None,
        })
    }

    /// A direct call of `f` with no arguments.
    fn call(f: FnId) -> Expr {
        Expr::Call {
            callee: Callee::Direct {
                f,
                ty_args: Vec::new(),
            },
            args: Vec::new(),
            fails: false,
            suspends: false,
        }
    }

    /// `f` used as a value.
    fn value(f: FnId) -> Expr {
        Expr::Fn {
            f,
            ty_args: Vec::new(),
        }
    }

    /// A direct call adds the edge in both directions.
    #[test]
    fn a_direct_call_is_an_edge_in_both_directions() {
        let mut m = Module::new();
        let a = m.declare("a", false);
        let b = m.declare("b", false);
        m.define(a, Vec::new(), TypeId::UNIT, call(b));
        m.define(b, Vec::new(), TypeId::UNIT, Expr::Lit(Lit::Unit));
        let graph = CallGraph::of_module(&m);
        assert_eq!(graph.callees[&a], vec![b]);
        assert_eq!(graph.callers[&b], vec![a]);
        assert!(graph.callees[&b].is_empty());
        assert!(!graph.callers.contains_key(&a));
    }

    /// A function that calls itself is a self-loop, and its SCC is a singleton.
    #[test]
    fn a_self_recursive_function_is_recursive() {
        let mut m = Module::new();
        let a = m.declare("a", false);
        m.define(a, Vec::new(), TypeId::UNIT, call(a));
        let graph = CallGraph::of_module(&m);
        assert_eq!(graph.sccs(), vec![vec![a]]);
        assert!(graph.is_recursive(a));
    }

    /// Two functions calling each other form one SCC of size two.
    #[test]
    fn mutual_recursion_is_one_component_of_two() {
        let mut m = Module::new();
        let a = m.declare("a", false);
        let b = m.declare("b", false);
        m.define(a, Vec::new(), TypeId::UNIT, call(b));
        m.define(b, Vec::new(), TypeId::UNIT, call(a));
        let graph = CallGraph::of_module(&m);
        assert_eq!(graph.sccs(), vec![vec![a, b]]);
        assert!(graph.is_recursive(a));
        assert!(graph.is_recursive(b));
    }

    /// A chain has three singleton components and no cycle.
    #[test]
    fn an_acyclic_chain_has_no_recursive_function() {
        let mut m = Module::new();
        let a = m.declare("a", false);
        let b = m.declare("b", false);
        let c = m.declare("c", false);
        m.define(a, Vec::new(), TypeId::UNIT, call(b));
        m.define(b, Vec::new(), TypeId::UNIT, call(c));
        m.define(c, Vec::new(), TypeId::UNIT, Expr::Lit(Lit::Unit));
        let graph = CallGraph::of_module(&m);
        assert_eq!(graph.callees[&a], vec![b]);
        assert_eq!(graph.callees[&b], vec![c]);
        assert!(graph.callees[&c].is_empty());
        assert_eq!(graph.sccs(), vec![vec![c], vec![b], vec![a]]);
        for f in [a, b, c] {
            assert!(!graph.is_recursive(f));
        }
    }

    /// A function used as a value is an edge, even though it is not called there.
    #[test]
    fn a_function_value_is_an_edge() {
        let mut m = Module::new();
        let a = m.declare("a", false);
        let b = m.declare("b", false);
        m.define(a, Vec::new(), TypeId::UNIT, value(b));
        m.define(b, Vec::new(), TypeId::UNIT, Expr::Lit(Lit::Unit));
        let graph = CallGraph::of_module(&m);
        assert_eq!(graph.callees[&a], vec![b]);
        assert_eq!(graph.callers[&b], vec![a]);
    }

    /// An indirect call has no direct callee: the graph records no edge for it.
    #[test]
    fn an_indirect_call_has_no_direct_edge() {
        let mut m = Module::new();
        let callee = m.local("callee", TypeId::UNIT);
        let a = m.declare("a", false);
        m.define(
            a,
            Vec::new(),
            TypeId::UNIT,
            Expr::Call {
                callee: Callee::Indirect(Box::new(Expr::Var(callee))),
                args: Vec::new(),
                fails: false,
                suspends: false,
            },
        );
        let graph = CallGraph::of_module(&m);
        assert!(graph.callees[&a].is_empty());
    }

    /// A function with no call or function value has an empty adjacency list and its own
    /// component.
    #[test]
    fn a_function_with_no_edges_has_an_empty_component() {
        let mut m = Module::new();
        let a = m.declare("a", false);
        m.define(a, Vec::new(), TypeId::UNIT, Expr::Lit(Lit::Unit));
        let graph = CallGraph::of_module(&m);
        assert!(graph.callees[&a].is_empty());
        assert!(graph.callers.is_empty());
        assert_eq!(graph.sccs(), vec![vec![a]]);
        assert!(!graph.is_recursive(a));
    }

    /// Two runs over the same module produce the same vectors and components.
    #[test]
    fn the_graph_and_its_components_are_deterministic() {
        let mut m = Module::new();
        let a = m.declare("a", false);
        let b = m.declare("b", false);
        let c = m.declare("c", false);
        m.define(
            a,
            Vec::new(),
            TypeId::UNIT,
            Expr::Tuple(vec![call(c), call(b), call(c), value(b)]),
        );
        m.define(b, Vec::new(), TypeId::UNIT, call(c));
        m.define(c, Vec::new(), TypeId::UNIT, Expr::Lit(Lit::Unit));
        let first = CallGraph::of_module(&m);
        let second = CallGraph::of_module(&m);
        assert_eq!(first.callees[&a], vec![b, c]);
        assert_eq!(first.callees[&a], second.callees[&a]);
        assert_eq!(first, second);
        assert_eq!(first.sccs(), second.sccs());
    }

    /// Reachability starts at the function itself and follows calls transitively.
    #[test]
    fn reachable_from_includes_the_start_and_follows_edges() {
        let mut m = Module::new();
        let a = m.declare("a", false);
        let b = m.declare("b", false);
        let c = m.declare("c", false);
        let d = m.declare("d", false);
        m.define(a, Vec::new(), TypeId::UNIT, call(b));
        m.define(b, Vec::new(), TypeId::UNIT, call(c));
        m.define(c, Vec::new(), TypeId::UNIT, Expr::Lit(Lit::Unit));
        m.define(d, Vec::new(), TypeId::UNIT, Expr::Lit(Lit::Unit));
        let graph = CallGraph::of_module(&m);
        let reached = graph.reachable_from(a);
        assert_eq!(reached, FxHashSet::from_iter([a, b, c]));
        assert!(!reached.contains(&d));
        assert_eq!(graph.reachable_from(d), FxHashSet::from_iter([d]));
    }

    /// An edge to a function that is not a key of either map is still a node: no index lookup
    /// panics, and it is reachable.
    #[test]
    fn a_callee_missing_from_the_map_does_not_panic() {
        let mut m = Module::new();
        let a = function(&mut m, "a", Expr::Lit(Lit::Unit));
        let mut other = Module::new();
        let _pad = function(&mut other, "pad", Expr::Lit(Lit::Unit));
        let orphan = function(&mut other, "orphan", Expr::Lit(Lit::Unit));
        assert_ne!(a, orphan);

        let graph = CallGraph {
            callees: FxHashMap::from_iter([(a, vec![orphan])]),
            callers: FxHashMap::from_iter([(orphan, vec![a])]),
        };
        assert_eq!(graph.sccs(), vec![vec![orphan], vec![a]]);
        assert!(!graph.is_recursive(a));
        assert!(!graph.is_recursive(orphan));
        assert_eq!(graph.reachable_from(a), FxHashSet::from_iter([a, orphan]));
    }

    /// A long chain is handled without recursing on the process stack. This is the shape that
    /// would overflow a recursive Tarjan.
    #[test]
    fn a_long_chain_does_not_overflow_the_stack() {
        let mut m = Module::new();
        let first = raw_function(&mut m, "f0", Expr::Lit(Lit::Unit));
        let mut previous = first;
        for i in 1..20_000 {
            let next = raw_function(&mut m, &format!("f{i}"), Expr::Lit(Lit::Unit));
            m.functions[previous].body = call(next);
            previous = next;
        }
        let graph = CallGraph::of_module(&m);
        assert_eq!(graph.sccs().len(), 20_000);
        assert!(!graph.is_recursive(first));
        assert_eq!(graph.reachable_from(first).len(), 20_000);
    }
}
