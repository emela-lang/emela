//! Constant folding: evaluating an operation on literal operands.
//!
//! An operation whose operands are literals is replaced by the literal it evaluates to, so the
//! operation and its operands disappear from the program. The rewrite is bottom-up: the children
//! of a node are folded first, so a folded literal is visible to its parent and a whole expression
//! of constants folds in one sweep.
//!
//! Integer arithmetic wraps in two's complement (spec `[target.number]`): `2147483647 + 1` is
//! `-2147483648`, and the minimum divided by `-1` is the minimum again. A `Div` or `Rem` whose
//! right operand is the literal zero is never folded, because that is a defect at run time and
//! folding it would remove the defect from the program.
//!
//! Float *arithmetic* is deliberately not folded. A folded `a + b` would have to reproduce IEEE
//! 754 exactly as the target does, including NaN (which is not equal to itself and has a payload
//! a literal cannot carry), the signed zero (where `-0.0 + 0.0` is `+0.0` but source-level
//! equality cannot tell the two zeros apart), and infinities; a compile-time evaluation in the
//! compiler's own `f64` would also tie the result to the host, not the target. Leaving the
//! operation in place keeps the rounding and the special values the run-time operation's. The one
//! Float operation that is folded is unary `Neg`, whose result is exactly the sign-bit flip of its
//! operand, including `-0.0` and NaN. Neither String operations nor `OpTy::Structural` equality is
//! folded: structural equality may become a call of an `Eq` impl during monomorphisation, and a
//! String operation would have to reproduce the written result.
//!
//! # Contract
//!
//! ```text
//! Rule ID: fold (constant folding)
//! Theory: δ reduction and constant evaluation of the builtin operators
//! Equation: op(lit, lit) → lit
//! Preconditions: every operand is a literal; the operation is one of the folded ones in the
//!                postcondition, and its operand type matches the literal's variant.
//! Postconditions: `Add`, `Sub`, `Mul`, `Div` and `Rem` on two Int or two Int64 literals are the
//!                 wrapped literal; the comparisons `Eq`, `Ne`, `Lt`, `Le`, `Gt` and `Ge` on two
//!                 Int or two Int64 literals, and `Eq` / `Ne` on two Bool literals, are the Bool
//!                 literal they evaluate to; `Neg` of an Int, Int64 or Float literal is the
//!                 negated literal (wrapping for the integers, a sign flip for Float); `Not` of a
//!                 Bool literal is the negated Bool. Each rewrite keeps the type. A second run
//!                 changes nothing.
//! Preserves: evaluation order, effects, failures, suspension points, drop points. The operands
//!            are literals, so evaluating them had no effect and could not fail; the operation
//!            itself is total for the folded operators and operands.
//! Deopt: `Div` and `Rem` whose right operand is the literal zero keep the run-time defect; the
//!        site is counted in `RuleStats::deopt`. Float arithmetic (NaN, signed zero, infinities),
//!        String operations and `OpTy::Structural` equality are not candidates and are left
//!        alone; so are Bool orderings (`Lt`, `Le`, `Gt`, `Ge`), because Bool is an enum in
//!        source and has no ordering.
//! Budget: strictly smaller: the operands are literals of one node each, and the result is one
//!         literal; the expression never grows.
//! Measures: [`crate::opt::Stats::constant`].
//! ```

use crate::ir::{BinOp, Expr, Lit, Module, OpTy, UnOp};
use crate::opt::{RuleStats, each_body};

/// Replaces an operation whose operands are literals by its literal result.
pub(in crate::opt) fn fold(m: &mut Module) -> RuleStats {
    let mut stats = RuleStats::default();
    each_body(m, &mut |_, e| rewrite(e, &mut stats));
    stats
}

/// Folds `e` bottom-up: children first, then the node they may have made foldable.
fn rewrite(e: &mut Expr, stats: &mut RuleStats) {
    crate::tail::for_each_child_mut(e, &mut |child| rewrite(child, stats));
    match fold_expr(e) {
        Outcome::Folded(folded) => {
            *e = folded;
            stats.applied += 1;
        }
        Outcome::Deopt => stats.deopt += 1,
        Outcome::Keep => {}
    }
}

/// What [`fold_expr`] decided about one node.
enum Outcome {
    /// The node is the literal it evaluates to.
    Folded(Expr),
    /// The node is a fold candidate whose operands keep a defect; it stays as it is.
    Deopt,
    /// The node is not one the rule folds.
    Keep,
}

/// The value of `e`, when `e` is an operation on literals the rule folds.
fn fold_expr(e: &Expr) -> Outcome {
    match e {
        Expr::Binary { op, ty, lhs, rhs } => fold_binary(*op, *ty, lhs, rhs),
        Expr::Unary { op, ty, operand } => fold_unary(*op, *ty, operand),
        _ => Outcome::Keep,
    }
}

