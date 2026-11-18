//! ζ reduction: substituting a let-bound value, and removing a dead trivial let.
//!
//! # Contract
//!
//! ```text
//! Rule ID: zeta
//! Theory: let calculus (copy propagation) and weakening
//! Equation: let x = v in e → e[x := v]        when v is a variable or a scalar literal
//!           let x = v in e → e                when x is unused and v is trivial
//! Preconditions: v is trivial, so reading it at each use neither repeats an effect nor moves one.
//!                The dead-let form also requires that v has no effect, cannot fail and cannot
//!                diverge, because the binding no longer evaluates it.
//! Postconditions: every use of x in e is v, or the binding is gone and x has no use in e.
//! Preserves: evaluation order, effects, failures, suspension points, drop points.
//! Deopt: v is not trivial (A2/PR-011 uses the finished substitution engine to decide when a
//!        single-use non-trivial value may still be moved).
//! Budget: the substituted value is trivial, so the expression does not grow.
//! Measures: [`crate::opt::Stats::zeta`].
//! ```
//!
//! # Status
//!
//! Skeleton only. A2/PR-011 implements the rule on top of the substitution engine (A1/PR-010).
//! Until then [`zeta`] returns [`crate::opt::RuleStats::default`] and rewrites nothing.

use crate::ir::Module;
use crate::opt::RuleStats;

/// Rewrites `let x = v in e` to `e[x := v]` when `v` is trivial, and removes a dead trivial let.
pub(in crate::opt) fn zeta(_m: &mut Module) -> RuleStats {
    RuleStats::default()
}
