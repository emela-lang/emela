//! η reduction: removing a lambda that only forwards its parameters to a call.
//!
//! A lambda `fn(x…) { f(x…) }` is the identity on `f` as far as a caller can tell: it passes its
//! arguments to `f` in the same order and returns what `f` returns. Replacing the lambda by the
//! function value `f` removes a closure, one indirect call per call of it, and the capture of every
//! local the wrapper would otherwise hold. The reduction applies only when the body is exactly the
//! call and the arguments are the lambda's parameters, in order, so no argument is dropped,
//! reordered, added or captured.
//!
//! A lambda is a value, so making one has no effect; the call inside it happens only when the value
//! is called, and the value substituted for it calls the same function with the same arguments.
//! The wrapper's type must be exactly the value's type: [`crate::verify`] accepts a function type
//! that is compatible (a narrower parameter, a wider result), but the call still has to follow the
//! calling convention of the value's type (whether it takes the evidence table, whether it may
//! fail or suspend). A type that only matches up to compatibility is left alone.
//!
//! # Contract
//!
//! ```text
//! Rule ID: eta
//! Theory: η conversion of the lambda calculus
//! Equation: λx. f x → f
//! Preconditions: the body is exactly a call whose arguments are the lambda's parameters, in
//!                order (nothing dropped, reordered, added or captured); the lambda's function
//!                type does not take the evidence table and has no set type parameters; the
//!                lambda does not suspend; the call is direct to a non-external, non-const,
//!                non-suspending function whose instantiated type is exactly the lambda's, or
//!                indirect through a local of exactly the lambda's type that does not suspend.
//! Postconditions: the lambda is the function value (`Expr::Fn`) or the local (`Expr::Var`); a
//!                 second run of the pipeline changes nothing.
//! Preserves: evaluation order, effects, failures, suspension points, and the order and count of
//!            the releases of `with`. Reference-count drop points and memory use may change
//!            (`[dyn.memory]`).
//! Deopt: a lambda that takes the evidence table or has set type parameters, suspends, drops,
//!        reorders, adds or captures an argument (partial application included), whose body is
//!        not the call, or whose type is not exactly the callee's; an external, const or
//!        suspending callee; a call through anything but a local of exactly the lambda's type.
//! Budget: strictly decreasing; the lambda and its call become one function value, which the
//!         debug build checks against the size measure.
//! Measures: [`crate::opt::Stats::eta`], and [`crate::opt::RuleStats::deopt`] for a forwarding
//!           candidate the rule leaves alone; the allocation counts of the programs that pass a
//!           call wrapper to a higher-order function (`List.map(xs, |x| f(x))`).
//! ```
//!
//! A deopt is counted only for a lambda whose body is a call: that is the shape of a wrapper, so
//! its failure says something about the rule. A lambda with any other body (`|x| x + 1`) is not a
//! candidate and is not counted.
//!
//! η runs after every shrinking pass: the fusions, inlining, ι and the algebraic laws. Only δ may
//! follow it (`run_with` keeps δ last because it is the one rule that may grow the program). The
//! range fusion copies a lambda into the loop it makes, and that is only possible while the lambda
//! is still a lambda: η would turn it into a function value, and the copied body (straight-line
//! code in the loop) is what the LIR and the target compiler can look through. The algebraic laws
//! remove forwarding bodies and so leave more wrapper lambdas for η to see.

use crate::ir::{Callee, Expr, FnId, FnType, Module, Type, TypeId};
use crate::opt::{RuleStats, each_body};

/// Replaces a call wrapper `λx…. f(x…)` by the function value `f`.
///
/// See the module docs for the contract.
pub(in crate::opt) fn eta(m: &mut Module) -> RuleStats {
    let mut applied = 0;
    let mut deopt = 0;
    each_body(m, &mut |m, e| rewrite(m, e, &mut applied, &mut deopt));
    RuleStats { applied, deopt }
}

