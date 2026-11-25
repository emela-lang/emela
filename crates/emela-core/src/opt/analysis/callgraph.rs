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
//! # Status
//!
//! Skeleton of B0/PR-002: [`analyze`] returns `Unknown`. B2/PR-021 implements the graph and its
//! components.

use rustc_hash::FxHashMap;

use crate::ir::{FnId, Module};

use super::{Analysis, AnalysisId, Fact};

/// The calls of a module, by caller, and the strongly connected components of the graph.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallGraph {
    /// For each caller, the functions it calls directly. A function with no entry calls none.
    pub edges: FxHashMap<FnId, Vec<FnId>>,
    /// The strongly connected components, each in a deterministic order.
    pub sccs: Vec<Vec<FnId>>,
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
