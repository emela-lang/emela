//! Substitution: replacing a local by a value expression.
//!
//! Substitution is shared by the let-level rules (`zeta`, `beta`, `iota`, `delta`). It is not a
//! rule of its own: it has no counter and never walks a whole module by itself.
//!
//! # Contract
//!
//! ```text
//! Rule ID: subst (helper, not a rule)
//! Theory: substitution in the let calculus, with capture avoidance and fresh locals
//! Equation: e[x := v]
//! Preconditions: v is trivial (a literal, a variable read or a function reference); x occurs in e.
//! Postconditions: every free occurrence of x in e is v, and no other local changed.
//! Preserves: evaluation order, effects, failures, suspension points, drop points.
//! Deopt: v is not trivial, so moving it to an occurrence could move, duplicate or drop its
//!        evaluation; or a fresh local would be needed (not implemented in this skeleton).
//! Budget: the size of the expression substituted, so a rule can refuse an unbounded copy.
//! Measures: none of its own; the calling rule counts what it rewrote.
//! ```
//!
//! # Status
//!
//! A0/PR-002 ships the API and a deliberately conservative body. It only substitutes a *trivial*
//! value, which may be read any number of times without repeating an effect. It never allocates a
//! fresh local and never renames a binder, so a non-trivial value deopts instead of moving.
//! A1/PR-010 completes the engine: capture-avoiding renaming, fresh locals for copied binders, a
//! use count and a size measure that the `beta` and `delta` budgets read.

use crate::ir::{Expr, Local, Module};

/// What one [`subst`] call changed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Subst {
    /// Occurrences of the variable replaced.
    pub replaced: u32,
    /// Fresh locals allocated for copied binders. Always zero until A1/PR-010.
    pub fresh: u32,
    /// Whether the call was abandoned, leaving the expression unchanged.
    pub deopt: bool,
}

/// Whether `e` may be evaluated more than once without changing the observable behaviour.
///
/// A literal, a variable read and a function reference have no effect, cannot fail and cannot
/// diverge, so substituting one for each use keeps evaluation order and effect count. Any other
/// expression is not trivial: evaluating it later or more than once can move, repeat or drop its
/// effects.
pub fn is_trivial(e: &Expr) -> bool {
    matches!(e, Expr::Lit(_) | Expr::Var(_) | Expr::Fn { .. })
}

/// The number of free occurrences of `var` in `e`.
///
/// Locals are unique in a module (`crate::ir`), so a binder of the same [`Local`] cannot occur
/// inside `e`; every occurrence is free.
pub fn use_count(e: &Expr, var: Local) -> u32 {
    let mut count = 0;
    count_uses(e, var, &mut count);
    count
}

fn count_uses(e: &Expr, var: Local, count: &mut u32) {
    if matches!(e, Expr::Var(v) if *v == var) {
        *count += 1;
        return;
    }
    e.for_each_child(&mut |child| count_uses(child, var, count));
}

/// Replaces every occurrence of `var` in `e` by `value`, when that is conservative and sound.
///
/// `m` is the module that owns the locals; A1/PR-010 uses it to allocate fresh locals when it
/// copies a binder. The skeleton does not allocate, so it leaves `m` untouched.
///
/// The call deopts (`Subst::deopt`, expression unchanged) when `value` is not [`is_trivial`]. A
/// trivial value can be copied to every use, so a variable with several uses is handled. With no
/// occurrence the call changes nothing and does not deopt: whether to remove the binding (the
/// dead-let half of ζ) is the calling rule's decision.
pub fn subst(_m: &mut Module, e: &mut Expr, var: Local, value: &Expr) -> Subst {
    if !is_trivial(value) {
        return Subst {
            deopt: true,
            ..Subst::default()
        };
    }
    if use_count(e, var) == 0 {
        return Subst::default();
    }
    let mut replaced = 0;
    replace(e, var, value, &mut replaced);
    Subst {
        replaced,
        fresh: 0,
        deopt: false,
    }
}

fn replace(e: &mut Expr, var: Local, value: &Expr, replaced: &mut u32) {
    if matches!(e, Expr::Var(v) if *v == var) {
        *e = value.clone();
        *replaced += 1;
        return;
    }
    crate::tail::for_each_child_mut(e, &mut |child| replace(child, var, value, replaced));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Lit, TypeId};

    fn var_expr(local: Local) -> Expr {
        Expr::Var(local)
    }

    /// A literal is copied to every use, and the count is reported.
    #[test]
    fn a_trivial_value_replaces_every_use() {
        let mut m = Module::new();
        let x = m.local("x", TypeId::INT);
        let y = m.local("y", TypeId::INT);
        let value = Expr::Lit(Lit::Int(3));
        let mut e = Expr::Binary {
            op: crate::ir::BinOp::Add,
            ty: crate::ir::OpTy::Int,
            lhs: Box::new(var_expr(x)),
            rhs: Box::new(Expr::Binary {
                op: crate::ir::BinOp::Mul,
                ty: crate::ir::OpTy::Int,
                lhs: Box::new(var_expr(x)),
                rhs: Box::new(var_expr(y)),
            }),
        };
        assert_eq!(use_count(&e, x), 2);
        let done = subst(&mut m, &mut e, x, &value);
        assert_eq!(
            done,
            Subst {
                replaced: 2,
                fresh: 0,
                deopt: false
            }
        );
        assert_eq!(use_count(&e, x), 0);
        assert_eq!(use_count(&e, y), 1);
    }

    /// A variable read is trivial too.
    #[test]
    fn a_variable_read_is_trivial() {
        let mut m = Module::new();
        let x = m.local("x", TypeId::INT);
        let y = m.local("y", TypeId::INT);
        let mut e = Expr::Var(x);
        let done = subst(&mut m, &mut e, x, &Expr::Var(y));
        assert_eq!(done.replaced, 1);
        assert_eq!(e, Expr::Var(y));
    }

    /// A value with an effect is not moved: the expression and the module stay as they were.
    #[test]
    fn a_non_trivial_value_deopts_without_changing_the_expression() {
        let mut m = Module::new();
        let x = m.local("x", TypeId::INT);
        let mut e = Expr::Var(x);
        let before = e.clone();
        // A call is not trivial, whatever the callee is.
        let f = m.declare("f", false);
        let value = Expr::Call {
            callee: crate::ir::Callee::Direct {
                f,
                ty_args: Vec::new(),
            },
            args: Vec::new(),
            fails: false,
            suspends: false,
        };
        let done = subst(&mut m, &mut e, x, &value);
        assert!(done.deopt);
        assert_eq!(done.replaced, 0);
        assert_eq!(e, before);
    }

    /// No occurrence is not a deopt: the caller decides whether the binding may go.
    #[test]
    fn a_missing_variable_changes_nothing() {
        let mut m = Module::new();
        let x = m.local("x", TypeId::INT);
        let y = m.local("y", TypeId::INT);
        let mut e = Expr::Var(y);
        let done = subst(&mut m, &mut e, x, &Expr::Lit(Lit::Unit));
        assert_eq!(done, Subst::default());
        assert_eq!(e, Expr::Var(y));
    }
}
