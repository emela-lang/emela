//! Bounds-check elimination on a fused `Array.get`.
//!
//! The Option fusion turns `match Array.get(a, i) { Some(x) -> …; None -> … }` into a bounds test
//! that guards the load (`super::super::option`, law `array_get_match`):
//!
//! ```text
//! if (i >= 0) && (i < Array.size(a)) {
//!   let x = ArrayLoadUnchecked(a, i)
//!   …
//! } else {
//!   …
//! }
//! ```
//!
//! When the index is provably in `0 .. Array.size(a)`, the test is redundant: this rule replaces
//! the `if` by its then branch, the load, and the check is gone.
//!
//! # What survives the other passes
//!
//! The loop unswitch ([`crate::opt::r#loop`]) moves a bounds test out of a loop only when the array
//! and the index are invariant in the loop. A test whose index is the loop counter cannot leave the
//! loop. The range fusion builds exactly that shape for a fold over a range, and the default
//! pipeline leaves the test in the loop:
//!
//! ```text
//! let a: Array[Int] = Array.from_list([10, 20, 30])
//! loop(i: Int = 0, acc: Int = 0) {
//!   if i < 3 {
//!     recur(i + 1, acc + if (i >= 0) && (i < Array.size(a)) {
//!       let x = ArrayLoadUnchecked(a, i)
//!       x
//!     } else {
//!       0
//!     })
//!   } else {
//!     acc
//!   }
//! }
//! ```
//!
//! `i` is in `0 ..= 2` inside the guard and `a` has three elements, so the inner test always
//! holds. The unswitch cannot remove it (the index changes each iteration) and the Option fusion
//! has already done its part. That is this rule's residual case. The straight-line case is the same
//! shape with a literal index: `match Array.get(Array.from_list([10, 20, 30]), 1) { … }` leaves a
//! test the constant folder cannot settle, because `Array.size` is not a constant it folds.
//!
//! # Contract
//!
//! ```text
//! Rule ID: reduce.bounds
//! Theory: interval analysis (B5) on the index and a statically known array length
//! Equation: if (i >= 0) && (i < Array.size(a)) { let x = ArrayLoadUnchecked(a, i); e } else { d }
//!           → let x = ArrayLoadUnchecked(a, i); e      when [lo, hi](i) ⊆ [0, len(a))
//! Preconditions: the test is the bounds test the Option fusion builds for one array and one
//!                index, the index is a variable or a scalar literal, and the then branch begins
//!                with the matching `ArrayLoadUnchecked`. The interval of the index, from the
//!                ranges of the enclosing loop's induction variable and of the `let` bindings on
//!                the path, is a subset of `0 .. len(a)`. `len(a)` is known because `a` is bound,
//!                possibly through copies, to `Array.from_list` of a list literal without a tail,
//!                or to `ArrayMapRange(lo, hi, f)` with constant `lo` and `hi`.
//! Postconditions: the test and its else branch are gone; the load and the then branch stay in
//!                 place. The value of the expression is the then branch's value.
//! Preserves: evaluation order, effects, failures, suspension points, and the order and count of
//!            the releases of `with`. The removed condition is a comparison, an `Array.size` and a
//!            conjunction of pure operations, so it has no effect and no defect to lose: it was
//!            evaluated before the then branch and only reads values the load also uses. The else
//!            branch did not run on an index the interval proves is in bounds. No drop point moves.
//! Deopt: an index whose interval is `Top` or `Bottom` (an `Unknown` answer, a parameter, a value
//!        the transfer does not model), whose lower bound is below 0, or whose upper bound is
//!        `len(a)` or above; an array whose length is not known from its binding (`Array.size` of
//!        a parameter, a `let`-bound array produced by anything but the two shapes above, an array
//!        a loop carries); a test the `ArrayGetOr` fusion built, which has no unchecked form that
//!        keeps its default argument's evaluation.
//! Budget: none: the rule only removes expressions.
//! Measures: [`crate::opt::Stats::bounds`]; the array loops whose index is a loop counter over a
//!           known number of iterations and whose array is built from a literal list or a
//!           constant `ArrayMapRange`.
//! ```
//!
//! # Why the interval comes from the path, not from [`ranges::FnRanges::ranges`]
//!
//! [`ranges::FnRanges::ranges`] is the hull of every interval the analysis recorded for a local,
//! including
//! the narrowed intervals of branch conditions on both sides. A local narrowed to `[0, 1]` under
//! one guard and to `[2, 3]` under its negation has the hull `[0, 3]`, which is not what a consumer
//! needs at a `let` binding outside the branch. This rule therefore walks the function with its own
//! environment: a `let` binds the interval of its value, an `if` narrows a copy of the environment
//! for each branch, and an enclosing loop binds its induction variable to the interval its trip
//! count proves. The transfer itself is B5's ([`ranges::of_expr`]).
//!
//! # Why the loop variable's interval covers the exit value
//!
//! [`Induction::iterations`] counts the back edges the guard allows. The loop body still runs once
//! with the counter at the value that fails the guard, so the interval bound from the induction is
//! `init ..= init + iterations * step`, not `init ..= init + (iterations - 1) * step`. A body that
//! reads the counter only inside the guard's then branch narrows it back to the smaller interval;
//! a body that reads it in the else branch keeps the wider one and deopts.
//!
//! A loop nested in another gets one entry per round of the outer loop's fixed point. The rounds
//! before the outer loop settles are narrower than any value the program can reach, so the rule
//! takes the hull of every entry: it contains the entry of the settling round, which is the sound
//! one. Taking a single entry could prove a bound from an early round that no longer holds.

