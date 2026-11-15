//! Hoisting of the parts of a loop that do not change between its iterations.
//!
//! `List.fold` and `List.map` over a range become loops ([`super::range`]), and inlining copies
//! the body of the lambda into them, so a step such as `acc + get(a, i, k) * get(b, k, j)` is
//! straight-line code in the loop's body. The bounds test and the row load of `get(a, i, k)` do
//! not depend on `k`: they are the same in every iteration. In the loop they cost more than the
//! multiply they feed, and a backend cannot move them out: the loop also writes reference counts,
//! so the C compiler cannot prove that the array it reads is not one of the objects it writes.
//!
//! The pass does two things, in this order.
//!
//! 1. It copies `let x = y` and `let x = <literal>` out of the loop when `y` is bound outside it.
//!    Inlining leaves a chain of them in front of the work (`let m = a; let i = i0; let a2 = m`),
//!    and the value the pass hoists next is written in terms of the outermost names.
//! 2. It unswitches the bounds test that `super::option::array_get_match` builds for
//!    `match Array.get(a, i)` when `a` and `i` are the same in every iteration (`i` is a variable
//!    bound outside the loop or a literal: `Array.get(a, 0)` in a loop is the same shape).
//!    `loop { if in_range { … } else { … } }` becomes `if in_range { loop { … } } else { loop {
//!    … } }`, and the load that the test guards is copied out of the branch: one bounds test, one
//!    load and one reference for the whole loop instead of one of each per iteration.
//!
//! The element is the same in every iteration because an array never changes while a name holds
//! it: `Array.set` consumes the array and copies it unless that was its last reference, and a
//! loop that reads the array through a name bound outside keeps one. That holds for every element
//! representation, so the pass is the same for an array of values held by value (the C target
//! reads such an element with `em_array_load_unchecked_agg` and copies it once).
//!
//! `Array.get` itself is not hoisted: the `Some` it makes allocates, and hoisting it would add an
//! allocation per loop entry, which the allocation gate counts.
//!
//! # Contract
//!
//! ```text
//! Preconditions: (1) a `let` in a loop body (or in the value of one) whose value is a variable
//!                bound outside the loop or a literal, outside any nested loop, lambda, handler or
//!                escape arm; (2) an `if` in a loop body whose condition is the bounds test of a
//!                fused `Array.get` on an array variable bound outside the loop and an index that
//!                is a variable bound outside or a literal, whose then branch starts with the
//!                matching `ArrayLoadUnchecked`, and whose body is no larger than
//!                [`MAX_UNSWITCH_NODES`].
//! Postconditions: (1) the value is bound once before the loop and the name it bound is bound
//!                there instead of in the body; (2) `loop { if c { A } else { B } }` is
//!                `if c { loop { A } } else { loop { B } }`, with the load of the then branch
//!                bound before that loop. Only one of the two loops runs, and only when the loop
//!                ran at least one iteration does the branch's work happen, so a program's
//!                results, effects, failures and suspension points are unchanged.
//! Preserves: evaluation order, effects, failures, suspension points, and the calls and the release
//!            order of `with`. A `let` value is a variable read or a literal, and an unswitched
//!            test is a comparison and a length read, so running it once instead of once per
//!            iteration changes nothing. No drop point moves except the one the hoist is for: the
//!            reference the loaded element held is taken once and released after the loop instead
//!            of once per iteration. That changes the memory use and the moment a program runs out
//!            of memory, which are implementation-defined (`[effect.handler]`); everything a
//!            program can observe through its results, its effects, its failures and its `with`
//!            releases is the same.
//! Deopt: a value that is not a variable bound outside or a literal; a loop, lambda, handler or
//!        escape arm in the body (its work happens a number of times the pass does not count); a
//!        test that is not the shape the fusion builds, or whose array or index is bound inside
//!        the loop; a body over the size budget, where duplicating the loop would cost more than
//!        the test it removes.
//! Measures: `matmul` (the row of `a` in its `dot`), and the loops of the C target's
//!           by-value-record programs that read a fixed position of a record array.
//! ```

use rustc_hash::FxHashSet;

use crate::intrinsics::Builtin;
use crate::ir::{BinOp, Expr, Lit, Local, Module, OpTy};
use crate::opt::{PassStats, each_body};

/// The largest loop body the pass unswitches, in expressions. The body is copied once, so the
/// budget keeps a function from doubling its size for a test that is cheap next to it. It fits
/// `matmul`'s `dot` (about 55) and a two-deep read of an array of records (about 110) with room
/// left, and stops before a body whose copy would be a function of its own.
const MAX_UNSWITCH_NODES: usize = 160;

