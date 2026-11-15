//! Removal of the `List.range` that `List.fold` and `List.map` consume.
//!
//! `List.fold(List.range(lo, hi), init, f)` becomes a loop over an integer counter, and
//! `List.map(List.range(lo, hi), f)` builds the result list directly (in reverse) and reverses it
//! once at the end. The range's cons cells and its recursion are gone; the result, the order in
//! which `f` is called and every failure, effect and drop point stay the same.

use crate::ir::{BinOp, Callee, Expr, FnId, Lit, Local, Module, OpTy, Type, TypeId};
use crate::opt::{CoreFn, CoreFns, PassStats, each_body};

/// Rewrites `List.fold(List.range(lo, hi), init, f)` to a loop over an integer counter.
///
/// Preconditions: the callee is the bundled core `List.fold` ([`CoreFn::ListFold`]) with its four
/// type arguments and four arguments (`xs`, `from`, `f`, `ev`); `xs` is a direct
/// `List.range(lo, hi)`, and the element type is `Int`.
/// Postconditions: `f` is called once for each `i` in `lo..hi` (`max(0, hi - lo)` times), in
/// ascending order, with the accumulator and `i`; the result is the last accumulator, or `init`
/// when the range is empty. `lo`, `hi`, `init` and `f` are evaluated once each, in that order,
/// before the loop; `f` is evaluated even when the range is empty.
/// Preserves: evaluation order, the calls of `f` and their failures, effects and suspension
/// points, and drop points. Only the range's cons cells are no longer made.
/// Deopt: a call that is not this shape (a user's `fold` or `range`, another element type, a
/// `let`-bound range, failing or suspending marks on the range) is left alone.
/// Measures: `nqueens`, `matmul` and `spectral` (the cons cells of their ranges).
pub(super) fn fold(m: &mut Module, core_fns: &CoreFns) -> PassStats {
    let mut changed = 0;
    each_body(m, &mut |m, e| rewrite_fold(m, e, core_fns, &mut changed));
    PassStats {
        range_fold: changed,
        ..PassStats::default()
    }
}

/// Removes the `List.range` from `List.map(List.range(lo, hi), f)`.
///
/// Preconditions: the callee is the bundled core `List.map` ([`CoreFn::ListMap`]) with its four
/// type arguments and three arguments (`xs`, `f`, `ev`); `xs` is a direct `List.range(lo, hi)`,
/// and the element type is `Int`.
/// Postconditions: `f` is called once for each `i` in `lo..hi`, in ascending order, and the result
/// is `List.map`'s list (each element replaced, in order). The list is built in reverse and
/// [`CoreFn::ListReverse`] is called once at the end. `lo`, `hi` and `f` are evaluated once each,
/// in that order, before the loop; `f` is evaluated even when the range is empty.
/// Preserves: evaluation order, the calls of `f` and their failures, effects and suspension
/// points, and drop points. The range's cons cells and its recursion are no longer made; the
/// result list and the reverse still are.
/// Deopt: a call that is not this shape is left alone, as in [`fold`].
/// Measures: `matmul` and `spectral` (the range part of their cons cells; the result list stays).
pub(super) fn map(m: &mut Module, core_fns: &CoreFns) -> PassStats {
    let mut changed = 0;
    each_body(m, &mut |m, e| rewrite_map(m, e, core_fns, &mut changed));
    PassStats {
        range_map: changed,
        ..PassStats::default()
    }
}

fn rewrite_fold(m: &mut Module, e: &mut Expr, core_fns: &CoreFns, changed: &mut u32) {
    if let Some(fused) = fuse_fold(m, e, core_fns) {
        *e = fused;
        *changed += 1;
    }
    // Recurse into the replacement too, so a nested fold in `init` or `f` is fused on the same run
    // and a second run changes nothing.
    crate::tail::for_each_child_mut(e, &mut |c| rewrite_fold(m, c, core_fns, changed));
}

fn rewrite_map(m: &mut Module, e: &mut Expr, core_fns: &CoreFns, changed: &mut u32) {
    if let Some(fused) = fuse_map(m, e, core_fns) {
        *e = fused;
        *changed += 1;
    }
    crate::tail::for_each_child_mut(e, &mut |c| rewrite_map(m, c, core_fns, changed));
}

