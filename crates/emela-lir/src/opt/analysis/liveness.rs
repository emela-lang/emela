//! Liveness: which variables are live after a statement or at a suspension point.
//!
//! # Contract
//!
//! ```text
//! Analysis ID: AnalysisId::Liveness
//! Domain / lattice: for each function, a map from statement to the set of variables live after
//!                   it. Sets are ordered by inclusion at a join; `Unknown` is the top element.
//! Transfer: backward. A use makes a variable live, a definition kills it, and a branch, loop,
//!           failure landing or suspension edge joins the sets of its successors.
//! Soundness: a variable in a `Known` set is read on some path after the statement. The analysis
//!            never leaves out a live variable.
//! Deopt (unknown): a consumer that gets `Unknown` does not remove the statement, the frame slot or
//!                  the reference it wanted to remove. It must not read `Unknown` as "not live".
//! Consumers: LIR DCE and frame pruning (C7/PR-028), rc cleanup (C1/PR-025) and SRA (C5/PR-043).
//! ```
//!
//! # Status
//!
//! Skeleton of B0/PR-002: [`analyze`] returns `Unknown`. The current liveness pass in
//! `crate::heap::live` keeps serving the state machines and the verifier; B4/PR-023 moves its
//! logic behind this API.

use rustc_hash::{FxHashMap, FxHashSet};

use crate::ir::{FnId, Module, StmtId, VarId};

use super::{Analysis, AnalysisId, Fact};

/// The variables live after each statement of each function.
pub type LiveSets = FxHashMap<FnId, FxHashMap<StmtId, FxHashSet<VarId>>>;

/// The liveness analysis.
pub struct Liveness;

impl Analysis for Liveness {
    type Output = Fact<LiveSets>;

    const ID: AnalysisId = AnalysisId::Liveness;

    fn analyze(&self, _m: &Module) -> Self::Output {
        Fact::Unknown
    }
}

/// The live sets of `m`, or `Unknown` (see the module contract).
pub fn analyze(m: &Module) -> Fact<LiveSets> {
    Liveness.analyze(m)
}
