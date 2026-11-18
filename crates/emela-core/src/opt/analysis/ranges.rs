//! Ranges of integer locals, including induction variables.
//!
//! # Contract
//!
//! ```text
//! Analysis ID: AnalysisId::Ranges
//! Domain / lattice: a partial map from Local to an inclusive integer interval, per signedness.
//!                   An interval may be unbounded; `Unknown` is the top element. Intervals are
//!                   intersected at joins, so the lattice is the usual interval lattice.
//! Transfer: literals give a point interval; arithmetic widens the result with the 32-bit or
//!           64-bit wrap of Int and Int64; an operation the analysis cannot bound gives an
//!           unbounded interval, not `Unknown`, only when every value of the local is still
//!           covered.
//! Soundness: a `Known` interval contains every value the local can hold at the point asked about.
//!            The wrap of Int and Int64 is part of the transfer, not ignored.
//! Deopt (unknown): a consumer that gets `Unknown` keeps the bounds check or the branch it wanted
//!                  to remove. It never assumes an index is in bounds.
//! Consumers: bounds-check elimination (A11/PR-040), induction variables and loop unswitching.
//! ```
//!
//! # Status
//!
//! Skeleton of B0/PR-002: [`analyze`] returns `Unknown`. B5/PR-024 implements the interval domain,
//! its wrap-aware transfer and induction variables. The placeholder [`Interval`] is deliberately
//! minimal; B5 replaces it with the signed/unsigned and Int/Int64 form it needs.

use rustc_hash::FxHashMap;

use crate::ir::{Local, Module};

use super::{Analysis, AnalysisId, Fact};

/// An inclusive integer interval. The bounds are stored as `i64`; a full `Int64` interval is
/// represented as unbounded rather than by an out-of-range bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interval {
    /// The smallest value, inclusive.
    pub lo: i64,
    /// The largest value, inclusive.
    pub hi: i64,
}

impl Interval {
    /// The interval `[lo, hi]`. `lo` must not be greater than `hi`.
    pub const fn new(lo: i64, hi: i64) -> Self {
        Self { lo, hi }
    }
}

/// The range of each local the analysis could bound.
pub type Ranges = FxHashMap<Local, Interval>;

/// The range analysis.
pub struct RangesAnalysis;

impl Analysis for RangesAnalysis {
    type Output = Fact<Ranges>;

    const ID: AnalysisId = AnalysisId::Ranges;

    fn analyze(&self, _m: &Module) -> Self::Output {
        Fact::Unknown
    }
}

/// The ranges of `m`, or `Unknown` (see the module contract).
pub fn analyze(m: &Module) -> Fact<Ranges> {
    RangesAnalysis.analyze(m)
}