/// `lhs op rhs` on two literals, computed with wrapping for Int and Int64 (spec `[target.number]`).
///
/// A division or remainder by zero is a defect at run time, not a value: it is left in the
/// program, so a program that divides by zero still fails when it is built with the passes on.
fn fold_binary(op: BinOp, ty: OpTy, lhs: &Expr, rhs: &Expr) -> Outcome {
    match (ty, lhs, rhs) {
        (OpTy::Int, Expr::Lit(Lit::Int(a)), Expr::Lit(Lit::Int(b))) => {
            fold_int(op, *a, *b).into_outcome()
        }
        (OpTy::Int64, Expr::Lit(Lit::Int64(a)), Expr::Lit(Lit::Int64(b))) => {
            fold_int64(op, *a, *b).into_outcome()
        }
        (OpTy::Bool, Expr::Lit(Lit::Bool(a)), Expr::Lit(Lit::Bool(b))) => {
            match compare_bool(op, *a, *b) {
                Some(value) => Outcome::Folded(Expr::Lit(Lit::Bool(value))),
                None => Outcome::Keep,
            }
        }
        // Float arithmetic is not folded (NaN, signed zero, infinities); nor is String arithmetic
        // or structural equality. A literal whose variant is not the operation's, which well-typed
        // IR does not have, is also left alone.
        _ => Outcome::Keep,
    }
}

/// The three answers of [`fold_int`] and [`fold_int64`].
enum Value {
    Folded(Lit),
    Deopt,
    Keep,
}

impl Value {
    fn into_outcome(self) -> Outcome {
        match self {
            Value::Folded(lit) => Outcome::Folded(Expr::Lit(lit)),
            Value::Deopt => Outcome::Deopt,
            Value::Keep => Outcome::Keep,
        }
    }
}

/// `a op b` on two Int literals.
fn fold_int(op: BinOp, a: i32, b: i32) -> Value {
    match op {
        BinOp::Add => Value::Folded(Lit::Int(a.wrapping_add(b))),
        BinOp::Sub => Value::Folded(Lit::Int(a.wrapping_sub(b))),
        BinOp::Mul => Value::Folded(Lit::Int(a.wrapping_mul(b))),
        // `wrapping_div` and `wrapping_rem` panic on a zero right side, and the defect must stay,
        // so they are only reached when `b` is not zero. `i32::MIN / -1` wraps to `i32::MIN`.
        BinOp::Div if b == 0 => Value::Deopt,
        BinOp::Rem if b == 0 => Value::Deopt,
        BinOp::Div => Value::Folded(Lit::Int(a.wrapping_div(b))),
        BinOp::Rem => Value::Folded(Lit::Int(a.wrapping_rem(b))),
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            match compare(op, a, b) {
                Some(value) => Value::Folded(Lit::Bool(value)),
                None => Value::Keep,
            }
        }
        BinOp::And | BinOp::Or => Value::Keep,
    }
}

/// `a op b` on two Int64 literals.
fn fold_int64(op: BinOp, a: i64, b: i64) -> Value {
    match op {
        BinOp::Add => Value::Folded(Lit::Int64(a.wrapping_add(b))),
        BinOp::Sub => Value::Folded(Lit::Int64(a.wrapping_sub(b))),
        BinOp::Mul => Value::Folded(Lit::Int64(a.wrapping_mul(b))),
        BinOp::Div if b == 0 => Value::Deopt,
        BinOp::Rem if b == 0 => Value::Deopt,
        // `i64::MIN / -1` wraps to `i64::MIN`, as the target does.
        BinOp::Div => Value::Folded(Lit::Int64(a.wrapping_div(b))),
        BinOp::Rem => Value::Folded(Lit::Int64(a.wrapping_rem(b))),
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            match compare(op, a, b) {
                Some(value) => Value::Folded(Lit::Bool(value)),
                None => Value::Keep,
            }
        }
        BinOp::And | BinOp::Or => Value::Keep,
    }
}

/// The comparison `a op b` of two ordered integers, or `None` for an operator that does not
/// compare.
fn compare<T: PartialOrd>(op: BinOp, a: T, b: T) -> Option<bool> {
    Some(match op {
        BinOp::Eq => a == b,
        BinOp::Ne => a != b,
        BinOp::Lt => a < b,
        BinOp::Le => a <= b,
        BinOp::Gt => a > b,
        BinOp::Ge => a >= b,
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem | BinOp::And | BinOp::Or => {
            return None;
        }
    })
}

