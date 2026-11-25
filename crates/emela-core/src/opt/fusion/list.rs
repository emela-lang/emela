//! Fusion laws over `List`: the range laws and the pipeline laws.
//!
//! # Range fusion
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
//! # Pipeline fusion
//!
//! The three pipeline laws compose the function of one list traversal with the function of the
//! traversal that consumes its list, so the intermediate list is never built:
//!
//! - `List.map(List.map(xs, f), g)` becomes `List.map(xs, λx. g(f x))` ([`map_map`]);
//! - `List.fold(List.map(xs, f), init, g)` becomes `List.fold(xs, init, λa, x. g(a, f x))`
//!   ([`fold_map`]);
//! - `List.map(List.filter(xs, p), f)` becomes
//!   `List.reverse(List.fold(xs, [], λacc, x. if p(x) { [f(x), ..acc] } else { acc }))`
//!   ([`filter_map`]).
//!
//! The composition is by call, not by copying a body: `g(f x)` is called in the same order as the
//! two traversals would call the two functions for one element. The change from two traversals to
//! one is unobservable when the functions are [`quiet`] (no effect, failure or suspension: their
//! call order would otherwise be observable) and when at most one of the evaluations the fusion
//! reorders is [`restless`] (may reach a defect or not terminate, which would let it overtake the
//! other). They must also be a lambda literal or a reference to a top-level function ([`Step`]):
//! the body of any other function value is unknown, so it deopts.
//!
//! # Registry entries
//!
//! These are the bodies of the registry entries [`LawId::MapMap`], [`LawId::FoldMap`],
//! [`LawId::FilterMap`], [`LawId::RangeFold`], [`LawId::RangeMapArray`] and [`LawId::RangeMap`], in
//! that registry order; the registry's flags, order and budget are in [`super`], and the contract
//! text of each law is beside its entry in [`super::law`].

use crate::intrinsics::Builtin;
use crate::ir::{BinOp, Callee, Expr, FnId, Lit, Local, Module, OpTy, Type, TypeId};
use crate::opt::analysis::effect::{self, EffectSummaries, EffectSummary};
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

// ---- The pipeline laws: map∘map, fold∘map and filter∘map (D3/PR-030) ----
//
// The map∘map and fold∘map shapes are shared with the array laws (D4/PR-031): the two collection
// types have the same call shape (four type arguments and three or four arguments, the collection
// first and the evidence table last), so [`compose_map_over`] and [`compose_fold_over`] take the
// [`CoreFn`] identities as arguments. The array module is the only other caller.

/// The most nodes a composed lambda may add over the two function expressions it composes.
///
/// The composition is by call, so its size is constant: a parameter, the calls and their callees,
/// plus (for `filter_map`) the test and the cons. The largest composition (`filter_map`) is well
/// below this bound, so the per-site budget documents the shape instead of declining a real site;
/// the budget that can fire is [`MAX_COMPOSED_PER_BODY`].
const COMPOSE_OVERHEAD: usize = 20;

/// The most composed lambdas one function body may gain from one pipeline law in one sweep.
///
/// A composition adds a lambda and, when a function value is a literal, a `let`. The cap bounds how
/// much one function body can grow in one sweep, so a chain of pipelines cannot multiply the code
/// without limit. It is above what a hand-written function needs; only the budget test reaches it.
const MAX_COMPOSED_PER_BODY: u32 = 16;

