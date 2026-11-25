//! Ownership facts: which references are known to be uniquely owned (reference count one).
//!
//! # Contract
//!
//! ```text
//! Analysis ID: AnalysisId::Ownership
//! Domain / lattice: a set of (function, variable) pairs that are uniquely owned. `Unknown` is the
//!                   top element; the empty set is the bottom.
//! Transfer: a freshly allocated value is uniquely owned until a `dup`, a store into a container,
//!           a capture, or a pass to a callee that takes ownership. A `drop` consumes it. The
//!           transfer follows the definitions in statement order and joins branches by
//!           intersection.
//! Soundness: a pair in a `Known` set has reference count one on every path at that point, so
//!            mutating or reusing the value in place is not observable.
//! Deopt (unknown): a consumer that gets `Unknown` does not reuse the constructor, mutate in place
//!                  or remove the copy. It must not read `Unknown` as "uniquely owned".
//! Consumers: Perceus reuse / FBIP (C4/PR-042), in-place array and record updates, and the
//!            `Array.set` decision.
//! ```
//!
//! # Status
//!
//! Skeleton of B0/PR-002: [`analyze`] returns `Unknown`. B6/PR-024 implements the facts on top of
//! escape and liveness.

use rustc_hash::FxHashSet;

use crate::ir::{FnId, Module, VarId};

use super::{Analysis, AnalysisId, Fact};

/// The references known to be uniquely owned.
pub type UniqueRefs = FxHashSet<(FnId, VarId)>;

/// The ownership analysis.
pub struct Ownership;

impl Analysis for Ownership {
    type Output = Fact<UniqueRefs>;

    const ID: AnalysisId = AnalysisId::Ownership;

    fn analyze(&self, _m: &Module) -> Self::Output {
        Fact::Unknown
    }
}

/// The uniquely owned references of `m`, or `Unknown` (see the module contract).
pub fn analyze(m: &Module) -> Fact<UniqueRefs> {
    Ownership.analyze(m)
}