use rustc_hash::{FxHashMap, FxHashSet};

use crate::intrinsics::Builtin;
use crate::ir::{BinOp, Expr, FnId, Lit, Local, Module, OpTy};

use super::super::RuleStats;
use super::super::analysis::Fact;
use super::super::analysis::ranges::{self, Env, Induction, Interval};

/// Removes the bounds test of a fused `Array.get` when the index is provably in the array.
///
/// See the module documentation for the contract.
pub(in crate::opt) fn bounds(m: &mut Module) -> RuleStats {
    let mut applied = 0;
    let mut deopt = 0;
    let ids: Vec<FnId> = m.functions.iter().map(|(id, _)| id).collect();
    for id in ids {
        // The lengths of the array locals and the induction variables are facts about the body as
        // it is now. The pass only removes `if`s, so the lengths stay the same while it walks.
        let (lengths, inductions) = {
            let body = &m.functions[id].body;
            let bindings = bindings_of(body);
            let mut lengths = FxHashMap::default();
            let mut constants = FxHashMap::default();
            let mut visiting = FxHashSet::default();
            // Constants first: an `ArrayMapRange`'s length is read from its `lo` and `hi`, which
            // the range fusion binds to fresh locals.
            for &var in bindings.keys() {
                local_constant(m, &bindings, &mut constants, &mut visiting, var);
            }
            for &var in bindings.keys() {
                local_length(m, &bindings, &mut lengths, &constants, &mut visiting, var);
            }
            let inductions = match ranges::inductions(m, id) {
                Fact::Known(inductions) => inductions,
                Fact::Unknown => Vec::new(),
            };
            (lengths, inductions)
        };
        let mut body = std::mem::replace(&mut m.functions[id].body, Expr::Lit(Lit::Unit));
        let context = Context {
            m,
            lengths: &lengths,
            inductions: &inductions,
        };
        let mut env = Env::new();
        context.explore(&mut body, &mut env, &mut applied, &mut deopt);
        m.functions[id].body = body;
    }
    RuleStats { applied, deopt }
}

/// The facts one function's walk needs.
struct Context<'a> {
    m: &'a Module,
    /// The length of every array local the walk could prove, from its binding.
    lengths: &'a FxHashMap<Local, usize>,
    /// The induction variables of the function, from [`ranges::inductions`].
    inductions: &'a [Induction],
}