/// Rewrites `List.map(List.map(xs, f), g)` to `List.map(xs, λx. g(f x))`.
///
/// Preconditions: the callee is the bundled core `List.map` ([`CoreFn::ListMap`]) with its four type
/// arguments and three arguments (`xs`, `f`, `ev`), and its `xs` argument is a direct call of the
/// same function in the same shape; neither call fails or suspends. `f` and `g` are a lambda
/// literal or a reference to a top-level function ([`Step`]), their instantiated types are pure, and
/// their bodies are [`quiet`]; at most one of them may be [`restless`].
/// Postconditions: the result is `List.map`'s list with each element replaced by `g(f(x))`, in the
/// order of `xs`. `f` and `g` are called once per element, `f` before `g`; no intermediate list is
/// made. The fused call keeps the outer call's type arguments for the result and its evidence
/// argument, and takes the element type from the inner call.
/// Preserves: evaluation order, effects, failures, suspension points and drop points. `f` and `g`
/// are proved effect-free, so calling them per element instead of in two traversals is not
/// observable; evaluating `xs` and the function values keeps its relative order.
/// Deopt: any other shape (a user's `map`, an indirect call); a function value that is neither a
/// lambda literal nor a reference to a top-level function; an effect, failure or suspension in `f`
/// or `g`; a site over the code-size budget.
/// Measures: the map pipelines of the benchmarks (the intermediate list's cons cells).
pub(super) fn map_map(m: &mut Module, core_fns: &CoreFns) -> LawRun {
    let run = compose_all(m, core_fns, fuse_map_map);
    LawRun {
        applied: run.applied,
        stats: PassStats {
            map_map: run.applied,
            deopt: run.deopt,
            ..PassStats::default()
        },
    }
}

/// Rewrites `List.fold(List.map(xs, f), init, g)` to `List.fold(xs, init, λa, x. g(a, f x))`.
///
/// Preconditions: the callee is the bundled core `List.fold` ([`CoreFn::ListFold`]) with its four
/// type arguments and four arguments (`xs`, `from`, `f`, `ev`), and its `xs` argument is a direct
/// call of the core `List.map` in the shape above; neither call fails or suspends. `f` and `g` are a
/// lambda literal or a reference to a top-level function, their instantiated types are pure, and
/// their bodies may neither use an effect nor fail nor suspend.
/// Postconditions: the result is the last accumulator of folding `g(acc, f(x))` over `xs`, or
/// `init` when `xs` is empty. `f` and `g` are called once per element, `f` before `g`, in the order
/// of `xs`; no intermediate list is made. The fold keeps its accumulator type, its type arguments
/// and its evidence argument.
/// Preserves: evaluation order, effects, failures, suspension points and drop points, for the
/// reason in [`map_map`]. `xs`, `init` and the function values are evaluated once, before the fold.
/// Deopt: the same refusals as [`map_map`], with `List.fold` as the outer call.
/// Measures: the fold pipelines of the benchmarks (the intermediate list's cons cells).
pub(super) fn fold_map(m: &mut Module, core_fns: &CoreFns) -> LawRun {
    let run = compose_all(m, core_fns, fuse_fold_map);
    LawRun {
        applied: run.applied,
        stats: PassStats {
            fold_map: run.applied,
            deopt: run.deopt,
            ..PassStats::default()
        },
    }
}

/// Rewrites `List.map(List.filter(xs, p), f)` to one fold that tests and maps each element.
///
/// The equation is
/// `List.map(List.filter(xs, p), f)` →
/// `List.reverse(List.fold(xs, [], λacc, x. if p(x) { [f(x), ..acc] } else { acc }))`.
/// It is the shape the library's own `filter` and `map` build (each is a fold into a reversed list
/// and one `List.reverse`), so the fusion makes the two traversals one and drops the two lists the
/// filter would build.
///
/// Preconditions: the callee is the bundled core `List.map` ([`CoreFn::ListMap`]) with its four type
/// arguments and three arguments, and its `xs` argument is a direct call of the bundled core
/// `List.filter` ([`CoreFn::ListFilter`]) with its three type arguments and three arguments; neither
/// call fails or suspends. `p` and `f` are a lambda literal or a reference to a top-level function,
/// their instantiated types are pure, and their bodies may neither use an effect nor fail nor
/// suspend. The module has a non-suspending `List.fold` and a `List.reverse`.
/// Postconditions: the result is the list of `f(x)` for each element `x` of `xs` for which `p(x)`
/// holds, in the order of `xs`. For each element `p` is called first, and `f` only when `p` holds.
/// The fold builds the result in reverse and the `List.reverse` puts it in order.
/// Preserves: evaluation order, effects, failures, suspension points and drop points, for the reason
/// in [`map_map`]. `xs` and the function values are evaluated once, before the fold.
/// Deopt: any other shape (a user's `filter` or `map`, an indirect call); a function value that is
/// neither a lambda literal nor a reference to a top-level function; an effect, failure or
/// suspension in `p` or `f`; a module with only a suspending `List.fold` or no `List.reverse`; a
/// site over the code-size budget.
/// Measures: the filter-then-map pipelines of the benchmarks (one of the two intermediate lists and
/// one of the two reverse passes).
pub(super) fn filter_map(m: &mut Module, core_fns: &CoreFns) -> LawRun {
    let run = compose_all(m, core_fns, fuse_filter_map);
    LawRun {
        applied: run.applied,
        stats: PassStats {
            filter_map: run.applied,
            deopt: run.deopt,
            ..PassStats::default()
        },
    }
}

