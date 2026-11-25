//! Loop transformations: LICM, unswitching and induction variables.
//!
//! These transformations are not reductions. They move work between the inside and the outside of
//! a loop, or change the counter that a loop walks, so their contracts name the loop they apply to
//! and the evaluations they move instead of an equation between expressions.
//!
//! The module has one implemented transformation:
//!
//! - **Unswitching**: an `if` inside a loop whose condition is invariant is lifted around the loop,
//!   so the test runs once. [`unswitch`] performs it for the bounds test that
//!   [`super::fusion`]'s `array_get_match` builds for `match Array.get(a, i)`, and copies the load
//!   the test guards out of the loop with it.
//!
//! Two transformations are reserved for later work:
//!
//! - **Loop-invariant code motion (LICM)**: a computation in a loop whose operands are bound
//!   outside the loop is evaluated once before it. Copy propagation is not LICM: a `let x = y`
//!   whose `y` is bound outside is a ζ reduction ([`super::reduce::zeta`]), which removes the
//!   binding wherever it is, not a loop transform.
//! - **Induction variables**: a counter whose value is an affine function of the loop counter is
//!   replaced by an affine update, or a derived counter is removed when the base counter and the
//!   step are known. B5/PR-024 supplies the ranges this needs.
//!
//! # Contract: unswitching the bounds test
//!
//! ```text
//! Rule ID: loop.unswitch
//! Theory: loop unswitching of an invariant condition
//! Equation: loop { if c { A } else { B } } → if c { loop { A } } else { loop { B } }
//! Preconditions: an `if` in a loop body whose condition is the bounds test of a fused
//!                `Array.get` on an array variable bound outside the loop and an index that is a
//!                variable bound outside or a literal, whose then branch starts with the matching
//!                `ArrayLoadUnchecked`, and whose body is no larger than [`MAX_UNSWITCH_NODES`].
//! Postconditions: only one of the two loops runs, and only when the loop ran at least one
//!                 iteration does the branch's work happen, so a program's results, effects,
//!                 failures and suspension points are unchanged. The load the test guards is
//!                 bound once before the then loop instead of once per iteration.
//! Preserves: evaluation order, effects, failures, suspension points, and the calls and the release
//!            order of `with`. The test is a comparison and a length read, so running it once
//!            instead of once per iteration changes nothing. No drop point moves except the one the
//!            unswitch is for: the reference the loaded element held is taken once and released
//!            after the loop instead of once per iteration. That changes the memory use and the
//!            moment a program runs out of memory, which are implementation-defined
//!            (`[effect.handler]`); everything a program can observe through its results, its
//!            effects, its failures and its `with` releases is the same.
//! Deopt: a condition that is not the shape the fusion builds, or whose array or index is bound
//!        inside the loop; a body over the size budget, where duplicating the loop would cost
//!        more than the test it removes.
//! Budget: at most [`MAX_UNSWITCH_NODES`] expressions in the loop body, because the body is
//!         copied into both arms of the `if`.
//! Measures: [`crate::opt::Stats::hoist`]; `matmul` (the row of `a` in its `dot`), and the loops
//!           of the C target's by-value-record programs that read a fixed position of a record
//!           array.
//! ```
//!
//! [`unswitch`] runs before δ and the fusions in the pipeline; see [`crate::opt`].

use rustc_hash::FxHashSet;

use crate::intrinsics::Builtin;
use crate::ir::{BinOp, Expr, Lit, Local, Module, OpTy};
use crate::opt::{PassStats, each_body};

/// The largest loop body the pass unswitches, in expressions. The body is copied once, so the
/// budget keeps a function from doubling its size for a test that is cheap next to it. It fits
/// `matmul`'s `dot` (about 55) and a two-deep read of an array of records (about 110) with room
/// left, and stops before a body whose copy would be a function of its own.
const MAX_UNSWITCH_NODES: usize = 160;

/// Lifts the bounds test of a fused `Array.get` out of every loop it is invariant in.
///
/// See the module docs for the contract. The pass has to see the `match Array.get` that
/// [`super::fusion`]'s `array_get_match` turned into a bounds test on the previous sweep, and it
/// runs before δ so that it sees the loop before a callee's body is copied into it.
pub(in crate::opt) fn unswitch(m: &mut Module) -> PassStats {
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
    if let Some(unswitched) = unswitch_loop(e) {
        *changed += 1;
        *e = unswitched;
    }
}

/// `loop { if c { A } else { B } }` as `if c { loop { A } } else { loop { B } }`, when `c` is the
/// bounds test of a fused `Array.get` on a variable and an index bound outside the loop, and the
/// load the test guards leaves the loop with it.
fn unswitch_loop(e: &mut Expr) -> Option<Expr> {
    let Expr::Loop { vars, inits, body } = e else {
        return None;
    };
    let mut bound: FxHashSet<Local> = vars.iter().copied().collect();
    bound_locals(body, &mut bound);
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

/// The parts of the `if` that [`unswitch_loop`] moves.
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
/// The shape is the one `super::fusion`'s `array_get_match` builds:
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

/// The locals bound inside a loop's body: its variables, every `let` in it, the parameters of the
/// lambdas in it, and every name a pattern or a `with` binds.
///
/// The unswitch only asks whether an array or an index name is bound inside the loop, so a
/// conservative superset is enough; it never rewrites a name it thinks is bound elsewhere.
fn bound_locals(e: &Expr, bound: &mut FxHashSet<Local>) {
    match e {
        Expr::Let { var, value, body } => {
            bound.insert(*var);
            bound_locals(value, bound);
            bound_locals(body, bound);
        }
        Expr::Loop { vars, inits, body } => {
            bound.extend(vars.iter().copied());
            for init in inits {
                bound_locals(init, bound);
            }
            bound_locals(body, bound);
        }
        Expr::Lambda { params, body, .. } => {
            bound.extend(params.iter().copied());
            bound_locals(body, bound);
        }
        Expr::Match { scrutinee, arms } => {
            bound_locals(scrutinee, bound);
            for arm in arms {
                pattern(&arm.pat, bound);
                if let Some(guard) = &arm.guard {
                    bound_locals(guard, bound);
                }
                bound_locals(&arm.body, bound);
            }
        }
        Expr::Escape { body, arms } => {
            bound_locals(body, bound);
            for arm in arms {
                pattern(&arm.pat, bound);
                if let Some(guard) = &arm.guard {
                    bound_locals(guard, bound);
                }
                bound_locals(&arm.body, bound);
            }
        }
        Expr::With { binds, body } => {
            for bind in binds {
                bound.insert(bind.var);
                bound_locals(&bind.value, bound);
                if let Some(release) = &bind.release {
                    bound_locals(&release.body, bound);
                }
            }
            bound_locals(body, bound);
        }
        _ => e.for_each_child(&mut |c| bound_locals(c, bound)),
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

/// The number of expressions in `e`.
fn count(e: &mut Expr) -> usize {
    let mut n = 1;
    crate::tail::for_each_child_mut(e, &mut |c| n += count(c));
    n
}
