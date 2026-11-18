//! ζ reduction: substituting a let-bound value, and removing a dead trivial let.
//!
//! `let x = y in body` where `y` is a variable or a scalar literal becomes `body` with every read
//! of `x` replaced by `y`: reading a variable or a literal again has no effect, so the binding
//! bought nothing. `let x = v in body` where `x` is never read and `v` is trivial is dropped:
//! making the value is at most an allocation, and nothing reads it.
//!
//! The two rules are the glue between lowering, inlining and the fusions. Lowering binds every
//! source binding to a local and inlining binds every argument to a fresh local, so the IR arrives
//! at this pass as chains of `let` bindings around the work; removing them is what lets the later
//! passes see the shapes inside (a constructor next to its consumer, a `match` on a call the
//! source wrote) and what lets the escape analysis see that an aggregate does not leave its
//! function.
//!
//! # Contract
//!
//! ```text
//! Rule ID: zeta
//! Theory: let calculus (copy propagation) and weakening
//! Equation: let x = v in e → e[x := v]        when v is a variable or a scalar literal
//!           let x = v in e → e                when x is unused and v is trivial
//! Preconditions: for the first form, v is an `Expr::Var` or a scalar literal (`Lit::Int`,
//!                `Lit::Int64`, `Lit::Bool`, `Lit::Unit`, `Lit::Float`), and a variable's
//!                top-level type carries no finalizer (see `has_finalizer`). For the second
//!                form, x has no read in e and v is trivial (subst::is_trivial), so evaluating
//!                it neither has an effect nor can fail, suspend or diverge.
//! Postconditions: the `let` is gone. Every read of the copied variable is the variable or the
//!                 literal it was bound to, and a debug build asserts that none is left; the body
//!                 of a dropped binding is the whole expression. The module stays well typed, and
//!                 a second run of the pipeline changes nothing.
//! Preserves: evaluation order, effects, failures, suspension points, drop points, and the order
//!            and count of the releases of `with`. A copied variable's type is one without a
//!            finalizer, which is what keeps its last drop where it was; a dropped trivial value
//!            at most allocates, so its memory use may change (`[dyn.memory]`).
//! Deopt: a value that is not a variable or a scalar literal (a call, builtin, constructor,
//!        lambda, operator, string literal, …) is not copied; an unused binding whose value is
//!        not trivial (a division or remainder that may raise a defect, `fail`, `assert`, a call
//!        that may have an effect or fail, …) stays. A variable whose top-level type is
//!        `Handler`, `Evidence`, `Cell`, `FiberScope` or `Any` is not copied, because copying it
//!        would move its last drop and so when its finalizer runs; that site is counted in
//!        `RuleStats::deopt`. A string literal is left alone: copying it would repeat its text
//!        at every read.
//! Budget: non-increasing. A copy replaces one read with one variable or literal, and the two
//!         nodes of the removed `let` (`Let` and its value) disappear, so the expression strictly
//!         shrinks; a removed dead binding shrinks it too. Both are asserted in debug builds.
//! Measures: [`crate::opt::Stats::zeta`].
//! ```
//!
//! # Finalizer-bearing variables
//!
//! A variable read has no effect, but it is not always free to move: a value with a finalizer
//! runs that finalizer when its last reference is dropped. `let x = v in e` reads `v` once and
//! reads `x` where `e` does; through `x`, the value's last drop happens at the end of `x`'s
//! scope. Replacing every read of `x` by `v` drops it at the end of `v`'s scope instead, which
//! is a different time. Plan §3.1 preserves drop points, so a `Var` of a top-level type from the
//! finalizer set stays bound. Copying an aggregate is not inspected here: its own drop is a
//! deallocation and the elements' drops are the aggregate's, so a later restriction (or the
//! author's reading of §3.1) decides whether an aggregate whose elements carry a finalizer is a
//! copy value.
//!
//! # Status
//!
//! A2/PR-011 implements both rules on top of the substitution engine of A1/PR-010. The rule runs
//! only when `Rules::zeta` is set, which is off by default until A10/PR-019, so the default
//! pipeline's output does not change yet.

use crate::ir::{Expr, Lit, Module, Type, TypeId};
use crate::opt::{RuleStats, each_body};

use super::subst::{is_trivial, size, subst, use_count};

