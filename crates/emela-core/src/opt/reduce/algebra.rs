//! Algebraic reduction: applying the laws of Bool and of the total integer operators.
//!
//! # Contract
//!
//! ```text
//! Rule ID: algebra
//! Theory: algebraic laws of the Bool lattice and of total integer operations
//! Equation: x && true → x, false && e → false, true || e → true, x || false → x,
//!           if true then e1 else e2 → e1, if false then e1 else e2 → e2
//! Preconditions: when the equation removes the evaluated side (`false && e`, `true || e`, an
//!                `if` arm), that side is trivial: it has no effect, cannot fail, cannot suspend and
//!                cannot diverge. Short-circuit evaluation of the kept side is unchanged.
//! Postconditions: the constant branch or operation is gone.
//! Preserves: evaluation order, effects, failures, suspension points, drop points.
//! Deopt: the side to remove is not trivial; the operation is integer division, remainder or
//!        comparison whose value depends on wrapping or on a defect; the law would change the
//!        number of evaluations of a non-trivial expression.
//! Budget: strictly smaller.
//! Measures: [`crate::opt::Stats::algebra`].
//! ```
//!
//! # Status
//!
//! Skeleton only. A7/PR-016 moves the logical half of the current simplifier here. Until then
//! [`algebra`] returns [`crate::opt::RuleStats::default`] and rewrites nothing.

use crate::ir::Module;
use crate::opt::RuleStats;

/// Applies the Bool and integer algebraic laws that remove a constant side.
pub(in crate::opt) fn algebra(_m: &mut Module) -> RuleStats {
    RuleStats::default()
}