impl Context<'_> {
    /// Walks `e` with `env`, rewrites the sites it can prove, and counts both.
    fn explore(&self, e: &mut Expr, env: &mut Env, applied: &mut u32, deopt: &mut u32) {
        match e {
            Expr::Let { var, value, body } => {
                let base = env.clone();
                self.explore(value, env, applied, deopt);
                let interval = ranges::of_expr(self.m, value, env);
                env.set(*var, interval);
                self.explore(body, env, applied, deopt);
                *env = base;
            }
            Expr::If { .. } => {
                if self.rewrite_if(e, env, applied, deopt) {
                    return;
                }
                let Expr::If { cond, then, else_ } = e else {
                    unreachable!("the match is on an `if`")
                };
                self.explore(cond, env, applied, deopt);
                let base = env.clone();
                self.narrow(cond, true, env);
                self.explore(then, env, applied, deopt);
                *env = base.clone();
                self.narrow(cond, false, env);
                self.explore(else_, env, applied, deopt);
                *env = base;
            }
            Expr::Match { scrutinee, arms } => {
                let base = env.clone();
                self.explore(scrutinee, env, applied, deopt);
                for arm in arms {
                    *env = base.clone();
                    self.bind_pattern(&arm.pat, env);
                    if let Some(guard) = &mut arm.guard {
                        self.explore(guard, env, applied, deopt);
                    }
                    self.explore(&mut arm.body, env, applied, deopt);
                }
                *env = base;
            }
            Expr::Escape { body, arms } => {
                let base = env.clone();
                self.explore(body, env, applied, deopt);
                for arm in arms {
                    *env = base.clone();
                    self.bind_pattern(&arm.pat, env);
                    if let Some(guard) = &mut arm.guard {
                        self.explore(guard, env, applied, deopt);
                    }
                    self.explore(&mut arm.body, env, applied, deopt);
                }
                *env = base;
            }
            Expr::Loop { vars, inits, body } => {
                let base = env.clone();
                for init in inits {
                    self.explore(init, env, applied, deopt);
                }
                for &var in vars.iter() {
                    env.set(var, self.loop_interval(var));
                }
                self.explore(body, env, applied, deopt);
                *env = base;
            }
            Expr::Lambda { params, body, .. } => {
                let base = env.clone();
                for &param in params.iter() {
                    env.set(param, Interval::Top);
                }
                self.explore(body, env, applied, deopt);
                *env = base;
            }
            Expr::With { binds, body } => {
                let base = env.clone();
                for bind in binds {
                    self.explore(&mut bind.value, env, applied, deopt);
                    let interval = ranges::of_expr(self.m, &bind.value, env);
                    env.set(bind.var, interval);
                    if let Some(release) = &mut bind.release {
                        self.explore(&mut release.body, env, applied, deopt);
                    }
                }
                self.explore(body, env, applied, deopt);
                *env = base;
            }
            // No other constructor binds a local. Its children are evaluated with the current
            // environment, and a child that does not bind cannot narrow what its siblings see.
            _ => crate::tail::for_each_child_mut(e, &mut |child| {
                self.explore(child, env, applied, deopt);
            }),
        }
    }

    /// Rewrites the `if` at `e` when it is a provable bounds test, and returns whether it did.
    ///
    /// On a deopt the site is counted here and `e` is left for the caller to walk normally.
    fn rewrite_if(&self, e: &mut Expr, env: &mut Env, applied: &mut u32, deopt: &mut u32) -> bool {
        // The proof only borrows the expression; the condition is copied before the `if` is
        // replaced, so the replacement does not hold a borrow of what it overwrites.
        let (index, array, cond) = {
            let Expr::If { cond, then, .. } = &*e else {
                return false;
            };
            let Some((index, array)) = checked_test(cond, then) else {
                return false;
            };
            (index, array, (**cond).clone())
        };
        let provable = self
            .length(array)
            .is_some_and(|len| in_bounds(ranges::of_expr(self.m, &index, env), len));
        if !provable {
            *deopt += 1;
            return false;
        }
        let Expr::If { then, .. } = std::mem::replace(e, Expr::Lit(Lit::Unit)) else {
            unreachable!("the caller matched an `if`")
        };
        *e = *then;
        *applied += 1;
        // The then branch runs only under the condition, so its nested sites may use its
        // narrowing. The narrowing is not needed for the proof of this site.
        let base = env.clone();
        self.narrow(&cond, true, env);
        self.explore(e, env, applied, deopt);
        *env = base;
        true
    }

    /// The known length of the array a bounds test reads, or `None`.
    fn length(&self, array: Local) -> Option<usize> {
        self.lengths.get(&array).copied()
    }

    /// The interval of a loop variable over the whole body of its loop, from its induction
    /// entries.
    ///
    /// `Top` when the loop has no recognised induction variable, or no fixed trip count. See the
    /// module documentation for why the interval includes the value that fails the guard.
    ///
    /// A loop nested in another is transferred once per round of the outer loop's fixed point, so
    /// it can have one entry per round. The rounds before the outer loop settles are narrower than
    /// the program can reach; the entry of the settling round is the sound one, and the hull of
    /// all entries contains it. Taking one entry would let an early round prove a bound the final
    /// round does not.
    fn loop_interval(&self, var: Local) -> Interval {
        let mut interval = Interval::Bottom;
        let mut found = false;
        for induction in self.inductions.iter().filter(|i| i.var == var) {
            found = true;
            let Some(entry) = entry_interval(induction) else {
                return Interval::Top;
            };
            interval = interval.join(entry);
        }
        if found { interval } else { Interval::Top }
    }

    /// Binds the locals a pattern binds. Their intervals are `Top`: a field or element of a value
    /// the analysis does not split is not bounded by the scrutinee's interval.
    fn bind_pattern(&self, pat: &crate::ir::Pat, env: &mut Env) {
        use crate::ir::Pat;
        match pat {
            Pat::Wild | Pat::Lit(_) => {}
            Pat::Bind(local) => env.set(*local, Interval::Top),
            Pat::Ctor { fields, .. } | Pat::Tuple(fields) => {
                for field in fields {
                    self.bind_pattern(field, env);
                }
            }
            Pat::List { elems, rest } => {
                for elem in elems {
                    self.bind_pattern(elem, env);
                }
                if let Some(rest) = rest {
                    self.bind_pattern(rest, env);
                }
            }
        }
    }

    /// Narrows `env` with the comparison `cond` for the branch where it is `truth`.
    ///
    /// This is the consumer's copy of the narrowing [`ranges`] does for an `if`: only a comparison
    /// on `Int` or `Int64`, only an operand that is a plain variable, and only when the other
    /// operand has a bound. A narrowing that is not provable does nothing.
    fn narrow(&self, cond: &Expr, truth: bool, env: &mut Env) {
        let Expr::Binary { op, ty, lhs, rhs } = cond else {
            return;
        };
        if !matches!(
            op,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
        ) {
            return;
        }
        let (type_lo, type_hi) = match ty {
            OpTy::Int => (i32::MIN as i128, i32::MAX as i128),
            OpTy::Int64 => (i64::MIN as i128, i64::MAX as i128),
            OpTy::Float | OpTy::Bool | OpTy::String | OpTy::Structural => return,
        };
        let op = if truth { *op } else { negate(*op) };
        let left = ranges::of_expr(self.m, lhs, env);
        let right = ranges::of_expr(self.m, rhs, env);
        match op {
            BinOp::Lt => {
                narrow_hi(env, lhs, right.hi().map(|v| v - 1), type_lo, type_hi);
                narrow_lo(env, rhs, left.lo().map(|v| v + 1), type_lo, type_hi);
            }
            BinOp::Le => {
                narrow_hi(env, lhs, right.hi(), type_lo, type_hi);
                narrow_lo(env, rhs, left.lo(), type_lo, type_hi);
            }
            BinOp::Gt => {
                narrow_lo(env, lhs, right.lo().map(|v| v + 1), type_lo, type_hi);
                narrow_hi(env, rhs, left.hi().map(|v| v - 1), type_lo, type_hi);
            }
            BinOp::Ge => {
                narrow_lo(env, lhs, right.lo(), type_lo, type_hi);
                narrow_hi(env, rhs, left.hi(), type_lo, type_hi);
            }
            BinOp::Eq => {
                narrow_meet(env, lhs, right, type_lo, type_hi);
                narrow_meet(env, rhs, left, type_lo, type_hi);
            }
            BinOp::Ne
            | BinOp::And
            | BinOp::Or
            | BinOp::Add
            | BinOp::Sub
            | BinOp::Mul
            | BinOp::Div
            | BinOp::Rem => {}
        }
    }
}