/// Propagates copies and removes dead bindings in every function body.
///
/// See the module docs for the contract. The pass allocates no locals: a copy reuses the value's
/// variable or literal, so the passes of the same sweep see the names lowering and inlining made.
pub(in crate::opt) fn zeta(m: &mut Module) -> RuleStats {
    let mut stats = RuleStats::default();
    each_body(m, &mut |m, body| rewrite(m, body, &mut stats));
    stats
}

/// Rewrites the `let`s in `e`, children first, and then `e` itself if it is one.
///
/// A `let` is decided only after its value and body are rewritten, so the body the rules read is
/// the final one: a copy inside it has already been propagated away, and a nested dead binding
/// has already been dropped. Each rewrite removes one `let` and adds only variables and literals
/// ([`is_copy_value`]), so the traversal terminates.
fn rewrite(m: &mut Module, e: &mut Expr, stats: &mut RuleStats) {
    crate::tail::for_each_child_mut(e, &mut |child| rewrite(m, child, stats));
    let before = size(e);
    let Expr::Let { var, value, body } = e else {
        return;
    };
    let var = *var;
    let propagated = is_copy_value(m, value);
    let dead = is_trivial(value) && use_count(body, var) == 0;
    if !propagated && !dead {
        // A copy the rule had to refuse is a deopt the reader should see; a value that is simply
        // not a copy (`let point = Point(…)`) is not a candidate at all.
        if is_finalizer_copy(m, value) {
            stats.deopt += 1;
        }
        return;
    }
    if propagated {
        let with = (**value).clone();
        let done = subst(m, body, var, &with);
        if done.deopt {
            stats.deopt += 1;
            return;
        }
        debug_assert_eq!(
            use_count(body, var),
            0,
            "copy propagation left a read of the local it removed (a compiler bug)"
        );
    }
    // `let x = value; body` evaluated `value` and then `body`. Both rules have established that
    // what `value` did is unobservable: the expression is `body`, wherever the `let` was.
    let body = std::mem::replace(body.as_mut(), Expr::Lit(Lit::Unit));
    let after = size(&body);
    if propagated {
        debug_assert!(
            after <= before,
            "copy propagation grew the expression ({after} > {before} nodes)"
        );
    } else {
        debug_assert!(
            after < before,
            "removing a dead binding did not shrink the expression ({after} >= {before} nodes)"
        );
    }
    *e = body;
    stats.applied += 1;
}

/// Whether `let x = value` may become `value` in place of every read of `x`.
///
/// A scalar literal evaluation has no effect, cannot fail, cannot suspend and cannot raise a
/// defect, so repeating one where a read of `x` was is unobservable. A variable read has the same
/// properties, but it may move the value's last drop, so a variable whose type carries a
/// finalizer is not a copy value ([`has_finalizer`]). A string literal is excluded: copying it
/// would repeat the text at every read, and the duplication is what substitution does.
fn is_copy_value(m: &Module, e: &Expr) -> bool {
    match e {
        Expr::Var(v) => !has_finalizer(m, m.locals[*v].ty),
        Expr::Lit(Lit::Int(_) | Lit::Int64(_) | Lit::Bool(_) | Lit::Unit | Lit::Float(_)) => true,
        _ => false,
    }
}

/// Whether `value` is a variable that [`is_copy_value`] refused for its type.
fn is_finalizer_copy(m: &Module, value: &Expr) -> bool {
    matches!(value, Expr::Var(v) if has_finalizer(m, m.locals[*v].ty))
}

/// Whether a value of `ty` owns a finalizer, or an untracked representation, which the pass must
/// not move.
///
/// `Handler` owns the handler value, `Evidence` the table it points at, `Cell` its contents,
/// `FiberScope` the scope it opens, and `Any` is not tracked by a type at all. Dropping one of
/// them runs that finalizer, so the last drop of such a value is a point the pass keeps where it
/// is (plan §3.1). The check reads the top-level type only; an aggregate's elements are not
/// inspected (see the module docs).
fn has_finalizer(m: &Module, ty: TypeId) -> bool {
    matches!(
        m.ty(ty),
        Type::Handler(_) | Type::Evidence | Type::Cell(_) | Type::FiberScope | Type::Any
    )
}
