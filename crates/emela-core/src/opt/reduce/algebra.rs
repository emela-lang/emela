//! Algebraic reduction: the Bool laws that remove a constant side of `&&`, `||`, `if` and `!!`.
//!
//! The rewrite is bottom-up: the children of a node are reduced first, so a literal a child
//! produced (a comparison folded by [`super::fold`], an arm taken by this rule) decides its
//! parent in the same sweep.
//!
//! A side this rule removes was either never evaluated (`false && e`, `true || e`: `&&` and `||`
//! short-circuit) or is a literal (the left side of `true && e`, the condition of an `if`). The
//! one remaining removal is the left side of `e && false` / `e || true`: it *is* evaluated today,
//! so it may only be dropped when it is trivial ([`subst::is_trivial`]) — otherwise its effect,
//! failure, suspension or defect would disappear with it.
//!
//! An `if` whose taken arm is the literal empty string is deliberately not reduced, even though
//! the condition is a literal. The native target rejects a constant empty separator at compile
//! time (`[target.native.unsupported]`, `String.split` with an empty separator), while a
//! separator that is empty at run time is a run-time defect (or, on ASCII text, a split by
//! character). Folding the `if` would turn that run-time behaviour into a compile error, so the
//! `if` stays; the deopt is counted and a driver test builds the program.
//!
//! # Contract
//!
//! ```text
//! Rule ID: algebra (algebraic reduction)
//! Theory: the algebraic laws of Bool, restricted to the sides a short-circuit or a literal shows
//!         are not evaluated
//! Equation: true && e → e, false && e → false, e && true → e, e && false → false (e trivial),
//!           false || e → e, true || e → true, e || false → e, e || true → true (e trivial),
//!           if true then a else b → a, if false then a else b → b (the taken arm is not ""),
//!           !!e → e
//! Preconditions: `&&` and `||` are `OpTy::Bool` and evaluate their right side only when the left
//!                side does not decide the result. The left side of `e && false` or `e || true`
//!                is `subst::is_trivial`. An `if` condition is a Bool literal, and the arm it
//!                selects is not the literal empty string. Both nodes of `!!e` are `UnOp::Not`
//!                of `OpTy::Bool`.
//! Postconditions: the operation or `if` and its constant operand or literal condition are gone;
//!                 the remaining expression is the side that was already there, unchanged and of
//!                 the same type. A second run changes nothing.
//! Preserves: evaluation order, effects, failures, suspension points, drop points. The removed
//!            side was either a literal, never evaluated because of the short-circuit, or trivial
//!            by the precondition.
//! Deopt: `e && false` and `e || true` whose left side is not trivial keep the evaluation of
//!        that side; an `if` whose taken arm is the literal empty string stays, because the
//!        native target rejects a constant empty separator at compile time while an empty
//!        separator at run time is a run-time defect. Such sites are counted in
//!        `RuleStats::deopt`. A condition that is not a Bool literal, a `Not` whose type is not
//!        Bool, and an `&&`/`||` whose operand type is not Bool are not candidates.
//! Budget: strictly smaller: one operation or `if` node and its constant operand or literal
//!         condition disappear, and no subexpression is copied.
//! Measures: [`crate::opt::Stats::algebra`].
//! ```
//!
//! # Status
//!
//! A7/PR-016 implements the Bool laws of the pipeline. The integer laws of the A0 skeleton's
//! first contract are not part of this rule: the total integer operations are evaluated by
//! [`super::fold`], and no other law (`if c then true else false → c`, absorption, `x + 0`) is
//! added in M1.

use crate::ir::{BinOp, Expr, Lit, Module, OpTy, UnOp};
use crate::opt::{RuleStats, each_body};

use super::subst;

/// Applies the Bool algebraic laws that remove a constant side.
pub(in crate::opt) fn algebra(m: &mut Module) -> RuleStats {
    let mut stats = RuleStats::default();
    each_body(m, &mut |_, e| rewrite(e, &mut stats));
    stats
}

/// Rewrites `e` bottom-up: children first, then the node they may have made constant.
fn rewrite(e: &mut Expr, stats: &mut RuleStats) {
    crate::tail::for_each_child_mut(e, &mut |child| rewrite(child, stats));
    match law(e) {
        Outcome::Rewritten(replacement) => {
            debug_assert!(
                subst::size(&replacement) < subst::size(e),
                "an algebraic law must not grow the expression"
            );
            *e = replacement;
            stats.applied += 1;
        }
        Outcome::Deopt => stats.deopt += 1,
        Outcome::Keep => {}
    }
}

/// What [`law`] decided about one node.
enum Outcome {
    /// The node is the expression the law gives.
    Rewritten(Expr),
    /// The node matches a law whose precondition is not met; it stays as it is.
    Deopt,
    /// The node is not one the laws rewrite.
    Keep,
}