/// Restricts `e` (a variable) to a largest value of `hi`.
fn narrow_hi(env: &mut Env, e: &Expr, hi: Option<i128>, type_lo: i128, type_hi: i128) {
    let (Expr::Var(local), Some(hi)) = (e, hi) else {
        return;
    };
    let value = env
        .get(*local)
        .meet(Interval::range(type_lo, hi.min(type_hi)));
    env.set(*local, value);
}

/// Restricts `e` (a variable) to a smallest value of `lo`.
fn narrow_lo(env: &mut Env, e: &Expr, lo: Option<i128>, type_lo: i128, type_hi: i128) {
    let (Expr::Var(local), Some(lo)) = (e, lo) else {
        return;
    };
    let value = env
        .get(*local)
        .meet(Interval::range(lo.max(type_lo), type_hi));
    env.set(*local, value);
}

/// Restricts `e` (a variable) to the intersection with `with`.
fn narrow_meet(env: &mut Env, e: &Expr, with: Interval, type_lo: i128, type_hi: i128) {
    let Expr::Var(local) = e else {
        return;
    };
    let value = env
        .get(*local)
        .meet(with)
        .meet(Interval::range(type_lo, type_hi));
    env.set(*local, value);
}

/// The comparison that holds when `op` does not.
fn negate(op: BinOp) -> BinOp {
    match op {
        BinOp::Lt => BinOp::Ge,
        BinOp::Le => BinOp::Gt,
        BinOp::Gt => BinOp::Le,
        BinOp::Ge => BinOp::Lt,
        BinOp::Eq => BinOp::Ne,
        BinOp::Ne => BinOp::Eq,
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem | BinOp::And | BinOp::Or => {
            op
        }
    }
}

