//! Constant folding: evaluating an operation on literal operands.
//!
//! # Contract
//!
//! ```text
//! Rule ID: fold (constant folding)
//! Theory: δ reduction and constant evaluation of the builtin operators
//! Equation: op(lit, lit) → lit
//! Preconditions: every operand is a literal of a type the rule folds; the operation is an integer,
//!                Bool or String operation whose result is fully defined for those operands.
//! Postconditions: the operation is replaced by its literal result.
//! Preserves: evaluation order, effects, failures, suspension points, drop points. Integer
//!            arithmetic keeps the 32-bit wrap of the target-independent Int and the 64-bit wrap of
//!            Int64.
//! Deopt: Int division or remainder by zero keeps the defect (it is not folded away); Float is
//!        never folded (its rounding is not a source-level constant); an operand may fail; the
//!        operation is one whose result is left to the backend.
//! Budget: strictly smaller.
//! Measures: [`crate::opt::Stats::constant`].
//! ```
//!
//! # Status
//!
//! Skeleton only. A6/PR-015 moves the constant half of the current simplifier here. Until then
//! [`fold`] returns [`crate::opt::RuleStats::default`] and rewrites nothing.

use crate::ir::Module;
use crate::opt::RuleStats;

/// Replaces an operation whose operands are literals by its literal result.
pub(in crate::opt) fn fold(_m: &mut Module) -> RuleStats {
    RuleStats::default()
}