/// What a law did at one site.
pub(super) enum Outcome {
    /// The fused expression that replaces the site.
    Rewritten(Expr),
    /// The site is the law's shape, but a precondition or the budget refused it.
    Declined,
}

/// The sites one law rewrote and declined in one body or in the whole module.
#[derive(Default, Clone, Copy)]
pub(super) struct Tally {
    pub(super) applied: u32,
    pub(super) deopt: u32,
}

/// The body of a pipeline law: the per-site transform.
pub(super) type FuseFn = fn(&mut Module, &Expr, &CoreFns, &EffectSummaries) -> Option<Outcome>;

/// Runs one pipeline law over every function body, with the effect summaries of the module.
///
/// The summaries are computed once, before any rewrite. A rewrite only ever adds calls of functions
/// the law proved effect-free, so the summaries of the module's functions cannot become less
/// conservative while the law runs: using the table of the start of the sweep is sound for every
/// site, including a site in a function an earlier site rewrote.
///
/// The budget is [`MAX_COMPOSED_PER_BODY`] compositions per function body: [`each_body`] calls the
/// closure once per body, so the count starts again at each.
pub(super) fn compose_all(m: &mut Module, core_fns: &CoreFns, fuse: FuseFn) -> Tally {
    let summaries = effect::of_module(m);
    let mut total = Tally::default();
    each_body(m, &mut |m, e| {
        let mut body = Tally::default();
        compose(m, e, core_fns, &summaries, fuse, &mut body);
        total.applied += body.applied;
        total.deopt += body.deopt;
    });
    total
}

fn compose(
    m: &mut Module,
    e: &mut Expr,
    core_fns: &CoreFns,
    summaries: &EffectSummaries,
    fuse: FuseFn,
    tally: &mut Tally,
) {
    match fuse(m, e, core_fns, summaries) {
        None => {}
        Some(Outcome::Declined) => tally.deopt += 1,
        Some(Outcome::Rewritten(fused)) => {
            if tally.applied >= MAX_COMPOSED_PER_BODY {
                // The budget of this body is spent. The locals and clones the composition made are
                // unused; a deopt is not a rewrite, so the sweep does not repeat it.
                tally.deopt += 1;
            } else {
                *e = fused;
                tally.applied += 1;
            }
        }
    }
    // Recurse into the replacement too, so a chain `map(map(map(…)))` is composed in one run and a
    // second run changes nothing.
    crate::tail::for_each_child_mut(e, &mut |c| compose(m, c, core_fns, summaries, fuse, tally));
}

/// A function value a pipeline law can call: a reference to a top-level function, or a lambda
/// literal bound to a local.
///
/// A reference stays a direct call, which the later passes can see through. A lambda literal is
/// bound to a local with a `let` before the fused call, so it is made once, as it was, and the
/// composed lambda captures it. Any other expression (a parameter, a field, a call) is not this
/// shape: its body and its effects are unknown, so the law deopts.
enum Step {
    /// A direct call of `f`, with the instantiation the value had.
    Direct { f: FnId, ty_args: Vec<TypeId> },
    /// An indirect call through the local holding the lambda.
    Indirect(Local),
}

