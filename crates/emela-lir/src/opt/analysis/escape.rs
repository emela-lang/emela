//! Escape analysis: which values of a function may leave it, or reach a place it does not control.
//!
//! # Contract
//!
//! ```text
//! Analysis ID: AnalysisId::Escape
//! Domain / lattice: for each function, a set of variables that may escape. The top element is
//!                   `Unknown`; the bottom is the empty set.
//! Transfer: a value escapes when it is a parameter, returned, failed with, stored in a container,
//!           a handler or an evidence table, captured by a closure, boxed or unboxed, passed to a
//!           call or builtin, has a finalizer or untracked representation, or is live across a
//!           suspension point. The fields of an escaping aggregate escape with it.
//! Soundness: a variable in a `Known` set may escape; the analysis never omits a value that does.
//!            A wrong `false` changes behaviour, while a wrong `true` only loses an optimisation.
//! Deopt (unknown): a consumer that gets `Unknown` does not decompose or reuse the value. It must
//!                  not read `Unknown` as "does not escape".
//! Consumers: SRA (B3/PR-022, then C5/PR-043), reuse (C4/PR-042), borrow inference (C2/PR-026)
//!            and closure devirtualisation (E5/PR-037).
//! ```
//!
//! # Status
//!
//! Skeleton of B0/PR-002: [`analyze`] returns `Unknown`. The implementation lives in
//! `crate::opt::escape` today and SRA keeps calling it there; B3/PR-022 moves it behind this
//! API without changing its answer.

use rustc_hash::{FxHashMap, FxHashSet};

use crate::ir::{FnId, Module, VarId};

use super::{Analysis, AnalysisId, Fact};

/// The variables of each function that may escape it.
pub type EscapeSets = FxHashMap<FnId, FxHashSet<VarId>>;

/// The escape analysis.
pub struct Escape;

impl Analysis for Escape {
    type Output = Fact<EscapeSets>;

    const ID: AnalysisId = AnalysisId::Escape;

    fn analyze(&self, _m: &Module) -> Self::Output {
        Fact::Unknown
    }
}

/// The escaping variables of `m`, or `Unknown` (see the module contract).
pub fn analyze(m: &Module) -> Fact<EscapeSets> {
    Escape.analyze(m)
}
