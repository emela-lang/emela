//! β reduction: calling a lambda that was bound to a variable.
//!
//! # Contract
//!
//! ```text
//! Rule ID: beta
//! Theory: β reduction of the lambda calculus, expressed with a let binding
//! Equation: let f = λp. e in C[f a] → let p0 = a0 in … let pn = an in C[e]
//! Preconditions: f is used exactly once in the whole `let`, that use is exactly the callee of an
//!                indirect call, the call passes one argument per parameter, the call is neither
//!                marked `fails` nor `suspends`, the lambda's type is a function type without the
//!                evidence table and without set type parameters, the lambda does not suspend, and
//!                the expansion fits `MAX_BETA_NODES`.
//! Postconditions: the lambda and the call are gone; the call site is the lambda's body with its
//!                 parameters bound to the arguments; the `let f` binding is removed.
//! Preserves: evaluation order, effects, failures, suspension points, and the order and count of
//!            the releases of `with`. The closure value is no longer made or dropped; that is a
//!            memory effect (spec `[dyn.memory]`), not an observable one, and no other drop point
//!            moves.
//! Deopt: f is used zero times or more than once, f is passed as a value, the lambda captures f
//!        (a recursive lambda), the use is not the callee of a call, another number of arguments
//!        (a partial application), a lambda that takes the evidence table or whose type has set
//!        type parameters, a suspending lambda, a call marked `fails` or `suspends`, or an
//!        expansion over the budget.
//! Budget: an expansion of at most `MAX_BETA_NODES` nodes, so that a call site stays small. The
//!         rewrite removes the binding, the lambda, the call and its callee and adds one `let` per
//!         parameter, so the size grows only if a call has more than four arguments; that case is
//!         still bounded by the budget.
//! Measures: [`crate::opt::Stats::beta`].
//! ```
//!
//! # Evaluation order
//!
//! The arguments are moved from the call to `let` bindings at the same place, one per parameter,
//! outermost for the first: `let p0 = a0; …; let pn = an; e`. Each argument is therefore evaluated
//! once, left to right, before the lambda body, exactly where the call evaluated it (call by
//! value, spec `[dyn.order]`). Nothing is substituted into the body: an argument the body ignores
//! is still evaluated once, and an argument the body reads twice is still evaluated once.
//!
//! # Reused parameter locals
//!
//! The expansion binds the lambda's own parameter locals, rather than fresh ones. That is sound
//! only because the `let f` binding is removed in the same rewrite and `f` was used exactly once,
//! so the lambda value and its parameters survive nowhere else: the expansion is the only binding
//! site of each parameter. A future rule that keeps the lambda (say, to share it between calls)
//! must freshen the parameters first. The debug assertion after the rewrite pins the other half:
//! nothing in the result still reads `f`.
//!
//! # Call flags
//!
//! The reference implementation never inspected [`crate::ir::Expr::Call::fails`] or
//! [`crate::ir::Expr::Call::suspends`]. This port requires both to be false and deopts otherwise.
//! An indirect call may be marked `suspends` while its type over-approximates the callee and the
//! lambda itself does not suspend ([`crate::ir::Expr::Lambda`]); expanding it would remove a
//! suspension marker from the caller's body. A call marked `fails` likewise carries a result check
//! that the expansion does not have. Both flags are conservative over-approximations, so leaving
//! the call alone is always sound, and the tests fix the choice.
//!
//! The budget is β's own (`MAX_BETA_NODES`), not δ's: the two rules land independently and must
//! not share a constant.

use crate::ir::{Callee, Expr, Lit, Local, Module, Type};
use crate::opt::RuleStats;
use crate::opt::reduce::subst;

/// The most nodes one β expansion may have: the lambda's body, one node per parameter binding and
/// the arguments. An expansion that would be larger is abandoned whole, so a later sweep with the
/// same deterministic budget abandons it again and the pass stays idempotent.
const MAX_BETA_NODES: u32 = 128;