fn fuse_fold(m: &mut Module, e: &Expr, core_fns: &CoreFns) -> Option<Expr> {
    let Expr::Call {
        callee: Callee::Direct { f, ty_args },
        args,
        fails,
        suspends,
    } = e
    else {
        return None;
    };
    if core_fns.get(f) != Some(&CoreFn::ListFold)
        || ty_args.len() != 4
        || ty_args[0] != TypeId::INT
        || args.len() != 4
    {
        return None;
    }
    let (lo, hi) = range_args(&args[0], core_fns)?;
    let b = ty_args[1];
    let f_ty = param_type(m, *f, 2, ty_args);
    let takes_ev = takes_ev(m, f_ty)?;
    Some(fold_loop(
        m,
        lo.clone(),
        hi.clone(),
        args[1].clone(),
        args[2].clone(),
        args[3].clone(),
        *fails,
        *suspends,
        b,
        f_ty,
        takes_ev,
    ))
}

fn fuse_map(m: &mut Module, e: &Expr, core_fns: &CoreFns) -> Option<Expr> {
    let Expr::Call {
        callee: Callee::Direct { f, ty_args },
        args,
        fails,
        suspends,
    } = e
    else {
        return None;
    };
    if core_fns.get(f) != Some(&CoreFn::ListMap)
        || ty_args.len() != 4
        || ty_args[0] != TypeId::INT
        || args.len() != 3
    {
        return None;
    }
    let (lo, hi) = range_args(&args[0], core_fns)?;
    let b = ty_args[1];
    let f_ty = param_type(m, *f, 1, ty_args);
    let takes_ev = takes_ev(m, f_ty)?;
    let reverse = reverse_fn(core_fns)?;
    Some(map_loop(
        m,
        lo.clone(),
        hi.clone(),
        args[1].clone(),
        args[2].clone(),
        *fails,
        *suspends,
        b,
        f_ty,
        takes_ev,
        reverse,
    ))
}

/// The `lo` and `hi` of a direct `List.range(lo, hi)`, if `e` is one.
fn range_args<'a>(e: &'a Expr, core_fns: &CoreFns) -> Option<(&'a Expr, &'a Expr)> {
    let Expr::Call {
        callee: Callee::Direct { f, ty_args },
        args,
        fails: false,
        suspends: false,
    } = e
    else {
        return None;
    };
    if core_fns.get(f) != Some(&CoreFn::ListRange) || !ty_args.is_empty() || args.len() != 2 {
        return None;
    }
    Some((&args[0], &args[1]))
}

/// The type of parameter `index` of `callee`, with the call's type arguments substituted.
fn param_type(m: &mut Module, callee: FnId, index: usize, ty_args: &[TypeId]) -> TypeId {
    let param = m.functions[callee].params[index];
    let ty = m.locals[param].ty;
    m.types.subst(ty, ty_args)
}

/// Whether the instantiated function type takes an evidence table as its last argument.
fn takes_ev(m: &Module, ty: TypeId) -> Option<bool> {
    match m.ty(ty) {
        Type::Fn(f) => Some(f.ev),
        _ => None,
    }
}

/// `List.reverse`'s IR function. There is one copy (it is pure), but taking the smallest ID keeps
/// the choice deterministic if that ever changes.
fn reverse_fn(core_fns: &CoreFns) -> Option<FnId> {
    core_fns
        .iter()
        .filter(|(_, kind)| **kind == CoreFn::ListReverse)
        .map(|(&id, _)| id)
        .min()
}