/// Copies the values and the bounds test that do not change out of a loop.
///
/// See the module docs for the contract. Neither [`super::range`] nor inlining runs here: the
/// pass has to see the `match Array.get` before the Option fusion turns it into a bounds test, and
/// the loop before inlining puts the callee's body in it.
pub(super) fn hoist(m: &mut Module) -> PassStats {
    let mut changed = 0;
    each_body(m, &mut |_, e| rewrite(e, &mut changed));
    PassStats {
        hoist: changed,
        ..PassStats::default()
    }
}

fn rewrite(e: &mut Expr, changed: &mut u32) {
    crate::tail::for_each_child_mut(e, &mut |c| rewrite(c, changed));
    if !matches!(e, Expr::Loop { .. }) {
        return;
    }
    // (1) first: the test of (2) is written in terms of the names (1) moves out.
    while hoist_values(e) {
        *changed += 1;
    }
    // (1) left the loop inside the `let`s it moved, so (2) looks through them rather than waiting
    // for the next sweep.
    let loop_ = innermost_loop(e);
    if let Some(unswitched) = unswitch(loop_) {
        *changed += 1;
        *loop_ = unswitched;
    }
}

/// The loop at the bottom of the chain of `let`s that [`hoist_values`] built.
fn innermost_loop(e: &mut Expr) -> &mut Expr {
    let mut e = e;
    while let Expr::Let { body, .. } = e {
        e = &mut **body;
    }
    e
}

/// Copies every `let x = y` and `let x = <literal>` of the loop's body out of it. Returns whether
/// it moved one.
fn hoist_values(e: &mut Expr) -> bool {
    let Expr::Loop { vars, body, .. } = e else {
        return false;
    };
    let mut bound: FxHashSet<Local> = vars.iter().copied().collect();
    let mut moved = Vec::new();
    bindings(body, &mut bound);
    take_values(body, &mut bound, &mut moved);
    if moved.is_empty() {
        return false;
    }
    let loop_ = std::mem::replace(e, Expr::Lit(Lit::Unit));
    let mut inner = loop_;
    // The first value taken is the outermost: it is written in terms of names that are still
    // outside the loop, and a later one may use it.
    for (var, value) in moved.into_iter().rev() {
        inner = Expr::Let {
            var,
            value: Box::new(value),
            body: Box::new(inner),
        };
    }
    *e = inner;
    true
}

/// Takes the values that [`hoist_values`] may move, in evaluation order, removing them from the
/// body and their names from `bound`.
fn take_values(e: &mut Expr, bound: &mut FxHashSet<Local>, moved: &mut Vec<(Local, Expr)>) {
    if let Expr::Let { var, value, body } = e {
        let movable = match &**value {
            Expr::Lit(_) => true,
            Expr::Var(v) => !bound.contains(v),
            _ => false,
        };
        if movable {
            moved.push((*var, (**value).clone()));
            bound.remove(var);
            let taken = std::mem::replace(e, Expr::Lit(Lit::Unit));
            if let Expr::Let { body, .. } = taken {
                *e = *body;
            }
            take_values(e, bound, moved);
            return;
        }
        // The value holds the `let`s of a call that was inlined into it (`p = get(points, 0)`
        // keeps `let a = points; let i = 0` inside the value of `let p`), so look in it too.
        take_values(value, bound, moved);
        take_values(body, bound, moved);
        return;
    }
    // A nested loop, a lambda, a handler or an escape arm may run its body a number of times this
    // pass does not count, so nothing inside one is moved.
    if matches!(
        e,
        Expr::Loop { .. }
            | Expr::Lambda { .. }
            | Expr::Handler { .. }
            | Expr::With { .. }
            | Expr::Escape { .. }
    ) {
        return;
    }
    crate::tail::for_each_child_mut(e, &mut |c| take_values(c, bound, moved));
}

/// The locals bound inside a loop's body: its variables, every `let` in it, the parameters of the
/// lambdas in it, and every name a pattern or a `with` binds.
fn bindings(e: &mut Expr, bound: &mut FxHashSet<Local>) {
    match e {
        Expr::Let { var, value, body } => {
            bound.insert(*var);
            bindings(value, bound);
            bindings(body, bound);
        }
        Expr::Loop { vars, inits, body } => {
            bound.extend(vars.iter().copied());
            for init in inits {
                bindings(init, bound);
            }
            bindings(body, bound);
        }
        Expr::Lambda { params, body, .. } => {
            bound.extend(params.iter().copied());
            bindings(body, bound);
        }
        Expr::Match { scrutinee, arms } => {
            bindings(scrutinee, bound);
            for arm in arms {
                pattern(&arm.pat, bound);
                if let Some(guard) = &mut arm.guard {
                    bindings(guard, bound);
                }
                bindings(&mut arm.body, bound);
            }
        }
        Expr::Escape { body, arms } => {
            bindings(body, bound);
            for arm in arms {
                pattern(&arm.pat, bound);
                if let Some(guard) = &mut arm.guard {
                    bindings(guard, bound);
                }
                bindings(&mut arm.body, bound);
            }
        }
        Expr::With { binds, body } => {
            for bind in binds {
                bound.insert(bind.var);
                bindings(&mut bind.value, bound);
                if let Some(release) = &mut bind.release {
                    bindings(&mut release.body, bound);
                }
            }
            bindings(body, bound);
        }
        _ => crate::tail::for_each_child_mut(e, &mut |c| bindings(c, bound)),
    }
}