/// Rewrites a linear `let f = λp.e in C[f a]` into `let p0 = a0; …; let pn = an; C[e]`.
///
/// See the module documentation for the contract. Children are rewritten first, so a `let` nested
/// in a value or a body is reduced before its parent.
pub(in crate::opt) fn beta(m: &mut Module) -> RuleStats {
    let mut stats = RuleStats::default();
    crate::opt::each_body(m, &mut |m, body| beta_expr(m, body, &mut stats));
    stats
}

/// Rewrites every `let` of `e`, children first.
fn beta_expr(m: &Module, e: &mut Expr, stats: &mut RuleStats) {
    crate::tail::for_each_child_mut(e, &mut |child| beta_expr(m, child, stats));
    if matches!(e, Expr::Let { .. }) {
        beta_let(m, e, stats);
    }
}

/// The rewrite of one `let`, after [`beta_expr`] has rewritten its children.
///
/// Kept out of [`beta_expr`] so that the rewrite's locals do not grow the frame of every level of
/// the walk: a program with many suspension points is a deep chain of `let`s.
fn beta_let(m: &Module, e: &mut Expr, stats: &mut RuleStats) {
    // A `let` whose value is not a lambda is not a β candidate and is not a deopt: it belongs to
    // ζ or to the source.
    let Expr::Let { value, .. } = e else {
        return;
    };
    if !matches!(&**value, Expr::Lambda { .. }) {
        return;
    }
    // From here the `let` binds a lambda: a candidate. A precondition that fails is a deopt.
    if !fits(m, e) {
        stats.deopt += 1;
        return;
    }
    // The shape is known, so the `let` can be taken apart. The lambda's parameter locals are
    // unique in the module and the binding is about to disappear, so the expansion is their only
    // binding site (see "Reused parameter locals").
    let Expr::Let {
        var: f,
        value,
        mut body,
    } = std::mem::replace(e, Expr::Lit(Lit::Unit))
    else {
        unreachable!("the shape was checked")
    };
    let Expr::Lambda {
        params,
        body: lambda_body,
        ..
    } = *value
    else {
        unreachable!("the shape was checked")
    };
    let Expr::Call { args, .. } =
        beta_call(&body, f).expect("the one use of the local is a call's callee")
    else {
        unreachable!("`beta_call` found a call")
    };
    // Arguments are bound left to right before the body. Building the chain in reverse makes the
    // first parameter the outermost `let`, which is what evaluates its argument first.
    let mut expanded = *lambda_body;
    for (param, arg) in params.into_iter().zip(args.to_vec()).rev() {
        expanded = Expr::Let {
            var: param,
            value: Box::new(arg),
            body: Box::new(expanded),
        };
    }
    let mut expansion = Some(expanded);
    let replaced = replace_beta_call(&mut body, f, &mut expansion);
    debug_assert!(replaced, "the call site was found by `fits`");
    // Nothing else can use `f`: the lambda does not capture it, so dropping the binding only drops
    // the closure value, which has no finalizer; that is a memory effect no program can observe
    // (spec `[dyn.memory]`).
    debug_assert_eq!(
        subst::use_count(&body, f),
        0,
        "β inlined a lambda whose local is still used"
    );
    *e = *body;
    stats.applied += 1;
}

