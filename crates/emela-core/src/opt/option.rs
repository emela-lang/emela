//! Fusion of `Option.or` with `Array.get`.
//!
//! `Option.or(Array.get(a, i), d)` becomes the internal `ArrayGetOr(a, i, d)`, and
//! `match Array.get(a, i) { Some(x) -> …; None -> … }` becomes a bounds test with
//! `ArrayLoadUnchecked`. Both remove the `Some` allocation, which is most of some programs'
//! allocations, and keep the evaluation order and every failure, effect and drop point.

use rustc_hash::FxHashMap;

use crate::intrinsics::Builtin;
use crate::ir::{
    Arm, BinOp, Callee, EnumId, Expr, FnId, Lit, Local, Module, OPTION_NONE, OPTION_SOME, OpTy,
    Pat, Type, TypeId,
};
use crate::opt::{CoreFn, PassStats};

/// Rewrites `Option.or(Array.get(a, i), d)` to `ArrayGetOr(a, i, d)`.
///
/// Preconditions: the callee is the bundled core `Option.or` ([`CoreFn::OptionOr`]), the call
/// neither fails nor suspends, its one type argument is the element type of the `Array.get`, and
/// that type is neither `()` nor `Never`.
/// Postconditions: the value is the element at `i`, or `d`; each of `a`, `i`, `d` is evaluated
/// once, in that order. `d` is evaluated even when `i` is in range, as before.
/// Preserves: evaluation order, failures and effects of the arguments, and drop points. The
/// `Some` the original call made is not observable.
/// Deopt: a call that is not this shape (an indirect call, a user's `or`, another element type,
/// `()` or `Never`, a failing or suspending callee) is left alone.
/// Measures: `matmul`, `fannkuch` and `spectral` (the `Option.or` part of their `Some`
/// allocations).
pub(super) fn option_or(m: &mut Module, core_fns: &FxHashMap<FnId, CoreFn>) -> PassStats {
    let mut changed = 0;
    each_body(m, &mut |m, e| {
        rewrite_option_or(m, e, core_fns, &mut changed)
    });
    PassStats {
        option_or: changed,
        ..PassStats::default()
    }
}

/// Rewrites `match Array.get(a, i) { Some(x) -> …; None -> … }` to a bounds test and a load.
///
/// Preconditions: the scrutinee is a direct `Array.get(a, i)`; the arms are exactly one
/// `Some(x)` or `Some(_)` and one `None`, in either order, with no guards; the element type is
/// neither `()` nor `Never`.
/// Postconditions: `a` and `i` are evaluated once each, in that order, before the test; the
/// `Some` arm's body sees the element, and the `None` arm's body runs when `i < 0` or
/// `i >= Array.size(a)`. The `Some` wrapper is not made.
/// Preserves: evaluation order, failures and effects of the scrutinee and both bodies, and drop
/// points.
/// Deopt: a `match` on a `let`-bound scrutinee (`let t = Array.get(..); match t`) is not fused
/// yet; neither is any other pattern shape or a guarded arm.
/// Measures: `matmul` (its `match Array.get` is most of its `Some` allocations).
///
/// `core_fns` is unused: the scrutinee is recognised by its builtin.
pub(super) fn array_get_match(m: &mut Module, _core_fns: &FxHashMap<FnId, CoreFn>) -> PassStats {
    let mut changed = 0;
    each_body(m, &mut |m, e| rewrite_array_get_match(m, e, &mut changed));
    PassStats {
        array_get_match: changed,
        ..PassStats::default()
    }
}

/// Applies `f` to the body of every function, with the module to allocate locals in.
fn each_body(m: &mut Module, f: &mut impl FnMut(&mut Module, &mut Expr)) {
    let ids: Vec<FnId> = m.functions.iter().map(|(id, _)| id).collect();
    for id in ids {
        let mut body = std::mem::replace(&mut m.functions[id].body, Expr::Lit(Lit::Unit));
        f(m, &mut body);
        m.functions[id].body = body;
    }
}

fn rewrite_option_or(
    m: &mut Module,
    e: &mut Expr,
    core_fns: &FxHashMap<FnId, CoreFn>,
    changed: &mut u32,
) {
    if let Some(fused) = fuse_option_or(m, e, core_fns) {
        *e = fused;
        *changed += 1;
    }
    // Recurse into the replacement too, so an argument that holds another match is fused on the
    // same run and a second run changes nothing.
    crate::tail::for_each_child_mut(e, &mut |c| rewrite_option_or(m, c, core_fns, changed));
}

fn fuse_option_or(m: &Module, e: &Expr, core_fns: &FxHashMap<FnId, CoreFn>) -> Option<Expr> {
    let Expr::Call {
        callee: Callee::Direct { f, ty_args },
        args,
        fails: false,
        suspends: false,
    } = e
    else {
        return None;
    };
    if core_fns.get(f) != Some(&CoreFn::OptionOr) || ty_args.len() != 1 || args.len() != 2 {
        return None;
    }
    let Expr::Builtin {
        op: Builtin::ArrayGet,
        ty_args: get_ty_args,
        args: get_args,
    } = &args[0]
    else {
        return None;
    };
    if get_ty_args.len() != 1 || get_ty_args[0] != ty_args[0] || get_args.len() != 2 {
        return None;
    }
    let elem = ty_args[0];
    if matches!(m.ty(elem), Type::Unit | Type::Never) {
        return None;
    }
    Some(Expr::Builtin {
        op: Builtin::ArrayGetOr,
        ty_args: vec![elem],
        args: vec![get_args[0].clone(), get_args[1].clone(), args[1].clone()],
    })
}