#[allow(clippy::too_many_arguments)]
fn fold_loop(
    m: &mut Module,
    lo: Expr,
    hi: Expr,
    init: Expr,
    f: Expr,
    ev: Expr,
    fails: bool,
    suspends: bool,
    b: TypeId,
    f_ty: TypeId,
    takes_ev: bool,
) -> Expr {
    let lo_local = m.local("lo", TypeId::INT);
    let hi_local = m.local("hi", TypeId::INT);
    let init_local = m.local("init", b);
    let f_local = m.local("f", f_ty);
    let i = m.local("i", TypeId::INT);
    let acc = m.local("acc", b);
    let mut call_args = vec![Expr::Var(acc), Expr::Var(i)];
    if takes_ev {
        call_args.push(ev);
    }
    let step = Expr::Call {
        callee: Callee::Indirect(Box::new(Expr::Var(f_local))),
        args: call_args,
        fails,
        suspends,
    };
    let body = Expr::If {
        cond: Box::new(less_than(i, hi_local)),
        then: Box::new(Expr::Recur(vec![increment(i), step])),
        else_: Box::new(Expr::Var(acc)),
    };
    let loop_ = Expr::Loop {
        vars: vec![i, acc],
        inits: vec![Expr::Var(lo_local), Expr::Var(init_local)],
        body: Box::new(body),
    };
    Expr::Let {
        var: lo_local,
        value: Box::new(lo),
        body: Box::new(Expr::Let {
            var: hi_local,
            value: Box::new(hi),
            body: Box::new(Expr::Let {
                var: init_local,
                value: Box::new(init),
                body: Box::new(Expr::Let {
                    var: f_local,
                    value: Box::new(f),
                    body: Box::new(loop_),
                }),
            }),
        }),
    }
}

#[allow(clippy::too_many_arguments)]
fn map_loop(
    m: &mut Module,
    lo: Expr,
    hi: Expr,
    f: Expr,
    ev: Expr,
    fails: bool,
    suspends: bool,
    b: TypeId,
    f_ty: TypeId,
    takes_ev: bool,
    reverse: FnId,
) -> Expr {
    let lo_local = m.local("lo", TypeId::INT);
    let hi_local = m.local("hi", TypeId::INT);
    let f_local = m.local("f", f_ty);
    let i = m.local("i", TypeId::INT);
    let list_b = m.list(b);
    let acc = m.local("acc", list_b);
    let xs = m.local("xs", list_b);
    let mut call_args = vec![Expr::Var(i)];
    if takes_ev {
        call_args.push(ev);
    }
    let step = Expr::Call {
        callee: Callee::Indirect(Box::new(Expr::Var(f_local))),
        args: call_args,
        fails,
        suspends,
    };
    let cons = Expr::List {
        elem: b,
        elems: vec![step],
        tail: Some(Box::new(Expr::Var(acc))),
    };
    let body = Expr::If {
        cond: Box::new(less_than(i, hi_local)),
        then: Box::new(Expr::Recur(vec![increment(i), cons])),
        else_: Box::new(Expr::Var(acc)),
    };
    let loop_ = Expr::Loop {
        vars: vec![i, acc],
        inits: vec![
            Expr::Var(lo_local),
            Expr::List {
                elem: b,
                elems: Vec::new(),
                tail: None,
            },
        ],
        body: Box::new(body),
    };
    let reverse_call = Expr::Call {
        callee: Callee::Direct {
            f: reverse,
            ty_args: vec![b],
        },
        args: vec![Expr::Var(xs)],
        fails: false,
        suspends: false,
    };
    let reversed = Expr::Let {
        var: xs,
        value: Box::new(loop_),
        body: Box::new(reverse_call),
    };
    Expr::Let {
        var: lo_local,
        value: Box::new(lo),
        body: Box::new(Expr::Let {
            var: hi_local,
            value: Box::new(hi),
            body: Box::new(Expr::Let {
                var: f_local,
                value: Box::new(f),
                body: Box::new(reversed),
            }),
        }),
    }
}

fn less_than(i: Local, hi: Local) -> Expr {
    Expr::Binary {
        op: BinOp::Lt,
        ty: OpTy::Int,
        lhs: Box::new(Expr::Var(i)),
        rhs: Box::new(Expr::Var(hi)),
    }
}

fn increment(i: Local) -> Expr {
    Expr::Binary {
        op: BinOp::Add,
        ty: OpTy::Int,
        lhs: Box::new(Expr::Var(i)),
        rhs: Box::new(Expr::Lit(Lit::Int(1))),
    }
}