/// Rewrites the call wrappers in `e`, children first, and then `e` itself.
fn rewrite(m: &mut Module, e: &mut Expr, applied: &mut u32, deopt: &mut u32) {
    crate::tail::for_each_child_mut(e, &mut |c| rewrite(m, c, applied, deopt));
    if let Some(value) = reduction(m, e, deopt) {
        debug_assert!(
            size(&value) < size(e),
            "η must strictly shrink the expression (a compiler bug)"
        );
        *e = value;
        *applied += 1;
    }
}

/// What `e` reduces to, if it is a call wrapper whose type lets it.
///
/// A lambda whose body is a call is a forwarding candidate; when the rest of the preconditions
/// fails, the site is counted in `deopt`.
fn reduction(m: &mut Module, e: &Expr, deopt: &mut u32) -> Option<Expr> {
    let Expr::Lambda {
        params,
        ty,
        body,
        suspends,
    } = e
    else {
        return None;
    };
    let Expr::Call { callee, args, .. } = &**body else {
        return None;
    };
    // From here the body is a call, so the lambda is a wrapper that η must decide about.
    if *suspends {
        *deopt += 1;
        return None;
    }
    let Type::Fn(ft) = m.ty(*ty) else {
        *deopt += 1;
        return None;
    };
    // A set type parameter makes the flags an over-approximation of every instantiation (the
    // evidence table would be passed where the instantiation does not need it), and the evidence
    // table is a calling convention the value's type has to state exactly.
    if ft.ev || ft.sets.is_some() {
        *deopt += 1;
        return None;
    }
    let ft = ft.clone();
    if args.len() != params.len() {
        *deopt += 1;
        return None;
    }
    // Every argument is the matching parameter, in order: the wrapper is the identity on them.
    if !args
        .iter()
        .zip(params)
        .all(|(arg, &param)| matches!(arg, Expr::Var(l) if *l == param))
    {
        *deopt += 1;
        return None;
    }
    let value = match callee {
        Callee::Direct { f, ty_args } => reduction_direct(m, *f, ty_args, &ft),
        Callee::Indirect(callee) => {
            let Expr::Var(g) = &**callee else {
                *deopt += 1;
                return None;
            };
            // The local replaces the lambda as a value, so it must have the lambda's type. Its
            // type decides how a call passes arguments, including the evidence table.
            if m.locals[*g].ty != *ty {
                *deopt += 1;
                return None;
            }
            let Type::Fn(g_ft) = m.ty(m.locals[*g].ty) else {
                *deopt += 1;
                return None;
            };
            if g_ft.suspends || g_ft.sets.is_some() {
                *deopt += 1;
                return None;
            }
            Some(Expr::Var(*g))
        }
        Callee::Op { .. } => None,
    };
    if value.is_none() {
        *deopt += 1;
    }
    value
}

/// The function value for a direct call to `f`, if the instantiated function has exactly `ft`.
fn reduction_direct(m: &mut Module, f: FnId, ty_args: &[TypeId], ft: &FnType) -> Option<Expr> {
    let (params, ret, fails, suspends, ev, type_params, external, constant, sets) = {
        let func = &m.functions[f];
        (
            func.params.clone(),
            func.ret,
            func.fails,
            func.suspends,
            m.takes_ev(f),
            func.type_params,
            func.external.is_some(),
            func.constant,
            func.sets.is_some(),
        )
    };
    // A const has its own evaluation rule, an external function is the host's, and `suspends`
    // covers both the wrapper's `suspends: false` and the value's non-suspending type. `sets`
    // makes the flags over-approximate the instantiation, as in [`reduction`].
    if external || constant || suspends || ev || sets || ty_args.len() != type_params {
        return None;
    }
    let params: Vec<TypeId> = params
        .iter()
        .map(|&p| m.locals[p].ty)
        .map(|t| m.types.subst(t, ty_args))
        .collect();
    let ret = m.types.subst(ret, ty_args);
    let value = FnType {
        params,
        ret,
        fails,
        suspends,
        ev,
        sets: None,
    };
    if &value != ft {
        return None;
    }
    Some(Expr::Fn {
        f,
        ty_args: ty_args.to_vec(),
    })
}