impl Step {
    /// The call `self(args)`, which neither fails nor suspends: both function types were checked to
    /// be pure.
    fn call(&self, args: Vec<Expr>) -> Expr {
        let callee = match self {
            Self::Direct { f, ty_args } => Callee::Direct {
                f: *f,
                ty_args: ty_args.clone(),
            },
            Self::Indirect(local) => Callee::Indirect(Box::new(Expr::Var(*local))),
        };
        Expr::Call {
            callee,
            args,
            fails: false,
            suspends: false,
        }
    }
}

/// The [`Step`] of the function value `e`, binding a lambda literal to a fresh local (in
/// `bindings`, in evaluation order) and leaving a top-level function as a direct call.
fn step(m: &mut Module, e: &Expr, bindings: &mut Vec<(Local, Expr)>) -> Option<Step> {
    match e {
        Expr::Fn { f, ty_args } => Some(Step::Direct {
            f: *f,
            ty_args: ty_args.clone(),
        }),
        Expr::Lambda { ty, .. } => {
            let local = m.local("f", *ty);
            bindings.push((local, e.clone()));
            Some(Step::Indirect(local))
        }
        _ => None,
    }
}

/// What calling the function value `e` may do, or `None` when its body is not visible.
///
/// A lambda is summarised by its body; a top-level function by its summary in `summaries`. Any
/// other expression (a parameter, a field, a call) is not known, and its caller deopts.
fn function_summary(m: &Module, e: &Expr, summaries: &EffectSummaries) -> Option<EffectSummary> {
    match e {
        Expr::Lambda { body, .. } => Some(effect::of_expr_in(m, body, summaries)),
        Expr::Fn { f, .. } => Some(summaries.get(f).copied().unwrap_or_default()),
        _ => None,
    }
}

/// Whether calling the function may use an effect, fail or suspend.
///
/// These are the facts whose *order* the fusion would change: `f` and `g` are called in two
/// traversals before and interleaved after, and `dbg` counts as an effect here even though the
/// builtin table calls it pure ([`crate::intrinsics`]), because it writes to stderr.
fn quiet(summary: EffectSummary) -> bool {
    !summary.may_use_effect && !summary.may_fail && !summary.may_suspend
}

/// Whether evaluating the function may stop the program at a defect or by not terminating.
///
/// Such a stop makes the position of the other evaluations observable: if both `f` and `g` may
/// stop, the fusion can change which one stops first (the original runs every `f` before the first
/// `g`, the fused form runs them per element). When at most one of the reordered evaluations may
/// stop, the calls of that one keep their order, so the first stop is the same.
fn restless(summary: EffectSummary) -> bool {
    summary.may_defect || summary.may_diverge
}

/// Whether evaluating `e` has no effect, so that dropping its evaluation is unobservable.
///
/// The three laws drop the *inner* call and with it the evidence argument the call passed. Lowering
/// makes that argument a local or `()` ([`crate::lower`]'s evidence argument); the check keeps the
/// "nothing observable is removed" claim true even if a later pass ever builds another expression
/// there.
fn is_value(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Lit(_) | Expr::Var(_) | Expr::Fn { .. } | Expr::Lambda { .. }
    )
}

/// Whether the instantiated function type `ty` is pure and exact: a call to it cannot fail,
/// suspend or need an evidence table, and its E and R are not open set parameters.
fn pure_step(m: &Module, ty: TypeId) -> bool {
    matches!(
        m.ty(ty),
        Type::Fn(f) if !f.fails && !f.suspends && !f.ev && f.sets.is_none()
    )
}

/// `body` wrapped in the `let`s of `bindings`, in order.
fn with_lets(bindings: &[(Local, Expr)], body: Expr) -> Expr {
    bindings
        .iter()
        .rev()
        .fold(body, |body, (var, value)| Expr::Let {
            var: *var,
            value: Box::new(value.clone()),
            body: Box::new(body),
        })
}

/// The number of expression nodes in `e`, as the budget measures it.
fn nodes(e: &Expr) -> usize {
    let mut count = 1;
    e.for_each_child(&mut |c| count += nodes(c));
    count
}

