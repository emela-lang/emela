//! Use counts of locals.
//!
//! # Contract
//!
//! ```text
//! Analysis ID: AnalysisId::Uses
//! Domain / lattice: a partial map from Local to a count. `Unknown` is the top element; a map that
//!                   omits a local says the local has no use.
//! Transfer: count every occurrence of a local as a value, a callee, an argument, a field base, a
//!           pattern guard's free variable, a release body and so on; a binder is not a use.
//! Soundness: a count is the number of evaluations of the local's value on the paths the analysis
//!            covers, and never under-counts a use of a local in the function.
//! Deopt (unknown): a consumer that gets `Unknown` does not remove or duplicate a binding. In
//!                  particular, the β reduction does not assume a function is linear.
//! Consumers: β and δ reduction (linearity), dead-let and dead-code removal, the bounds and CSE
//!            rules.
//! ```
//!
//! # Status
//!
//! Skeleton of B0/PR-002: [`analyze`] returns `Unknown`. B2/PR-021 implements use counts.

use rustc_hash::FxHashMap;

use crate::ir::{Local, Module};

use super::{Analysis, AnalysisId, Fact};

/// The number of uses of each local of the module.
pub type UseCounts = FxHashMap<Local, u32>;

/// The use-count analysis.
pub struct Uses;

impl Analysis for Uses {
    type Output = Fact<UseCounts>;

    const ID: AnalysisId = AnalysisId::Uses;

    fn analyze(&self, _m: &Module) -> Self::Output {
        Fact::Unknown
    }
}

/// The use counts of `m`, or `Unknown` (see the module contract).
pub fn analyze(m: &Module) -> Fact<UseCounts> {
    Uses.analyze(m)
}
