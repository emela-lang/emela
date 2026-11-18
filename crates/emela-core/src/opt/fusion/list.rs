//! Range fusion laws: the `List.range` that `List.fold` and `List.map` consume.
//!
//! `List.fold(List.range(lo, hi), init, f)` becomes a loop over an integer counter, and
//! `List.map(List.range(lo, hi), f)` builds the result list directly (in reverse) and reverses it
//! once at the end. The range's cons cells and its recursion are gone; the result, the order in
//! which `f` is called and every failure, effect and drop point stay the same.
//!
//! `Array.from_list(List.map(List.range(lo, hi), f))` builds neither the range nor the mapped
//! list: it becomes the internal `ArrayMapRange(lo, hi, f)`, which makes the array in one pass
//! ([`map_array`]). A defect from `f` is not in its type; the loop stops at the first one and
//! releases the array with the elements already written.
//!
//! When `f` is a lambda literal, its body is copied into the loop instead of being called through
//! the closure ([`inlined_step`]). That is the common shape of a fused loop, and it is what lets
//! the rest of the pipeline look inside the loop's step.
//!
//! These are the bodies of the registry entries [`LawId::RangeFold`], [`LawId::RangeMapArray`]
//! and [`LawId::RangeMap`], in that registry order; the registry's flags, order and budget are in
//! [`super`], and the contract text of each law is beside its entry in [`super::law`].

use crate::intrinsics::Builtin;
use crate::ir::{BinOp, Callee, Expr, FnId, Lit, Local, Module, OpTy, Type, TypeId};
use crate::opt::{CoreFn, CoreFns, PassStats, each_body};

use super::law::LawRun;

/// Rewrites `List.fold(List.range(lo, hi), init, f)` to a loop over an integer counter.
///
/// Preconditions: the callee is the bundled core `List.fold` ([`CoreFn::ListFold`]) with its four
/// type arguments and four arguments (`xs`, `from`, `f`, `ev`); `xs` is a direct
/// `List.range(lo, hi)`, and the element type is `Int`.
/// Postconditions: `f` is called once for each `i` in `lo..hi` (`max(0, hi - lo)` times), in
/// ascending order, with the accumulator and `i`; the result is the last accumulator, or `init`
/// when the range is empty. `lo`, `hi`, `init` and `f` are evaluated once each, in that order,
/// before the loop; `f` is evaluated even when the range is empty. A lambda literal as `f` is
/// copied in rather than called ([`inlined_step`]), so it is not made at all.
/// Preserves: evaluation order, the calls of `f` and their failures, effects and suspension
/// points, and drop points. Only the range's cons cells are no longer made, and the closure of an
/// inlined lambda is not made.
/// Deopt: a call that is not this shape (a user's `fold` or `range`, another element type, a
/// `let`-bound range, failing or suspending marks on the range) is left alone.
/// Measures: `nqueens`, `matmul` and `spectral` (the cons cells of their ranges).
pub(super) fn fold(m: &mut Module, core_fns: &CoreFns) -> LawRun {
    let mut changed = 0;
    each_body(m, &mut |m, e| rewrite_fold(m, e, core_fns, &mut changed));
    LawRun {
        applied: changed,
        stats: PassStats {
            range_fold: changed,
            ..PassStats::default()
        },
    }
}