/// The locals `pat` binds.
fn pattern(pat: &crate::ir::Pat, bound: &mut FxHashSet<Local>) {
    use crate::ir::Pat;
    match pat {
        Pat::Wild | Pat::Lit(_) => {}
        Pat::Bind(local) => {
            bound.insert(*local);
        }
        Pat::Ctor { fields, .. } | Pat::Tuple(fields) => {
            for field in fields {
                pattern(field, bound);
            }
        }
        Pat::List { elems, rest } => {
            for elem in elems {
                pattern(elem, bound);
            }
            if let Some(rest) = rest {
                pattern(rest, bound);
            }
        }
    }
}

/// `loop { if c { A } else { B } }` as `if c { loop { A } } else { loop { B } }`, when `c` is the
/// bounds test of a fused `Array.get` on a variable and an index bound outside the loop, and the
/// load the test guards leaves the loop with it.
fn unswitch(e: &mut Expr) -> Option<Expr> {
    let Expr::Loop { vars, inits, body } = e else {
        return None;
    };
    let mut bound: FxHashSet<Local> = vars.iter().copied().collect();
    bindings(body, &mut bound);
    if count(body) > MAX_UNSWITCH_NODES {
        return None;
    }
    let test = find_test(body, &bound)?;
    let mut then_body = (**body).clone();
    replace_test(&mut then_body, &bound, true)?;
    let mut else_body = (**body).clone();
    replace_test(&mut else_body, &bound, false)?;
    let mut then_loop = Expr::Loop {
        vars: vars.clone(),
        inits: inits.clone(),
        body: Box::new(then_body),
    };
    // The test holds in the then branch, so the load it guards runs in bounds whether or not the
    // loop runs: it can leave the loop.
    hoist_guarded_load(&mut then_loop, test.array, &test.index, test.value);
    Some(Expr::If {
        cond: test.cond,
        then: Box::new(then_loop),
        else_: Box::new(Expr::Loop {
            vars: vars.clone(),
            inits: inits.clone(),
            body: Box::new(else_body),
        }),
    })
}

/// The parts of the `if` that [`unswitch`] moves.
struct FusedTest {
    cond: Box<Expr>,
    array: Local,
    /// The index the test and the load share: a variable bound outside the loop, or a literal
    /// (`Array.get(a, 0)` in a loop is the same shape with a constant index).
    index: Expr,
    /// The local the then branch binds to the loaded element.
    value: Local,
}

/// The first `if` in `e` that is the bounds test of a fused `Array.get`, outside any nested loop
/// or lambda, whose array and index are not bound in `bound`.
///
/// The shape is the one `super::option::array_get_match` builds:
///
/// ```text
/// if (i >= 0) && (i < Array.size(a)) {
///   let x = ArrayLoadUnchecked(a, i)
///   …
/// } else {
///   …
/// }
/// ```
fn find_test(e: &mut Expr, bound: &FxHashSet<Local>) -> Option<FusedTest> {
    if let Some(test) = as_fused_test(e, bound) {
        return Some(test);
    }
    if matches!(e, Expr::Loop { .. } | Expr::Lambda { .. }) {
        return None;
    }
    let mut found = None;
    crate::tail::for_each_child_mut(e, &mut |c| {
        if found.is_none() {
            found = find_test(c, bound);
        }
    });
    found
}

/// `e` as a [`FusedTest`], when it is one and its variables are outside `bound`.
fn as_fused_test(e: &Expr, bound: &FxHashSet<Local>) -> Option<FusedTest> {
    let Expr::If { cond, then, .. } = e else {
        return None;
    };
    let (index, array) = in_range(cond)?;
    if !outside(&index, bound) || bound.contains(&array) {
        return None;
    }
    let Expr::Let { var, value, .. } = &**then else {
        return None;
    };
    let Expr::Builtin {
        op: Builtin::ArrayLoadUnchecked,
        args,
        ..
    } = &**value
    else {
        return None;
    };
    match (args.first(), args.get(1)) {
        (Some(Expr::Var(a)), Some(i)) if *a == array && *i == index => Some(FusedTest {
            cond: cond.clone(),
            array,
            index,
            value: *var,
        }),
        _ => None,
    }
}