/// The number of nodes in `e`, for the size check of the contract.
///
/// Not gated by `debug_assertions`: the expression of a `debug_assert!` is type checked in every
/// profile, so a release build needs the function to exist even though it never calls it.
fn size(e: &Expr) -> usize {
    let mut count = 1;
    e.for_each_child(&mut |c| count += size(c));
    count
}

#[cfg(test)]
mod tests {
    use super::eta;
    use crate::ir::{Callee, Expr, FnId, FnSets, FnType, Module, Type, TypeId};

    /// A module with `f`, an `Int` → `Int` function whose body is `x`, and `main`, whose body is
    /// the lambda `|x| f(x)`. `fails` sets whether `f` may fail. Returns the module and `main`.
    fn wrapper_module(fails: bool) -> (Module, FnId) {
        let mut m = Module::new();
        let f = m.declare("f", false);
        let p = m.local("x", TypeId::INT);
        m.define(f, vec![p], TypeId::INT, Expr::Var(p));
        m.functions[f].fails = fails;

        let main = m.declare("main", true);
        let x = m.local("x", TypeId::INT);
        let ty = m.fn_type(vec![TypeId::INT], TypeId::INT);
        let body = Expr::Lambda {
            params: vec![x],
            ty,
            body: Box::new(Expr::Call {
                callee: Callee::Direct {
                    f,
                    ty_args: Vec::new(),
                },
                args: vec![Expr::Var(x)],
                fails: false,
                suspends: false,
            }),
            suspends: false,
        };
        m.define(main, Vec::new(), TypeId::UNIT, body);
        (m, main)
    }

    /// An exact match reduces the wrapper to the function value.
    #[test]
    fn an_exact_type_reduces() {
        let (mut m, main) = wrapper_module(false);
        let stats = eta(&mut m);
        assert_eq!(stats.applied, 1);
        assert_eq!(stats.deopt, 0);
        assert!(matches!(m.functions[main].body, Expr::Fn { .. }));
    }

    /// `verify` accepts a wider function type, but the value's type decides the calling convention
    /// (`fails`), so a lambda whose type says it cannot fail is left alone when `f` can.
    #[test]
    fn a_type_that_only_matches_up_to_compatibility_does_not_reduce() {
        let (mut m, main) = wrapper_module(true);
        let stats = eta(&mut m);
        assert_eq!(stats.applied, 0);
        assert_eq!(stats.deopt, 1);
        assert!(matches!(m.functions[main].body, Expr::Lambda { .. }));
    }

    /// A call through a local of exactly the lambda's non-suspending type reduces to the local.
    #[test]
    fn indirect_through_a_local_reduces() {
        let mut m = Module::new();
        let main = m.declare("main", true);
        let ty = m.func(FnType::pure(vec![TypeId::INT], TypeId::INT));
        let g = m.local("g", ty);
        let x = m.local("x", TypeId::INT);
        let body = Expr::Lambda {
            params: vec![x],
            ty,
            body: Box::new(Expr::Call {
                callee: Callee::Indirect(Box::new(Expr::Var(g))),
                args: vec![Expr::Var(x)],
                fails: false,
                suspends: false,
            }),
            suspends: false,
        };
        m.define(main, Vec::new(), TypeId::UNIT, body);
        let stats = eta(&mut m);
        assert_eq!(stats.applied, 1);
        assert!(matches!(m.functions[main].body, Expr::Var(l) if l == g));
    }

