//! δ reduction (definition expansion): replacing a direct call by the callee's body.
//!
//! This is the reduction that the compiler calls inlining. It may make the program larger, so it
//! is the only rule of the kernel that carries a budget.
//!
//! # Contract
//!
//! ```text
//! Rule ID: delta
//! Theory: δ reduction in the lambda calculus, with a cost model
//! Equation: f v → [v/x]body(f); const c = e is expanded only in the limited cases the cost model
//!           allows
//! Preconditions: the callee is a direct call of a function with a known body (not an external
//!                function, not a core function that a fusion matches), the call is the only use
//!                of that function if it is also used as a value, and the argument evaluation
//!                order is kept. The cost model must accept the call: a large or recursive body is
//!                left alone.
//! Postconditions: the call is gone, the callee's parameters are bound to the arguments and its
//!                body takes the place of the call.
//! Preserves: evaluation order, effects, failures, suspension points, the order and count of the
//!            releases of `with`, drop points.
//! Deopt: the call is indirect or through a handler, the callee is external, the callee is a core
//!        function another pass matches, the body exceeds the per-call or per-function budget,
//!        the call is recursive without a decreasing argument, or the expansion would duplicate
//!        a binder that needs a fresh local.
//! Budget: [`crate::opt::Stats`] does not fix a number here; A8/PR-017 owns the cost model and
//!         documents the per-call and per-module limits in this module.
//! Measures: [`crate::opt::Stats::delta`].
//! ```
//!
//! # Status
//!
//! Skeleton only. A8/PR-017 separates the existing call inlining (`super::super::inline`) into
//! β reduction and this δ reduction. Until then [`delta`] returns
//! [`crate::opt::RuleStats::default`] and rewrites nothing; the old inliner keeps running in the
//! pipeline.

use crate::ir::Module;
use crate::opt::RuleStats;

/// Expands a direct call `f v` to a copy of `f`'s body, when the cost model accepts it.
pub(in crate::opt) fn delta(_m: &mut Module) -> RuleStats {
    RuleStats::default()
}