/// Removes the list from `Array.from_list(List.map(List.range(lo, hi), f))`.
///
/// Preconditions: the argument is a direct `List.map(List.range(lo, hi), f)` of the bundled core
/// [`CoreFn::ListMap`] with its four type arguments and three arguments (`xs`, `f`, `ev`); the
/// element type is `Int`, the map's result type is the array's element type, that type is neither
/// `()` nor `Never`, and `f`'s type is a pure function of one `Int` (no failure, no suspension, no
/// evidence table), so the map call neither fails nor suspends.
/// Postconditions: the result is an array of `max(0, hi - lo)` elements whose element `i` is
/// `f(lo + i)`, in ascending order. `lo`, `hi` and `f` are evaluated once each, in that order;
/// `f` is evaluated even when the range is empty. Neither the range's cons cells nor the mapped
/// list are made, and the array is made once.
/// Preserves: evaluation order, the calls of `f` and their failures, effects and suspension
/// points, and drop points. A defect is not part of `f`'s type: it stops the loop at the first
/// one, and the array, which is made before the first call, is released with the elements already
/// written. The array holds the values `f` returns, owned as the list's elements were.
/// Deopt: a call that is not this shape is left alone, as in [`fold`]; so is an effectful,
/// failing or suspending `f` (the internal builtin has no failure edge and takes no evidence
/// table), and an element type without a representation.
/// Measures: `matmul`, `spectral` and `k-nucleotide` (the mapped list's cons cells and the second
/// traversal `Array.from_list` made).
pub(super) fn map_array(m: &mut Module, core_fns: &CoreFns) -> LawRun {
    let mut changed = 0;
    each_body(m, &mut |m, e| {
        rewrite_map_array(m, e, core_fns, &mut changed)
    });
    LawRun {
        applied: changed,
        stats: PassStats {
            map_range: changed,
            ..PassStats::default()
        },
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
/// in that order, before the loop; `f` is evaluated even when the range is empty. A lambda literal
/// as `f` is copied in rather than called ([`inlined_step`]), so it is not made at all.
/// Preserves: evaluation order, the calls of `f` and their failures, effects and suspension
/// points, and drop points. The range's cons cells, the recursion of the range and the closure of
/// an inlined lambda are no longer made; the result list and the reverse still are.
/// Deopt: a call that is not this shape is left alone, as in [`fold`].
/// Measures: the list forms of the range fusion. An `Array.from_list(List.map(List.range(…), f))`
/// is [`map_array`]'s case, which this pass sees only when that fusion gave up (an effectful `f`,
/// or an element type without a representation).
pub(super) fn map(m: &mut Module, core_fns: &CoreFns) -> LawRun {
    let mut changed = 0;
    each_body(m, &mut |m, e| rewrite_map(m, e, core_fns, &mut changed));
    LawRun {
        applied: changed,
        stats: PassStats {
            range_map: changed,
            ..PassStats::default()
        },
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

fn rewrite_map_array(m: &mut Module, e: &mut Expr, core_fns: &CoreFns, changed: &mut u32) {
    if let Some(fused) = fuse_map_array(m, e, core_fns) {
        *e = fused;
        *changed += 1;
    }
    crate::tail::for_each_child_mut(e, &mut |c| rewrite_map_array(m, c, core_fns, changed));
}

/// `Array.from_list(List.map(List.range(lo, hi), f))` as the internal `ArrayMapRange(lo, hi, f)`.
fn fuse_map_array(m: &mut Module, e: &Expr, core_fns: &CoreFns) -> Option<Expr> {
    let Expr::Builtin {
        op: Builtin::ArrayFromList,
        ty_args: array_ty_args,
        args: array_args,
    } = e
    else {
        return None;
    };
    let [b] = array_ty_args[..] else {
        return None;
    };
    if array_args.len() != 1 {
        return None;
    }
    let mapped = &array_args[0];
    let Expr::Call {
        callee: Callee::Direct { f, ty_args },
        args,
        fails: false,
        suspends: false,
    } = mapped
    else {
        return None;
    };
    if core_fns.get(f) != Some(&CoreFn::ListMap)
        || ty_args.len() != 4
        || ty_args[0] != TypeId::INT
        || ty_args[1] != b
        || args.len() != 3
    {
        return None;
    }
    let (lo, hi) = range_args(&args[0], core_fns)?;
    if matches!(m.ty(b), Type::Unit | Type::Never) {
        return None;
    }
    let f_ty = param_type(m, *f, 1, ty_args);
    // A pure `f` takes no evidence table and cannot fail or suspend. The builtin has no failure
    // edge, so anything else is left to the list form, which keeps it. A defect is not part of a
    // function type, so the builtin may raise one too; the loop stops at the first.
    let Type::Fn(ft) = m.ty(f_ty) else {
        return None;
    };
    if ft.fails || ft.suspends || ft.ev || ft.params != [TypeId::INT] || ft.ret != b {
        return None;
    }
    Some(Expr::Builtin {
        op: Builtin::ArrayMapRange,
        ty_args: vec![b, f_ty],
        args: vec![lo.clone(), hi.clone(), args[1].clone()],
    })
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
    let i = m.local("i", TypeId::INT);
    let acc = m.local("acc", b);
    // The step is `f(acc, i)`, or the body of a lambda `f` with those two parameters: a lambda has
    // this one call site (the loop is in the function that made it), so copying the body in
    // removes an indirect call and a closure from every iteration.
    let (step, closure) = match if takes_ev {
        None
    } else {
        inlined_step(&f, &[acc, i])
    } {
        Some(body) => (body, None),
        None => {
            let f_local = m.local("f", f_ty);
            let mut call_args = vec![Expr::Var(acc), Expr::Var(i)];
            if takes_ev {
                call_args.push(ev);
            }
            let call = Expr::Call {
                callee: Callee::Indirect(Box::new(Expr::Var(f_local))),
                args: call_args,
                fails,
                suspends,
            };
            (call, Some(f_local))
        }
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
    let inner = match closure {
        Some(f_local) => Expr::Let {
            var: f_local,
            value: Box::new(f),
            body: Box::new(loop_),
        },
        None => loop_,
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
                body: Box::new(inner),
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
    let i = m.local("i", TypeId::INT);
    let list_b = m.list(b);
    let acc = m.local("acc", list_b);
    let xs = m.local("xs", list_b);
    let (step, closure) = match if takes_ev {
        None
    } else {
        inlined_step(&f, &[i])
    } {
        Some(body) => (body, None),
        None => {
            let f_local = m.local("f", f_ty);
            let mut call_args = vec![Expr::Var(i)];
            if takes_ev {
                call_args.push(ev);
            }
            let call = Expr::Call {
                callee: Callee::Indirect(Box::new(Expr::Var(f_local))),
                args: call_args,
                fails,
                suspends,
            };
            (call, Some(f_local))
        }
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
    let inner = match closure {
        Some(f_local) => Expr::Let {
            var: f_local,
            value: Box::new(f),
            body: Box::new(reversed),
        },
        None => reversed,
    };
    Expr::Let {
        var: lo_local,
        value: Box::new(lo),
        body: Box::new(Expr::Let {
            var: hi_local,
            value: Box::new(hi),
            body: Box::new(inner),
        }),
    }
}

/// The body of `f` with its parameters replaced by `args`, when `f` is a lambda that takes exactly
/// those parameters and no evidence table.
///
/// `List.fold(List.range(..), .., |acc, i| ..)` and `List.map(List.range(..), |i| ..)` hand the
/// loop a closure whose only call site is the loop's step, in the function that made it. Copying
/// the body in removes an indirect call and a closure (an allocation, plus a reference for each
/// capture) from every iteration, and leaves the step as straight-line code that the LIR and the C
/// compiler can see through: after this, `Array.get(a, i)` in the step is an expression of the
/// enclosing function, not a call into a closure.
///
/// Creating a lambda value has no effect (its free variables are captured as they are, and the
/// capture set is computed later), so not creating it changes nothing observable: memory use and
/// when a program runs out of memory are implementation-defined (`[effect.handler]`).
///
/// Deopt: `f` is not a lambda, takes an evidence table (a third parameter), suspends, or its
/// parameters do not line up with the loop's variables.
fn inlined_step(f: &Expr, args: &[Local]) -> Option<Expr> {
    let Expr::Lambda {
        params,
        body,
        suspends: false,
        ..
    } = f
    else {
        return None;
    };
    if params.len() != args.len() {
        return None;
    }
    let mut body = (**body).clone();
    substitute(&mut body, params, args);
    Some(body)
}

/// Replaces every `Var(params[i])` in `e` by `Var(args[i])`. Locals are unique in a function, so
/// no binding inside the body can capture a parameter, and the arguments are the loop's own
/// variables, so the replacement cannot duplicate or reorder an evaluation.
fn substitute(e: &mut Expr, params: &[Local], args: &[Local]) {
    if let Expr::Var(l) = e {
        if let Some(i) = params.iter().position(|p| p == l) {
            *l = args[i];
        }
        return;
    }
    crate::tail::for_each_child_mut(e, &mut |c| substitute(c, params, args));
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