    /// A call through anything but a local (here a nested lambda) is left alone.
    #[test]
    fn indirect_through_anything_else_does_not_reduce() {
        let mut m = Module::new();
        let main = m.declare("main", true);
        let ty = m.func(FnType::pure(vec![TypeId::INT], TypeId::INT));
        let x = m.local("x", TypeId::INT);
        let y = m.local("y", TypeId::INT);
        let inner = Expr::Lambda {
            params: vec![y],
            ty,
            body: Box::new(Expr::Var(y)),
            suspends: false,
        };
        let body = Expr::Lambda {
            params: vec![x],
            ty,
            body: Box::new(Expr::Call {
                callee: Callee::Indirect(Box::new(inner)),
                args: vec![Expr::Var(x)],
                fails: false,
                suspends: false,
            }),
            suspends: false,
        };
        m.define(main, Vec::new(), TypeId::UNIT, body);
        let stats = eta(&mut m);
        assert_eq!(stats.applied, 0);
        assert_eq!(stats.deopt, 1);
    }

    /// A suspending lambda is a suspension point of its own; its value is not the callee's.
    #[test]
    fn a_suspending_lambda_does_not_reduce() {
        let (mut m, main) = wrapper_module(false);
        let Expr::Lambda { suspends, .. } = &mut m.functions[main].body else {
            panic!("the wrapper is a lambda");
        };
        *suspends = true;
        let stats = eta(&mut m);
        assert_eq!(stats.applied, 0);
        assert_eq!(stats.deopt, 1);
        assert!(matches!(m.functions[main].body, Expr::Lambda { .. }));
    }

    /// A lambda whose type takes the evidence table or has set type parameters is left alone.
    #[test]
    fn ev_and_sets_do_not_reduce() {
        let sets = FnSets {
            errors: TypeId::UNIT,
            effects: TypeId::UNIT,
        };
        for (ev, sets) in [(true, None), (false, Some(sets))] {
            let (mut m, main) = wrapper_module(false);
            let ty = m.fn_type(vec![TypeId::INT], TypeId::INT);
            let Type::Fn(ft) = m.ty(ty).clone() else {
                panic!("the wrapper's type is a function type");
            };
            let ft = FnType { ev, sets, ..ft };
            let ty = m.func(ft);
            let Expr::Lambda { ty: lambda_ty, .. } = &mut m.functions[main].body else {
                panic!("the wrapper is a lambda");
            };
            *lambda_ty = ty;
            let stats = eta(&mut m);
            assert_eq!(stats.applied, 0, "ev {ev}, sets {sets:?}");
            assert_eq!(stats.deopt, 1, "ev {ev}, sets {sets:?}");
        }
    }

    /// A direct callee with type parameters must be instantiated exactly as the lambda's type
    /// says: `f[Int](x)` reduces when it instantiates to the lambda's type, and a call with the
    /// wrong number of type arguments is left alone.
    #[test]
    fn a_direct_callee_with_ty_args_must_instantiate_exactly() {
        fn module(ty_args: Vec<TypeId>) -> (Module, FnId) {
            let mut m = Module::new();
            let param = m.intern(Type::Param(0));
            let f = m.declare("f", false);
            let p = m.local("x", param);
            m.define(f, vec![p], param, Expr::Var(p));
            m.functions[f].type_params = 1;

            let main = m.declare("main", true);
            let x = m.local("x", TypeId::INT);
            let ty = m.fn_type(vec![TypeId::INT], TypeId::INT);
            let body = Expr::Lambda {
                params: vec![x],
                ty,
                body: Box::new(Expr::Call {
                    callee: Callee::Direct { f, ty_args },
                    args: vec![Expr::Var(x)],
                    fails: false,
                    suspends: false,
                }),
                suspends: false,
            };
            m.define(main, Vec::new(), TypeId::UNIT, body);
            (m, main)
        }

        let (mut m, main) = module(vec![TypeId::INT]);
        let stats = eta(&mut m);
        assert_eq!(stats.applied, 1);
        assert_eq!(stats.deopt, 0);
        assert!(matches!(
            &m.functions[main].body,
            Expr::Fn { ty_args, .. } if ty_args == &[TypeId::INT]
        ));

        let (mut m, main) = module(Vec::new());
        let stats = eta(&mut m);
        assert_eq!(stats.applied, 0);
        assert_eq!(stats.deopt, 1);
        assert!(matches!(m.functions[main].body, Expr::Lambda { .. }));
    }
}