/// Whether the index of a bounds test is the same in every iteration: a literal, or a variable
/// bound outside the loop.
fn outside(index: &Expr, bound: &FxHashSet<Local>) -> bool {
    match index {
        Expr::Lit(_) => true,
        Expr::Var(v) => !bound.contains(v),
        _ => false,
    }
}

/// The `(index, array)` of the bounds test `(i >= 0) && (i < Array.size(a))`.
fn in_range(cond: &Expr) -> Option<(Expr, Local)> {
    let Expr::Binary {
        op: BinOp::And,
        ty: OpTy::Bool,
        lhs,
        rhs,
    } = cond
    else {
        return None;
    };
    let Expr::Binary {
        op: BinOp::Ge,
        ty: OpTy::Int,
        lhs: lower,
        rhs: zero,
    } = &**lhs
    else {
        return None;
    };
    let index = match &**lower {
        Expr::Var(_) | Expr::Lit(_) => (**lower).clone(),
        _ => return None,
    };
    let Expr::Lit(Lit::Int(0)) = &**zero else {
        return None;
    };
    let Expr::Binary {
        op: BinOp::Lt,
        ty: OpTy::Int,
        lhs: upper,
        rhs: size,
    } = &**rhs
    else {
        return None;
    };
    if **upper != index {
        return None;
    }
    let Expr::Builtin {
        op: Builtin::ArraySize,
        args,
        ..
    } = &**size
    else {
        return None;
    };
    let Some(Expr::Var(array)) = args.first() else {
        return None;
    };
    Some((index, *array))
}

/// Replaces the first fused test in `e` by its then branch (`then`) or its else branch.
fn replace_test(e: &mut Expr, bound: &FxHashSet<Local>, then: bool) -> Option<()> {
    if as_fused_test(e, bound).is_some() {
        let taken = std::mem::replace(e, Expr::Lit(Lit::Unit));
        let Expr::If {
            then: then_branch,
            else_: else_branch,
            ..
        } = taken
        else {
            unreachable!("as_fused_test matched an `if`")
        };
        *e = if then { *then_branch } else { *else_branch };
        return Some(());
    }
    if matches!(e, Expr::Loop { .. } | Expr::Lambda { .. }) {
        return None;
    }
    let mut found = None;
    crate::tail::for_each_child_mut(e, &mut |c| {
        if found.is_none() {
            found = replace_test(c, bound, then);
        }
    });
    found
}

/// Copies `let value = ArrayLoadUnchecked(array, index)` out of the loop `e`.
fn hoist_guarded_load(e: &mut Expr, array: Local, index: &Expr, value: Local) {
    let Expr::Loop { body, .. } = e else {
        return;
    };
    let mut load = None;
    take_guarded_load(body, array, index, value, &mut load);
    let Some(load) = load else {
        return;
    };
    let loop_ = std::mem::replace(e, Expr::Lit(Lit::Unit));
    *e = Expr::Let {
        var: value,
        value: Box::new(load),
        body: Box::new(loop_),
    };
}

fn take_guarded_load(
    e: &mut Expr,
    array: Local,
    index: &Expr,
    value_var: Local,
    load: &mut Option<Expr>,
) {
    if load.is_some() {
        return;
    }
    if let Expr::Let {
        var,
        value: v,
        body,
    } = e
    {
        let matched = *var == value_var
            && matches!(
                &**v,
                Expr::Builtin {
                    op: Builtin::ArrayLoadUnchecked,
                    args,
                    ..
                } if matches!(
                    (args.first(), args.get(1)),
                    (Some(Expr::Var(a)), Some(i)) if *a == array && *i == *index
                )
            );
        if matched {
            *load = Some((**v).clone());
            let taken = std::mem::replace(e, Expr::Lit(Lit::Unit));
            if let Expr::Let { body, .. } = taken {
                *e = *body;
            }
            return;
        }
        // The value of a `let` may hold the load (`p = Array.get(a, 0) match { … }` keeps the
        // match, and the load the test guards, inside the value of `let p`).
        take_guarded_load(v, array, index, value_var, load);
        take_guarded_load(body, array, index, value_var, load);
        return;
    }
    if matches!(e, Expr::Loop { .. } | Expr::Lambda { .. }) {
        return;
    }
    crate::tail::for_each_child_mut(e, &mut |c| {
        if load.is_none() {
            take_guarded_load(c, array, index, value_var, load);
        }
    });
}

/// The number of expressions in `e`.
fn count(e: &mut Expr) -> usize {
    let mut n = 1;
    crate::tail::for_each_child_mut(e, &mut |c| n += count(c));
    n
}
