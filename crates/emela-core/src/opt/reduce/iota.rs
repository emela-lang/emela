//! ι reduction: matching a value whose constructor is already known.
//!
//! # Contract
//!
//! ```text
//! Rule ID: iota
//! Theory: case-of-known-constructor
//! Equation: match K(v) { … K(x) -> e … } → e[x := v]
//! Preconditions: the scrutinee is a constructor application of the matched arm, no earlier arm
//!                has a guard that could match, and the pattern of the matched arm binds only
//!                variables (no nested pattern that would test the value again). A nested pattern
//!                keeps the evaluation order of the values it destructures.
//! Postconditions: the match and the constructor are gone; the arm's body takes their place with
//!                the pattern variables bound to the constructor's arguments.
//! Preserves: evaluation order, effects, failures, suspension points, drop points.
//! Deopt: the scrutinee is not a known constructor, no arm matches, an earlier arm is not provably
//!        disjoint (a guard), the pattern is nested or refutable, or the arm is reached after the
//!        value would have been evaluated.
//! Budget: one copy of the arm body; the A1/PR-010 size measure guards it.
//! Measures: [`crate::opt::Stats::iota`].
//! ```
//!
//! # Status
//!
//! Skeleton only. A5/PR-014 implements ι reduction. Until then [`iota`] returns
//! [`crate::opt::RuleStats::default`] and rewrites nothing.

use crate::ir::Module;
use crate::opt::RuleStats;

/// Rewrites `match K(v) { … K(x) -> e … }` to `e[x := v]` for a known constructor.
pub(in crate::opt) fn iota(_m: &mut Module) -> RuleStats {
    RuleStats::default()
}