fn rewrite_array_get_match(m: &mut Module, e: &mut Expr, changed: &mut u32) {
    if fuse_array_get_match(m, e) {
        *changed += 1;
    }
    crate::tail::for_each_child_mut(e, &mut |c| rewrite_array_get_match(m, c, changed));
}

/// Rewrites `e` if it is the `match` shape, and returns whether it did.
fn fuse_array_get_match(m: &mut Module, e: &mut Expr) -> bool {
    let Expr::Match { scrutinee, arms } = e else {
        return false;
    };
    if arms.len() != 2 {
        return false;
    }
    let Expr::Builtin {
        op: Builtin::ArrayGet,
        ty_args,
        args,
    } = &**scrutinee
    else {
        return false;
    };
    if ty_args.len() != 1 || args.len() != 2 {
        return false;
    }
    let elem = ty_args[0];
    if matches!(m.ty(elem), Type::Unit | Type::Never) {
        return false;
    }
    let Some(option) = m.option else {
        return false;
    };
    let Some((some_index, none_index, binding)) = option_arms(arms, option) else {
        return false;
    };

    // The checked shape is still there: take it apart and build the replacement.
    let Expr::Match {
        scrutinee,
        mut arms,
    } = std::mem::replace(e, Expr::Lit(Lit::Unit))
    else {
        unreachable!()
    };
    let Expr::Builtin { args, .. } = *scrutinee else {
        unreachable!()
    };
    let mut args = args.into_iter();
    let a = args.next().expect("two arguments");
    let i = args.next().expect("two arguments");
    let some_body = std::mem::replace(&mut arms[some_index].body, Expr::Lit(Lit::Unit));
    let none_body = std::mem::replace(&mut arms[none_index].body, Expr::Lit(Lit::Unit));

    let array_ty = m.intern(Type::Array(elem));
    let a_local = m.local("a", array_ty);
    let i_local = m.local("i", TypeId::INT);
    let x_local = binding.unwrap_or_else(|| m.local("x", elem));

    let in_range = Expr::Binary {
        op: BinOp::And,
        ty: OpTy::Bool,
        lhs: Box::new(Expr::Binary {
            op: BinOp::Ge,
            ty: OpTy::Int,
            lhs: Box::new(Expr::Var(i_local)),
            rhs: Box::new(Expr::Lit(Lit::Int(0))),
        }),
        rhs: Box::new(Expr::Binary {
            op: BinOp::Lt,
            ty: OpTy::Int,
            lhs: Box::new(Expr::Var(i_local)),
            rhs: Box::new(Expr::Builtin {
                op: Builtin::ArraySize,
                ty_args: vec![elem],
                args: vec![Expr::Var(a_local)],
            }),
        }),
    };
    let load = Expr::Builtin {
        op: Builtin::ArrayLoadUnchecked,
        ty_args: vec![elem],
        args: vec![Expr::Var(a_local), Expr::Var(i_local)],
    };
    let then = Expr::Let {
        var: x_local,
        value: Box::new(load),
        body: Box::new(some_body),
    };
    let if_ = Expr::If {
        cond: Box::new(in_range),
        then: Box::new(then),
        else_: Box::new(none_body),
    };
    *e = Expr::Let {
        var: a_local,
        value: Box::new(a),
        body: Box::new(Expr::Let {
            var: i_local,
            value: Box::new(i),
            body: Box::new(if_),
        }),
    };
    true
}

/// The indices of the `Some` and `None` arms and the local the `Some` field binds (`None` for
/// `Some(_)`), or `None` if the two patterns are not exactly one of each, unguarded.
fn option_arms(arms: &[Arm], option: EnumId) -> Option<(usize, usize, Option<Local>)> {
    let mut some = None;
    let mut none = None;
    for (index, arm) in arms.iter().enumerate() {
        if arm.guard.is_some() {
            return None;
        }
        let Pat::Ctor { ctor, fields } = &arm.pat else {
            return None;
        };
        if ctor.enum_id != option {
            return None;
        }
        match ctor.variant {
            OPTION_SOME if fields.len() == 1 => {
                let binding = match &fields[0] {
                    Pat::Bind(l) => Some(*l),
                    Pat::Wild => None,
                    _ => return None,
                };
                if some.replace((index, binding)).is_some() {
                    return None;
                }
            }
            OPTION_NONE if fields.is_empty() => {
                if none.replace(index).is_some() {
                    return None;
                }
            }
            _ => return None,
        }
    }
    let (some_index, binding) = some?;
    Some((some_index, none?, binding))
}