/// The IR function of a bundled core function `kind`: the smallest ID, which is the original copy
/// rather than a `$suspend` one.
///
/// A `List.fold` that is only in the module as a suspending copy is not a `List.fold` this law can
/// call with `suspends` false, so it is not returned.
fn core_fn(m: &Module, core_fns: &CoreFns, kind: CoreFn) -> Option<FnId> {
    core_fns
        .iter()
        .filter(|(_, k)| **k == kind)
        .map(|(&id, _)| id)
        .filter(|&id| !m.functions[id].suspends)
        .min()
}

/// A direct call of a bundled core function, as the call part of a pipeline law's shape.
struct CoreCall<'a> {
    f: &'a FnId,
    ty_args: &'a [TypeId],
    args: &'a [Expr],
    fails: bool,
    suspends: bool,
}

/// The parts of `e` when it is a direct call of the core function `kind` with `type_args` type
/// arguments and `args` arguments.
fn core_call<'a>(
    e: &'a Expr,
    core_fns: &CoreFns,
    kind: CoreFn,
    type_args: usize,
    args: usize,
) -> Option<CoreCall<'a>> {
    let Expr::Call {
        callee: Callee::Direct { f, ty_args },
        args: call_args,
        fails,
        suspends,
    } = e
    else {
        return None;
    };
    if core_fns.get(f) != Some(&kind) || ty_args.len() != type_args || call_args.len() != args {
        return None;
    }
    Some(CoreCall {
        f,
        ty_args,
        args: call_args,
        fails: *fails,
        suspends: *suspends,
    })
}

fn fuse_map_map(
    m: &mut Module,
    e: &Expr,
    core_fns: &CoreFns,
    summaries: &EffectSummaries,
) -> Option<Outcome> {
    compose_map_over(m, e, core_fns, summaries, CoreFn::ListMap)
}

/// The map∘map shape of [`fuse_map_map`], for the bundled map function `kind`.
///
/// [`CoreFn::ListMap`] and [`CoreFn::ArrayMap`] have the same shape: four type arguments (the
/// element type, the result type and the two set parameters), and three arguments (the collection,
/// the function and the evidence table). The composed call is a call of the same function, so the
/// law does not need to know which collection it is.
pub(super) fn compose_map_over(
    m: &mut Module,
    e: &Expr,
    core_fns: &CoreFns,
    summaries: &EffectSummaries,
    kind: CoreFn,
) -> Option<Outcome> {
    let outer = core_call(e, core_fns, kind, 4, 3)?;
    let inner = core_call(&outer.args[0], core_fns, kind, 4, 3)?;
    // From here on the site is this law's shape: a refusal is a deopt, not a miss.
    if outer.fails || outer.suspends || inner.fails || inner.suspends || !is_value(&inner.args[2]) {
        return Some(Outcome::Declined);
    }
    let (a, b) = (inner.ty_args[0], outer.ty_args[1]);
    if inner.ty_args[1] != outer.ty_args[0] {
        return Some(Outcome::Declined);
    }
    let fused_ty = vec![a, b, outer.ty_args[2], outer.ty_args[3]];
    let step_ty = param_type(m, *outer.f, 1, &fused_ty);
    if !pure_step(m, step_ty) {
        return Some(Outcome::Declined);
    }
    let (f_value, g_value) = (&inner.args[1], &outer.args[1]);
    let (Some(f_summary), Some(g_summary)) = (
        function_summary(m, f_value, summaries),
        function_summary(m, g_value, summaries),
    ) else {
        return Some(Outcome::Declined);
    };
    if !quiet(f_summary) || !quiet(g_summary) || (restless(f_summary) && restless(g_summary)) {
        return Some(Outcome::Declined);
    }
    let mut bindings = Vec::new();
    let (Some(f_step), Some(g_step)) = (
        step(m, f_value, &mut bindings),
        step(m, g_value, &mut bindings),
    ) else {
        return Some(Outcome::Declined);
    };
    let Type::Fn(ft) = m.ty(step_ty) else {
        return Some(Outcome::Declined);
    };
    if ft.params != [a] || ft.ret != b {
        return Some(Outcome::Declined);
    }
    let x = m.local("x", a);
    let body = g_step.call(vec![f_step.call(vec![Expr::Var(x)])]);
    let lambda = Expr::Lambda {
        params: vec![x],
        ty: step_ty,
        body: Box::new(body),
        suspends: false,
    };
    if nodes(&lambda) > nodes(f_value) + nodes(g_value) + COMPOSE_OVERHEAD {
        return Some(Outcome::Declined);
    }
    let call = Expr::Call {
        callee: Callee::Direct {
            f: *outer.f,
            ty_args: fused_ty,
        },
        args: vec![inner.args[0].clone(), lambda, outer.args[2].clone()],
        fails: false,
        suspends: false,
    };
    Some(Outcome::Rewritten(with_lets(&bindings, call)))
}

