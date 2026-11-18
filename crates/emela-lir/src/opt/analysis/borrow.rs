//! Borrow facts: which parameters a function may receive without taking ownership.
//!
//! # Contract
//!
//! ```text
//! Analysis ID: AnalysisId::Borrow
//! Domain / lattice: for each function, a Boolean per parameter: `true` when the parameter may be
//!                   borrowed. The lattice is pointwise implication; `Unknown` is the top element.
//! Transfer: a parameter may be borrowed when the function never stores it, returns it, captures
//!           it, lets it escape through a call or a container, or keeps it live across a
//!           suspension point. The transfer crosses direct calls: a caller's ownership decision
//!           reads the callee's borrow facts.
//! Soundness: `Known(true)` means the callee does not take ownership, so the caller may keep its
//!           reference. The analysis never marks a parameter borrowed when it may be stored.
//! Deopt (unknown): a consumer that gets `Unknown` leaves the `dup` and the ownership transfer as
//!                  they are. It must not read `Unknown` as "borrowed".
//! Consumers: borrow inference (C2/PR-026), rc cleanup (C1/PR-025) and Perceus reuse (C4/PR-042).
//! ```
//!
//! # Status
//!
//! Skeleton of B0/PR-002: [`analyze`] returns `Unknown`. B6/PR-024 (with B3 and B4) implements
//! the facts.

use rustc_hash::FxHashMap;

use crate::ir::{FnId, Module};

use super::{Analysis, AnalysisId, Fact};

/// For each function, one flag per parameter: whether it may be borrowed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BorrowFacts {
    /// Parallel to `Function::params`.
    pub borrowed_params: FxHashMap<FnId, Vec<bool>>,
}

/// The borrow analysis.
pub struct Borrow;

impl Analysis for Borrow {
    type Output = Fact<BorrowFacts>;

    const ID: AnalysisId = AnalysisId::Borrow;

    fn analyze(&self, _m: &Module) -> Self::Output {
        Fact::Unknown
    }
}

/// The borrow facts of `m`, or `Unknown` (see the module contract).
pub fn analyze(m: &Module) -> Fact<BorrowFacts> {
    Borrow.analyze(m)
}