/// The interval an induction entry covers over the loop body, or `None` when it proves no bound.
///
/// The body runs with the counter at `init` and after every back edge; `iterations` counts the
/// back edges the guard allows, so the last value is `init + iterations * step`.
fn entry_interval(induction: &Induction) -> Option<Interval> {
    let iterations = induction.iterations?;
    let start = induction.init.as_point()?;
    let delta = i128::from(iterations).checked_mul(induction.step)?;
    let end = start.checked_add(delta)?;
    Some(Interval::range(start.min(end), start.max(end)))
}

/// Whether `interval` is a bounded range inside `0 .. len`.
fn in_bounds(interval: Interval, len: usize) -> bool {
    match interval {
        Interval::Range { lo, hi } => lo >= 0 && hi < len as i128,
        Interval::Bottom | Interval::Top => false,
    }
}

/// The `(index, array)` of a checked access, when `cond` and `then` are the shape the Option
/// fusion builds.
///
/// The condition must be exactly `(i >= 0) && (i < Array.size(a))`, and the then branch must start
/// with `let x = ArrayLoadUnchecked(a, i)` for the same `a` and `i`. `a` is a variable: the length
/// lookup is per local.
fn checked_test(cond: &Expr, then: &Expr) -> Option<(Expr, Local)> {
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
    if upper != lower {
        return None;
    }
    // The index of the fused test is a variable the fusion bound (a copy ζ may have replaced by
    // another variable or a scalar literal). Requiring that shape keeps the dropped condition
    // free of effects and defects: only the comparison, the conjunction and the length read go.
    if !matches!(&**upper, Expr::Var(_) | Expr::Lit(_)) {
        return None;
    }
    let Expr::Builtin {
        op: Builtin::ArraySize,
        args: size_args,
        ..
    } = &**size
    else {
        return None;
    };
    let [Expr::Var(array)] = &size_args[..] else {
        return None;
    };
    let Expr::Let { value, .. } = then else {
        return None;
    };
    let Expr::Builtin {
        op: Builtin::ArrayLoadUnchecked,
        args: load_args,
        ..
    } = &**value
    else {
        return None;
    };
    let [Expr::Var(load_array), load_index] = &load_args[..] else {
        return None;
    };
    if load_array != array || *load_index != **upper {
        return None;
    }
    Some(((**upper).clone(), *array))
}