fn fuse_fold_map(
    m: &mut Module,
    e: &Expr,
    core_fns: &CoreFns,
    summaries: &EffectSummaries,
) -> Option<Outcome> {
    compose_fold_over(m, e, core_fns, summaries, CoreFn::ListMap, CoreFn::ListFold)
}

/// The fold∘map shape of [`fuse_fold_map`]: a fold `fold` whose collection argument is a direct
/// call of the bundled map function `map`.
///
/// `List` and `Array` have the same shape here too: both map functions take four type arguments
/// and three arguments, and both folds take four type arguments and four arguments (the
/// collection, the initial accumulator, the step and the evidence table), with the element type
/// first and the accumulator type second.
pub(super) fn compose_fold_over(
    m: &mut Module,
    e: &Expr,
    core_fns: &CoreFns,
    summaries: &EffectSummaries,
    map: CoreFn,
    fold: CoreFn,
) -> Option<Outcome> {
    let outer = core_call(e, core_fns, fold, 4, 4)?;
    let inner = core_call(&outer.args[0], core_fns, map, 4, 3)?;
    if outer.fails || outer.suspends || inner.fails || inner.suspends || !is_value(&inner.args[2]) {
        return Some(Outcome::Declined);
    }
    let (a, b) = (inner.ty_args[0], outer.ty_args[1]);
    if inner.ty_args[1] != outer.ty_args[0] {
        return Some(Outcome::Declined);
    }
    let fused_ty = vec![a, b, outer.ty_args[2], outer.ty_args[3]];
    let step_ty = param_type(m, *outer.f, 2, &fused_ty);
    if !pure_step(m, step_ty) {
        return Some(Outcome::Declined);
    }
    let (f_value, g_value) = (&inner.args[1], &outer.args[2]);
    let (Some(f_summary), Some(g_summary)) = (
        function_summary(m, f_value, summaries),
        function_summary(m, g_value, summaries),
    ) else {
        return Some(Outcome::Declined);
    };
    // The map's calls and `init` swap places: the original evaluates `init` after the whole map.
    // Letting one of the three stop the program is enough to keep the first stop the same; two
    // would let a defect or a divergence of one overtake the other.
    let moving = u32::from(restless(f_summary))
        + u32::from(restless(g_summary))
        + u32::from(!effect::of_expr_in(m, &outer.args[1], summaries).is_pure());
    if !quiet(f_summary) || !quiet(g_summary) || moving > 1 {
        return Some(Outcome::Declined);
    }
    let mut bindings = Vec::new();
    let (Some(f_step), Some(g_step)) = (
        step(m, f_value, &mut bindings),
        step(m, g_value, &mut bindings),
    ) else {
        return Some(Outcome::Declined);
    };
    let Type::Fn(ft) = m.ty(step_ty) else {
        return Some(Outcome::Declined);
    };
    if ft.params != [b, a] || ft.ret != b {
        return Some(Outcome::Declined);
    }
    let acc = m.local("acc", b);
    let x = m.local("x", a);
    let body = g_step.call(vec![Expr::Var(acc), f_step.call(vec![Expr::Var(x)])]);
    let lambda = Expr::Lambda {
        params: vec![acc, x],
        ty: step_ty,
        body: Box::new(body),
        suspends: false,
    };
    if nodes(&lambda) > nodes(f_value) + nodes(g_value) + COMPOSE_OVERHEAD {
        return Some(Outcome::Declined);
    }
    let call = Expr::Call {
        callee: Callee::Direct {
            f: *outer.f,
            ty_args: fused_ty,
        },
        args: vec![
            inner.args[0].clone(),
            outer.args[1].clone(),
            lambda,
            outer.args[3].clone(),
        ],
        fails: false,
        suspends: false,
    };
    Some(Outcome::Rewritten(with_lets(&bindings, call)))
}