/// Whether the `let` at `e` has the shape of β and its expansion is safe and within budget.
///
/// The caller has already checked that the value is a lambda, so `false` is a deopt.
fn fits(m: &Module, e: &Expr) -> bool {
    let Expr::Let {
        var: f,
        value,
        body,
    } = e
    else {
        return false;
    };
    let Expr::Lambda {
        params,
        ty,
        body: lambda_body,
        suspends: false,
    } = &**value
    else {
        return false;
    };
    let Type::Fn(ft) = m.ty(*ty) else {
        return false;
    };
    // A lambda that takes the evidence table has one more argument at the call than its type
    // lists; a type with set type parameters would need substitution. Both are left alone.
    if ft.ev || ft.sets.is_some() {
        return false;
    }
    // The type and the lambda must agree exactly, so the expansion keeps the module well typed
    // with the lambda's own parameter locals.
    if ft.params.len() != params.len()
        || params
            .iter()
            .zip(&ft.params)
            .any(|(param, ty)| m.locals[*param].ty != *ty)
    {
        return false;
    }
    // Exactly one use in the whole `let`. That rules out passing the lambda as a value, calling it
    // twice, and a lambda that reads its own binding.
    if subst::use_count(e, *f) != 1 {
        return false;
    }
    let Some(call) = beta_call(body, *f) else {
        return false;
    };
    let Expr::Call {
        args,
        fails,
        suspends,
        ..
    } = call
    else {
        unreachable!("`beta_call` found a call")
    };
    // The expansion has no call, so it has no result check and no suspension marker to move.
    if *fails || *suspends {
        return false;
    }
    // A partial application (or a call the type checker would reject) is left alone.
    if args.len() != params.len() {
        return false;
    }
    let size =
        subst::size(lambda_body) + params.len() as u32 + args.iter().map(subst::size).sum::<u32>();
    size <= MAX_BETA_NODES
}

/// The call in `e` whose callee is exactly `Var(f)`, in pre-order.
///
/// The traversal order is not evaluation order at a `with` ([`crate::tail::for_each_child_mut`]),
/// but there is exactly one use of `f` in the whole `let`, so the first and only candidate is the
/// use. Searching `body` only (never the lambda value) is what makes a lambda that captures `f`
/// a deopt instead of a self-application.
fn beta_call(e: &Expr, f: Local) -> Option<&Expr> {
    if let Expr::Call {
        callee: Callee::Indirect(callee),
        ..
    } = e
        && matches!(&**callee, Expr::Var(v) if *v == f)
    {
        return Some(e);
    }
    let mut found = None;
    e.for_each_child(&mut |child| {
        if found.is_none() {
            found = beta_call(child, f);
        }
    });
    found
}