/// The `let` bindings of a function body, by the local they bind. A local is bound at most once in
/// a module, so the map has one value per key.
fn bindings_of(e: &Expr) -> FxHashMap<Local, &Expr> {
    let mut bindings = FxHashMap::default();
    collect_bindings(e, &mut bindings);
    bindings
}

/// Collects the `let` bindings in `e` into `bindings`.
fn collect_bindings<'a>(e: &'a Expr, bindings: &mut FxHashMap<Local, &'a Expr>) {
    if let Expr::Let { var, value, .. } = e {
        bindings.insert(*var, value);
    }
    e.for_each_child(&mut |child| collect_bindings(child, bindings));
}

/// The length of the array `var` is bound to, through copies, or `None`.
fn local_length(
    m: &Module,
    bindings: &FxHashMap<Local, &Expr>,
    lengths: &mut FxHashMap<Local, usize>,
    constants: &FxHashMap<Local, i128>,
    visiting: &mut FxHashSet<Local>,
    var: Local,
) -> Option<usize> {
    if let Some(&len) = lengths.get(&var) {
        return Some(len);
    }
    if !visiting.insert(var) {
        // A cycle of copies: no binding produces an array.
        return None;
    }
    let value = bindings.get(&var).copied();
    let length =
        value.and_then(|value| expr_length(m, bindings, lengths, constants, visiting, value));
    visiting.remove(&var);
    if let Some(len) = length {
        lengths.insert(var, len);
    }
    length
}

/// The constant value of the array's `lo` or `hi`, through copies, or `None`.
fn local_constant(
    m: &Module,
    bindings: &FxHashMap<Local, &Expr>,
    constants: &mut FxHashMap<Local, i128>,
    visiting: &mut FxHashSet<Local>,
    var: Local,
) -> Option<i128> {
    if let Some(&value) = constants.get(&var) {
        return Some(value);
    }
    if !visiting.insert(var) {
        return None;
    }
    let value = bindings.get(&var).copied();
    let constant = value.and_then(|value| match value {
        Expr::Var(inner) => local_constant(m, bindings, constants, visiting, *inner),
        other => ranges::of_expr(m, other, &Env::new()).as_point(),
    });
    visiting.remove(&var);
    if let Some(value) = constant {
        constants.insert(var, value);
    }
    constant
}

/// The length of the array an expression makes, when its shape gives it.
///
/// Only the two shapes whose element count is written in the expression: `Array.from_list` of a
/// list literal without a tail, and `ArrayMapRange(lo, hi, f)` with constant bounds (its length is
/// `max(0, hi - lo)`).
fn expr_length(
    m: &Module,
    bindings: &FxHashMap<Local, &Expr>,
    lengths: &mut FxHashMap<Local, usize>,
    constants: &FxHashMap<Local, i128>,
    visiting: &mut FxHashSet<Local>,
    e: &Expr,
) -> Option<usize> {
    match e {
        Expr::Var(var) => local_length(m, bindings, lengths, constants, visiting, *var),
        Expr::Builtin {
            op: Builtin::ArrayFromList,
            args,
            ..
        } => match &args[..] {
            [
                Expr::List {
                    elems, tail: None, ..
                },
            ] => Some(elems.len()),
            _ => None,
        },
        Expr::Builtin {
            op: Builtin::ArrayMapRange,
            args,
            ..
        } => {
            let lo = constant_of(m, constants, args.first()?)?;
            let hi = constant_of(m, constants, args.get(1)?)?;
            let len = hi.checked_sub(lo)?;
            if len <= 0 {
                Some(0)
            } else {
                usize::try_from(len).ok()
            }
        }
        _ => None,
    }
}

/// The constant value of `e`, when its `let` bindings resolve it, or B5's point interval.
fn constant_of(m: &Module, constants: &FxHashMap<Local, i128>, e: &Expr) -> Option<i128> {
    match e {
        Expr::Var(var) => constants.get(var).copied(),
        other => ranges::of_expr(m, other, &Env::new()).as_point(),
    }
}