fn fuse_filter_map(
    m: &mut Module,
    e: &Expr,
    core_fns: &CoreFns,
    summaries: &EffectSummaries,
) -> Option<Outcome> {
    let outer = core_call(e, core_fns, CoreFn::ListMap, 4, 3)?;
    let filter = core_call(&outer.args[0], core_fns, CoreFn::ListFilter, 3, 3)?;
    if outer.fails
        || outer.suspends
        || filter.fails
        || filter.suspends
        || !is_value(&filter.args[2])
    {
        return Some(Outcome::Declined);
    }
    let a = filter.ty_args[0];
    let r = outer.ty_args[1];
    if outer.ty_args[0] != a {
        return Some(Outcome::Declined);
    }
    let Some(fold) = core_fn(m, core_fns, CoreFn::ListFold) else {
        return Some(Outcome::Declined);
    };
    let Some(reverse) = core_fn(m, core_fns, CoreFn::ListReverse) else {
        return Some(Outcome::Declined);
    };
    let list_r = m.list(r);
    let fold_ty = vec![a, list_r, outer.ty_args[2], outer.ty_args[3]];
    let step_ty = param_type(m, fold, 2, &fold_ty);
    if !pure_step(m, step_ty) {
        return Some(Outcome::Declined);
    }
    let (p_value, f_value) = (&filter.args[1], &outer.args[1]);
    let (Some(p_summary), Some(f_summary)) = (
        function_summary(m, p_value, summaries),
        function_summary(m, f_value, summaries),
    ) else {
        return Some(Outcome::Declined);
    };
    if !quiet(p_summary) || !quiet(f_summary) || (restless(p_summary) && restless(f_summary)) {
        return Some(Outcome::Declined);
    }
    let mut bindings = Vec::new();
    let (Some(p_step), Some(f_step)) = (
        step(m, p_value, &mut bindings),
        step(m, f_value, &mut bindings),
    ) else {
        return Some(Outcome::Declined);
    };
    let Type::Fn(ft) = m.ty(step_ty) else {
        return Some(Outcome::Declined);
    };
    if ft.params != [list_r, a] || ft.ret != list_r {
        return Some(Outcome::Declined);
    }
    let acc = m.local("acc", list_r);
    let x = m.local("x", a);
    let keep = Expr::List {
        elem: r,
        elems: vec![f_step.call(vec![Expr::Var(x)])],
        tail: Some(Box::new(Expr::Var(acc))),
    };
    let body = Expr::If {
        cond: Box::new(p_step.call(vec![Expr::Var(x)])),
        then: Box::new(keep),
        else_: Box::new(Expr::Var(acc)),
    };
    let lambda = Expr::Lambda {
        params: vec![acc, x],
        ty: step_ty,
        body: Box::new(body),
        suspends: false,
    };
    if nodes(&lambda) > nodes(p_value) + nodes(f_value) + COMPOSE_OVERHEAD {
        return Some(Outcome::Declined);
    }
    let fold_call = Expr::Call {
        callee: Callee::Direct {
            f: fold,
            ty_args: fold_ty,
        },
        args: vec![
            filter.args[0].clone(),
            Expr::List {
                elem: r,
                elems: Vec::new(),
                tail: None,
            },
            lambda,
            outer.args[2].clone(),
        ],
        fails: false,
        suspends: false,
    };
    let reverse_call = Expr::Call {
        callee: Callee::Direct {
            f: reverse,
            ty_args: vec![r],
        },
        args: vec![fold_call],
        fails: false,
        suspends: false,
    };
    Some(Outcome::Rewritten(with_lets(&bindings, reverse_call)))
}