/// The comparison of two Bool literals: only equality, because Bool is an enum in source and has
/// no ordering.
fn compare_bool(op: BinOp, a: bool, b: bool) -> Option<bool> {
    Some(match op {
        BinOp::Eq => a == b,
        BinOp::Ne => a != b,
        _ => return None,
    })
}

/// `-x` of a numeric literal (`wrapping_neg` for the integers) and `!x` of a Bool literal.
///
/// Float negation is exact — it is the sign-bit flip, so it also covers `-0.0` and NaN — and is
/// folded even though Float arithmetic is not.
fn fold_unary(op: UnOp, ty: OpTy, operand: &Expr) -> Outcome {
    let folded = match (op, ty, operand) {
        (UnOp::Neg, OpTy::Int, Expr::Lit(Lit::Int(a))) => Lit::Int(a.wrapping_neg()),
        (UnOp::Neg, OpTy::Int64, Expr::Lit(Lit::Int64(a))) => Lit::Int64(a.wrapping_neg()),
        (UnOp::Neg, OpTy::Float, Expr::Lit(Lit::Float(a))) => Lit::Float(-a),
        (UnOp::Not, OpTy::Bool, Expr::Lit(Lit::Bool(b))) => Lit::Bool(!b),
        _ => return Outcome::Keep,
    };
    Outcome::Folded(Expr::Lit(folded))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{int, int64};

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

    /// The minimum divided by `-1` wraps to the minimum, as `i32::wrapping_div` says.
    #[test]
    fn the_int_minimum_divided_by_negative_one_wraps() {
        let e = binary(BinOp::Div, OpTy::Int, int(i32::MIN), int(-1));
        assert!(matches!(
            fold_expr(&e),
            Outcome::Folded(Expr::Lit(Lit::Int(i32::MIN)))
        ));
    }

    /// The same for Int64.
    #[test]
    fn the_int64_minimum_divided_by_negative_one_wraps() {
        let e = binary(BinOp::Div, OpTy::Int64, int64(i64::MIN), int64(-1));
        assert!(matches!(
            fold_expr(&e),
            Outcome::Folded(Expr::Lit(Lit::Int64(i64::MIN)))
        ));
    }

    /// Zero divisors are a defect, not a value: the node stays and the site is a deopt.
    #[test]
    fn a_zero_divisor_is_a_deopt() {
        for op in [BinOp::Div, BinOp::Rem] {
            let e = binary(op, OpTy::Int, int(1), int(0));
            assert!(matches!(fold_expr(&e), Outcome::Deopt));
            let e = binary(op, OpTy::Int64, int64(1), int64(0));
            assert!(matches!(fold_expr(&e), Outcome::Deopt));
        }
    }

    /// `Int64` comparisons fold to a Bool literal, like the `Int` ones.
    #[test]
    fn int64_comparisons_fold() {
        let cases = [
            (BinOp::Lt, 1, 2, true),
            (BinOp::Le, 2, 2, true),
            (BinOp::Gt, 1, 2, false),
            (BinOp::Ge, 2, 3, false),
            (BinOp::Eq, 3, 3, true),
            (BinOp::Ne, 3, 3, false),
        ];
        for (op, a, b, expected) in cases {
            let e = binary(op, OpTy::Int64, int64(a), int64(b));
            assert!(
                matches!(fold_expr(&e), Outcome::Folded(Expr::Lit(Lit::Bool(v))) if v == expected),
                "{op:?} {a} {b}"
            );
        }
    }

    /// Bool has no ordering: only `Eq` and `Ne` are folded.
    #[test]
    fn bool_has_only_equality_comparisons() {
        use crate::build::bool as b;
        assert!(matches!(
            fold_expr(&binary(BinOp::Eq, OpTy::Bool, b(true), b(false))),
            Outcome::Folded(Expr::Lit(Lit::Bool(false)))
        ));
        for op in [BinOp::Lt, BinOp::Le, BinOp::Gt, BinOp::Ge] {
            let e = binary(op, OpTy::Bool, b(true), b(false));
            assert!(matches!(fold_expr(&e), Outcome::Keep), "{op:?}");
        }
    }

    /// Float arithmetic is not a candidate, but a Float sign flip is exact.
    #[test]
    fn float_arithmetic_is_not_folded_and_negation_is() {
        use crate::build::float;
        assert!(matches!(
            fold_expr(&binary(BinOp::Add, OpTy::Float, float(1.5), float(2.5))),
            Outcome::Keep
        ));
        assert!(matches!(
            fold_expr(&unary(UnOp::Neg, OpTy::Float, float(0.0))),
            Outcome::Folded(Expr::Lit(Lit::Float(v))) if v == 0.0 && v.is_sign_negative()
        ));
    }
}
