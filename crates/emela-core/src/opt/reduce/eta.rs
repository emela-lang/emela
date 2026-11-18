//! η reduction: removing a lambda that only forwards its parameter.
//!
//! # Contract
//!
//! ```text
//! Rule ID: eta
//! Theory: η conversion of the lambda calculus
//! Equation: λx. f x → f
//! Preconditions: x does not occur free in f; the call `f x` is a direct application that forwards
//!                exactly one argument; f's function type has the same calling convention as the
//!                lambda (parameters, return type, failure, suspension and evidence).
//! Postconditions: the wrapper lambda is gone and f takes its place.
//! Preserves: evaluation order, effects, failures, suspension points, drop points.
//! Deopt: x occurs in f, the body is not `f x`, the call has named or reordered arguments, or the
//!        calling conventions differ.
//! Budget: strictly smaller.
//! Measures: [`crate::opt::Stats::eta`].
//! ```
//!
//! # Status
//!
//! Skeleton only. A4/PR-013 implements η reduction. Until then [`eta`] returns
//! [`crate::opt::RuleStats::default`] and rewrites nothing.

use crate::ir::Module;
use crate::opt::RuleStats;

/// Rewrites `λx. f x` to `f` when `x` is not free in `f` and the calling conventions match.
pub(in crate::opt) fn eta(_m: &mut Module) -> RuleStats {
    RuleStats::default()
}
