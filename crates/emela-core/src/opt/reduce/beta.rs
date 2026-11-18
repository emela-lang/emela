//! β reduction: calling a lambda that was bound to a variable.
//!
//! # Contract
//!
//! ```text
//! Rule ID: beta
//! Theory: β reduction of the lambda calculus, expressed with a let binding
//! Equation: let f = λx.e in C[f v] → let x = v in C[e]
//! Preconditions: f is called exactly once and is not used as a value otherwise; the argument v is
//!                evaluated exactly once, left to right, at the point of the call (call by value,
//!                spec [dyn.order]).
//! Postconditions: the lambda and the call are gone; the argument is bound to the parameter and
//!                the body takes the place of the call.
//! Preserves: evaluation order, effects, failures, suspension points, drop points.
//! Deopt: f is used zero times or more than once, f is passed as a value, the call is not a direct
//!        application of f, or the copy would need a fresh local that the substitution engine does
//!        not yet provide.
//! Budget: one copy of the lambda body per call; the A1/PR-010 size measure guards it.
//! Measures: [`crate::opt::Stats::beta`].
//! ```
//!
//! # Status
//!
//! Skeleton only. A3/PR-012 implements β reduction after the substitution engine (A1/PR-010).
//! Until then [`beta`] returns [`crate::opt::RuleStats::default`] and rewrites nothing.

use crate::ir::Module;
use crate::opt::RuleStats;

/// Rewrites a linear `let f = λx.e in C[f v]` into `let x = v in C[e]`.
pub(in crate::opt) fn beta(_m: &mut Module) -> RuleStats {
    RuleStats::default()
}