/// Replaces the one call to `f` in `e` by `expansion`. Returns whether it found the call.
fn replace_beta_call(e: &mut Expr, f: Local, expansion: &mut Option<Expr>) -> bool {
    if let Expr::Call {
        callee: Callee::Indirect(callee),
        ..
    } = e
        && matches!(&**callee, Expr::Var(v) if *v == f)
    {
        *e = expansion.take().expect("one call site");
        return true;
    }
    let mut replaced = false;
    crate::tail::for_each_child_mut(e, &mut |child| {
        if !replaced {
            replaced = replace_beta_call(child, f, expansion);
        }
    });
    replaced
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{FnId, FnSets, FnType, TypeId};

    /// Defines `body` as the body of a parameterless function `g` and returns its id.
    fn define(m: &mut Module, body: Expr) -> FnId {
        let g = m.declare("g", false);
        m.define(g, Vec::new(), TypeId::INT, body);
        g
    }

    /// `let f = |params| lambda_body; call`, where `call` is an indirect call to `f`.
    fn lambda_let(
        m: &mut Module,
        params: Vec<Local>,
        ty: TypeId,
        lambda_body: Expr,
        args: Vec<Expr>,
        fails: bool,
        suspends: bool,
    ) -> Expr {
        let f = m.local("f", ty);
        Expr::Let {
            var: f,
            value: Box::new(Expr::Lambda {
                params,
                ty,
                body: Box::new(lambda_body),
                suspends: false,
            }),
            body: Box::new(Expr::Call {
                callee: Callee::Indirect(Box::new(Expr::Var(f))),
                args,
                fails,
                suspends,
            }),
        }
    }

    /// `x + 1`.
    fn add_one(x: Local) -> Expr {
        Expr::Binary {
            op: crate::ir::BinOp::Add,
            ty: crate::ir::OpTy::Int,
            lhs: Box::new(Expr::Var(x)),
            rhs: Box::new(Expr::Lit(Lit::Int(1))),
        }
    }

    /// The one entry point used by the tests: a lambda with one `Int` parameter.
    fn one_param(m: &mut Module) -> (Local, TypeId) {
        let x = m.local("x", TypeId::INT);
        let ty = m.fn_type(vec![TypeId::INT], TypeId::INT);
        (x, ty)
    }

    /// One use as the callee: the `let` is gone and the call site is the body.
    #[test]
    fn one_use_as_callee_expands() {
        let mut m = Module::new();
        let (x, ty) = one_param(&mut m);
        let body = lambda_let(
            &mut m,
            vec![x],
            ty,
            add_one(x),
            vec![Expr::Lit(Lit::Int(2))],
            false,
            false,
        );
        let g = define(&mut m, body);
        let stats = beta(&mut m);
        assert_eq!(
            stats,
            RuleStats {
                applied: 1,
                deopt: 0
            }
        );
        let Expr::Let { var, value, body } = &m.functions[g].body else {
            panic!("the argument is bound at the call site")
        };
        assert_eq!(*var, x);
        assert_eq!(**value, Expr::Lit(Lit::Int(2)));
        assert!(matches!(**body, Expr::Binary { .. }));
        assert_eq!(subst::use_count(&m.functions[g].body, x), 1);
        assert_eq!(crate::verify::verify(&m), Ok(()));
    }

    /// A call node marked `fails` or `suspends` is left alone, even when its lambda is pure: the
    /// expansion would drop a result check or a suspension marker.
    #[test]
    fn marked_calls_deopt() {
        for (fails, suspends) in [(true, false), (false, true)] {
            let mut m = Module::new();
            let (x, ty) = one_param(&mut m);
            let body = lambda_let(
                &mut m,
                vec![x],
                ty,
                add_one(x),
                vec![Expr::Lit(Lit::Int(2))],
                fails,
                suspends,
            );
            let g = define(&mut m, body);
            let before = m.functions[g].body.clone();
            let stats = beta(&mut m);
            assert_eq!(
                stats,
                RuleStats {
                    applied: 0,
                    deopt: 1
                },
                "fails: {fails}, suspends: {suspends}"
            );
            assert_eq!(m.functions[g].body, before, "a deopt changes nothing");
        }
    }

    /// Two parameters and one argument: a partial application is not expanded.
    #[test]
    fn partial_application_deopts() {
        let mut m = Module::new();
        let a = m.local("a", TypeId::INT);
        let b = m.local("b", TypeId::INT);
        let ty = m.fn_type(vec![TypeId::INT, TypeId::INT], TypeId::INT);
        let body = lambda_let(
            &mut m,
            vec![a, b],
            ty,
            add_one(a),
            vec![Expr::Lit(Lit::Int(1))],
            false,
            false,
        );
        let g = define(&mut m, body);
        let before = m.functions[g].body.clone();
        let stats = beta(&mut m);
        assert_eq!(
            stats,
            RuleStats {
                applied: 0,
                deopt: 1
            }
        );
        assert_eq!(m.functions[g].body, before);
    }

    /// A lambda that reads its own binding has two uses of `f`: expanding the call would leave the
    /// closure and bind the parameter locals twice.
    #[test]
    fn recursive_lambda_deopts() {
        let mut m = Module::new();
        let f_ty = m.fn_type(Vec::new(), TypeId::INT);
        let f = m.local("f", f_ty);
        let self_call = Expr::Call {
            callee: Callee::Indirect(Box::new(Expr::Var(f))),
            args: Vec::new(),
            fails: false,
            suspends: false,
        };
        let body = Expr::Let {
            var: f,
            value: Box::new(Expr::Lambda {
                params: Vec::new(),
                ty: f_ty,
                body: Box::new(self_call.clone()),
                suspends: false,
            }),
            body: Box::new(self_call),
        };
        let g = define(&mut m, body);
        let before = m.functions[g].body.clone();
        let stats = beta(&mut m);
        assert_eq!(
            stats,
            RuleStats {
                applied: 0,
                deopt: 1
            }
        );
        assert_eq!(m.functions[g].body, before);
    }

    /// A lambda type that takes the evidence table is left alone.
    #[test]
    fn an_evidence_lambda_deopts() {
        let mut m = Module::new();
        let x = m.local("x", TypeId::INT);
        let ty = m.intern(Type::Fn(FnType {
            params: vec![TypeId::INT],
            ret: TypeId::INT,
            fails: false,
            suspends: false,
            ev: true,
            sets: None,
        }));
        let body = lambda_let(
            &mut m,
            vec![x],
            ty,
            add_one(x),
            vec![Expr::Lit(Lit::Int(1))],
            false,
            false,
        );
        define(&mut m, body);
        assert_eq!(
            beta(&mut m),
            RuleStats {
                applied: 0,
                deopt: 1
            }
        );
    }

    /// A lambda type with set type parameters is left alone: the expansion would have to
    /// substitute them.
    #[test]
    fn a_set_type_lambda_deopts() {
        let mut m = Module::new();
        let x = m.local("x", TypeId::INT);
        let ty = m.intern(Type::Fn(FnType {
            params: vec![TypeId::INT],
            ret: TypeId::INT,
            fails: false,
            suspends: false,
            ev: false,
            sets: Some(FnSets {
                errors: TypeId::NEVER,
                effects: TypeId::NEVER,
            }),
        }));
        let body = lambda_let(
            &mut m,
            vec![x],
            ty,
            add_one(x),
            vec![Expr::Lit(Lit::Int(1))],
            false,
            false,
        );
        define(&mut m, body);
        assert_eq!(
            beta(&mut m),
            RuleStats {
                applied: 0,
                deopt: 1
            }
        );
    }

    /// An expansion bigger than the budget is abandoned whole, so the pass is idempotent.
    #[test]
    fn an_expansion_over_budget_deopts() {
        let mut m = Module::new();
        let (x, ty) = one_param(&mut m);
        // A tuple of enough literals to exceed `MAX_BETA_NODES` on its own.
        let big = Expr::Tuple(vec![Expr::Lit(Lit::Int(1)); MAX_BETA_NODES as usize]);
        let body = lambda_let(
            &mut m,
            vec![x],
            ty,
            big,
            vec![Expr::Lit(Lit::Int(1))],
            false,
            false,
        );
        let g = define(&mut m, body);
        let before = m.functions[g].body.clone();
        let stats = beta(&mut m);
        assert_eq!(
            stats,
            RuleStats {
                applied: 0,
                deopt: 1
            }
        );
        assert_eq!(m.functions[g].body, before);
    }

    /// A `let` whose value is not a lambda is not a candidate, so it is not counted as a deopt.
    #[test]
    fn a_plain_let_is_not_a_deopt() {
        let mut m = Module::new();
        let x = m.local("x", TypeId::INT);
        let body = Expr::Let {
            var: x,
            value: Box::new(Expr::Lit(Lit::Int(1))),
            body: Box::new(add_one(x)),
        };
        define(&mut m, body);
        assert_eq!(beta(&mut m), RuleStats::default());
    }

    /// The rewrite removes the binding, the lambda and the call and adds one `let` per parameter,
    /// so it does not grow the program.
    #[test]
    fn size_is_non_increasing() {
        let mut m = Module::new();
        let (x, ty) = one_param(&mut m);
        let body = lambda_let(
            &mut m,
            vec![x],
            ty,
            add_one(x),
            vec![Expr::Lit(Lit::Int(2))],
            false,
            false,
        );
        let g = define(&mut m, body);
        let before = subst::size(&m.functions[g].body);
        assert_eq!(beta(&mut m).applied, 1);
        let after = subst::size(&m.functions[g].body);
        assert!(after <= before, "before {before}, after {after}");
    }
}