/// The expression `e` reduces to, when one of the Bool laws applies.
fn law(e: &Expr) -> Outcome {
    match e {
        Expr::Binary {
            op: BinOp::And,
            ty: OpTy::Bool,
            lhs,
            rhs,
        } => match (bool_lit(lhs), bool_lit(rhs)) {
            // The right side is not evaluated, before or after.
            (Some(true), _) => Outcome::Rewritten((**rhs).clone()),
            (Some(false), _) => Outcome::Rewritten(bool_expr(false)),
            // `e` is evaluated either way; its value decides.
            (_, Some(true)) => Outcome::Rewritten((**lhs).clone()),
            // Dropping `e` must not drop an effect, a failure or a defect.
            (_, Some(false)) if subst::is_trivial(lhs) => Outcome::Rewritten(bool_expr(false)),
            (_, Some(false)) => Outcome::Deopt,
            _ => Outcome::Keep,
        },
        Expr::Binary {
            op: BinOp::Or,
            ty: OpTy::Bool,
            lhs,
            rhs,
        } => match (bool_lit(lhs), bool_lit(rhs)) {
            (Some(false), _) => Outcome::Rewritten((**rhs).clone()),
            (Some(true), _) => Outcome::Rewritten(bool_expr(true)),
            (_, Some(false)) => Outcome::Rewritten((**lhs).clone()),
            (_, Some(true)) if subst::is_trivial(lhs) => Outcome::Rewritten(bool_expr(true)),
            (_, Some(true)) => Outcome::Deopt,
            _ => Outcome::Keep,
        },
        // An arm that is the literal empty string is not selected. The native target rejects a
        // constant empty separator at compile time (`[target.native.unsupported]`), while a
        // separator that is empty at run time is a run-time defect: replacing the `if` by the
        // literal would turn that run-time defect into a compile error.
        Expr::If { cond, then, else_ } => match bool_lit(cond) {
            Some(true) if !is_empty_string(then) => Outcome::Rewritten((**then).clone()),
            Some(false) if !is_empty_string(else_) => Outcome::Rewritten((**else_).clone()),
            Some(_) => Outcome::Deopt,
            None => Outcome::Keep,
        },
        // `!!e` is `e`: both are Bool, and nothing is evaluated between the two `!`s.
        Expr::Unary {
            op: UnOp::Not,
            ty: OpTy::Bool,
            operand,
        } => match &**operand {
            Expr::Unary {
                op: UnOp::Not,
                ty: OpTy::Bool,
                operand,
            } => Outcome::Rewritten((**operand).clone()),
            _ => Outcome::Keep,
        },
        _ => Outcome::Keep,
    }
}

/// A Bool literal expression.
fn bool_expr(value: bool) -> Expr {
    Expr::Lit(Lit::Bool(value))
}

/// Whether `e` is the literal empty string.
fn is_empty_string(e: &Expr) -> bool {
    matches!(e, Expr::Lit(Lit::String(s)) if s.is_empty())
}

/// The Bool literal `e` is, if it is one.
fn bool_lit(e: &Expr) -> Option<bool> {
    match e {
        Expr::Lit(Lit::Bool(b)) => Some(*b),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{bool, int};

    fn binary(op: BinOp, ty: OpTy, lhs: Expr, rhs: Expr) -> Expr {
        Expr::Binary {
            op,
            ty,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        }
    }

    fn unary(op: UnOp, ty: OpTy, operand: Expr) -> Expr {
        Expr::Unary {
            op,
            ty,
            operand: Box::new(operand),
        }
    }

    fn if_(cond: Expr, then: Expr, else_: Expr) -> Expr {
        Expr::If {
            cond: Box::new(cond),
            then: Box::new(then),
            else_: Box::new(else_),
        }
    }

    fn string(s: &str) -> Expr {
        Expr::Lit(Lit::String(s.to_owned()))
    }

    /// The guard is on the arm the condition selects, not on both arms: the untaken arm's empty
    /// string is dead code and does not stop the fold.
    #[test]
    fn only_the_selected_arm_is_guarded() {
        assert!(matches!(
            law(&if_(bool(true), string(""), string(","))),
            Outcome::Deopt
        ));
        assert!(matches!(
            law(&if_(bool(false), string(","), string(""))),
            Outcome::Deopt
        ));
        assert!(matches!(
            law(&if_(bool(true), string("a"), string(""))),
            Outcome::Rewritten(Expr::Lit(Lit::String(s))) if s == "a"
        ));
        assert!(matches!(
            law(&if_(bool(false), string(""), string("b"))),
            Outcome::Rewritten(Expr::Lit(Lit::String(s))) if s == "b"
        ));
    }

    /// Both `!`s must be the Bool one; a double negation of anything else is left alone.
    #[test]
    fn only_a_bool_double_negation_is_its_operand() {
        let e = unary(UnOp::Not, OpTy::Bool, unary(UnOp::Not, OpTy::Int, int(1)));
        assert!(matches!(law(&e), Outcome::Keep));
        let e = unary(UnOp::Not, OpTy::Int, unary(UnOp::Not, OpTy::Int, int(1)));
        assert!(matches!(law(&e), Outcome::Keep));
    }

    /// A constant on both sides is decided by the left side, as the short-circuit does.
    #[test]
    fn a_constant_left_side_decides() {
        assert!(matches!(
            law(&binary(BinOp::And, OpTy::Bool, bool(true), bool(false))),
            Outcome::Rewritten(Expr::Lit(Lit::Bool(false)))
        ));
        assert!(matches!(
            law(&binary(BinOp::Or, OpTy::Bool, bool(true), bool(false))),
            Outcome::Rewritten(Expr::Lit(Lit::Bool(true)))
        ));
    }
}
