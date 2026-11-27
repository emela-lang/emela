//! Tests for the range analysis (`emela_core::opt::analysis::ranges`).
//!
//! The interesting half is the deopt: an operation whose exact result may wrap, a divisor that may
//! be 0, a loop counter with no provable bound and a non-constant step must all come back as
//! `Interval::Top` (or as no [`Induction`]), never as a narrower interval. The tests build the IR
//! by hand so each shape is visible; the lowered shape of a `for` loop (`let lo = …; let hi = …;
//! loop i = lo { if i < hi then recur(i + 1) else … }`) is the one the loop tests use.

use emela_core::opt::analysis::Fact;
use emela_core::opt::analysis::ranges::{
    Env, Induction, Interval, analyze, inductions, of_expr, of_function,
};
use emela_core::{BinOp, Expr, FnId, Lit, Local, Module, OpTy, TypeId, UnOp};

/// An `Int` local.
fn int(m: &mut Module, name: &str) -> Local {
    m.local(name, TypeId::INT)
}

/// An `Int64` local.
fn int64_local(m: &mut Module, name: &str) -> Local {
    m.local(name, TypeId::INT64)
}

/// A `Bool` local.
fn boolean(m: &mut Module, name: &str) -> Local {
    m.local(name, TypeId::BOOL)
}

/// An `Int` literal.
fn lit(value: i32) -> Expr {
    Expr::Lit(Lit::Int(value))
}

/// An `Int64` literal.
fn lit64(value: i64) -> Expr {
    Expr::Lit(Lit::Int64(value))
}

/// A literal of any scalar.
fn expr_lit(lit: Lit) -> Expr {
    Expr::Lit(lit)
}

/// A read of `local`.
fn var(local: Local) -> Expr {
    Expr::Var(local)
}

/// A binary operation with an explicit operand type.
fn binary(op: BinOp, ty: OpTy, lhs: Expr, rhs: Expr) -> Expr {
    Expr::Binary {
        op,
        ty,
        lhs: Box::new(lhs),
        rhs: Box::new(rhs),
    }
}

/// `a + b` on `Int`.
fn add(lhs: Expr, rhs: Expr) -> Expr {
    binary(BinOp::Add, OpTy::Int, lhs, rhs)
}

/// `a - b` on `Int`.
fn sub(lhs: Expr, rhs: Expr) -> Expr {
    binary(BinOp::Sub, OpTy::Int, lhs, rhs)
}

/// `a * b` on `Int`.
fn mul(lhs: Expr, rhs: Expr) -> Expr {
    binary(BinOp::Mul, OpTy::Int, lhs, rhs)
}

/// `a / b` on `Int`.
fn div(lhs: Expr, rhs: Expr) -> Expr {
    binary(BinOp::Div, OpTy::Int, lhs, rhs)
}

/// `a % b` on `Int`.
fn rem(lhs: Expr, rhs: Expr) -> Expr {
    binary(BinOp::Rem, OpTy::Int, lhs, rhs)
}

/// `a < b` on `Int`.
fn less(lhs: Expr, rhs: Expr) -> Expr {
    binary(BinOp::Lt, OpTy::Int, lhs, rhs)
}

/// `-a` on `Int`.
fn neg(operand: Expr) -> Expr {
    Expr::Unary {
        op: UnOp::Neg,
        ty: OpTy::Int,
        operand: Box::new(operand),
    }
}

/// A function without parameters and with `body`.
fn function(m: &mut Module, name: &str, body: Expr) -> FnId {
    let f = m.declare(name, false);
    m.define(f, Vec::new(), TypeId::UNIT, body);
    f
}

/// The range of `e` with no bound locals.
fn range(m: &Module, e: &Expr) -> Interval {
    of_expr(m, e, &Env::new())
}

/// An environment with one binding.
fn one(local: Local, interval: Interval) -> Env {
    let mut env = Env::new();
    env.set(local, interval);
    env
}

/// The ranges of `f`, which the analysis always knows.
fn ranges_of(m: &Module, f: FnId) -> emela_core::opt::analysis::ranges::FnRanges {
    match of_function(m, f) {
        Fact::Known(ranges) => ranges,
        Fact::Unknown => panic!("the range analysis is total"),
    }
}

/// The inductions of `f`, which the analysis always knows.
fn inductions_of(m: &Module, f: FnId) -> Vec<Induction> {
    match inductions(m, f) {
        Fact::Known(inductions) => inductions,
        Fact::Unknown => panic!("the range analysis is total"),
    }
}

/// A literal is a point; Bool is 0 or 1; a value the analysis does not model is Top.
#[test]
fn literals_are_points() {
    let m = Module::new();
    assert_eq!(range(&m, &lit(5)), Interval::point(5));
    assert_eq!(range(&m, &lit(-7)), Interval::point(-7));
    assert_eq!(
        range(&m, &lit64(5_000_000_000)),
        Interval::point(5_000_000_000)
    );
    assert_eq!(range(&m, &expr_lit(Lit::Bool(true))), Interval::point(1));
    assert_eq!(range(&m, &expr_lit(Lit::Bool(false))), Interval::point(0));
    assert_eq!(range(&m, &expr_lit(Lit::Float(1.5))), Interval::Top);
    assert_eq!(range(&m, &expr_lit(Lit::Unit)), Interval::Top);
}

/// Add, sub and mul combine the bounds exactly when the result fits the width.
#[test]
fn arithmetic_without_overflow_is_exact() {
    let mut m = Module::new();
    let a = int(&mut m, "a");
    let b = int(&mut m, "b");
    let mut env = Env::new();
    env.set(a, Interval::range(1, 3));
    env.set(b, Interval::range(4, 5));
    assert_eq!(
        of_expr(&m, &add(var(a), var(b)), &env),
        Interval::range(5, 8)
    );
    assert_eq!(
        of_expr(&m, &sub(var(a), var(b)), &env),
        Interval::range(-4, -1)
    );

    let mut env = Env::new();
    env.set(a, Interval::range(-2, 3));
    env.set(b, Interval::range(-4, 5));
    assert_eq!(
        of_expr(&m, &mul(var(a), var(b)), &env),
        Interval::range(-12, 15)
    );

    assert_eq!(
        range(&m, &add(lit(i32::MAX - 1), lit(1))),
        Interval::point(i32::MAX as i128)
    );
    assert_eq!(range(&m, &mul(lit(-3), lit(4))), Interval::point(-12));
}

/// An exact result outside the width may wrap, so the answer is Top.
#[test]
fn overflow_widens_to_top() {
    let m = Module::new();
    assert_eq!(range(&m, &add(lit(i32::MAX), lit(1))), Interval::Top);
    assert_eq!(range(&m, &sub(lit(i32::MIN), lit(1))), Interval::Top);
    assert_eq!(range(&m, &mul(lit(i32::MAX), lit(2))), Interval::Top);

    assert_eq!(
        range(
            &m,
            &binary(BinOp::Add, OpTy::Int64, lit64(i64::MAX), lit64(1))
        ),
        Interval::Top
    );
    assert_eq!(
        range(
            &m,
            &binary(BinOp::Sub, OpTy::Int64, lit64(i64::MIN), lit64(1))
        ),
        Interval::Top
    );
    assert_eq!(
        range(
            &m,
            &binary(BinOp::Mul, OpTy::Int64, lit64(i64::MAX), lit64(2))
        ),
        Interval::Top
    );

    // A range whose upper end overflows is Top, not a clamped bound.
    let mut width = Module::new();
    let x = int(&mut width, "x");
    let env = one(x, Interval::range(i32::MAX as i128 - 2, i32::MAX as i128));
    assert_eq!(of_expr(&width, &add(var(x), lit(1)), &env), Interval::Top);
}

/// The minimum-value cases: wrapping and `MIN / -1` are not claimed as bounds.
#[test]
fn minimum_value_cases_widen_to_top() {
    let m = Module::new();
    assert_eq!(range(&m, &neg(lit(i32::MIN))), Interval::Top);
    assert_eq!(
        range(
            &m,
            &Expr::Unary {
                op: UnOp::Neg,
                ty: OpTy::Int64,
                operand: Box::new(lit64(i64::MIN)),
            }
        ),
        Interval::Top
    );
    assert_eq!(range(&m, &div(lit(i32::MIN), lit(-1))), Interval::Top);
    assert_eq!(
        range(&m, &div(lit(i32::MIN), lit(1))),
        Interval::point(i32::MIN as i128)
    );
    assert_eq!(
        range(&m, &mul(lit(i32::MIN), lit(-1))),
        Interval::Top,
        "the exact result is 2^31, outside Int"
    );
}

/// Negation of a range is the mirrored range, and it deopts when the mirror does not fit.
#[test]
fn negation_of_a_range() {
    let mut m = Module::new();
    let x = int(&mut m, "x");
    let env = one(x, Interval::range(-5, 3));
    assert_eq!(of_expr(&m, &neg(var(x)), &env), Interval::range(-3, 5));

    let env = one(x, Interval::range(i32::MIN as i128, 5));
    assert_eq!(of_expr(&m, &neg(var(x)), &env), Interval::Top);
}

/// Division is exact only when the divisor excludes 0; a range containing 0 is Top.
#[test]
fn division_and_remainder_are_conservative() {
    let mut m = Module::new();
    let x = int(&mut m, "x");
    let d = int(&mut m, "d");

    let mut env = Env::new();
    env.set(x, Interval::range(10, 20));
    env.set(d, Interval::range(2, 4));
    assert_eq!(
        of_expr(&m, &div(var(x), var(d)), &env),
        Interval::range(2, 10)
    );

    let env = one(d, Interval::range(-1, 1));
    assert_eq!(
        of_expr(&m, &div(lit(10), var(d)), &env),
        Interval::Top,
        "a divisor range containing 0 may divide by zero"
    );
    assert_eq!(of_expr(&m, &rem(lit(10), var(d)), &env), Interval::Top);

    // The remainder has the sign of the dividend and a magnitude below the largest divisor.
    let env = one(d, Interval::range(3, 5));
    let remainder = of_expr(&m, &rem(lit(-8), var(d)), &env);
    assert!(
        remainder.contains(-2),
        "the corner `-8 % 3` is in the bound"
    );
    assert_eq!(remainder, Interval::range(-4, 0));

    assert_eq!(range(&m, &div(lit(i32::MIN), lit(0))), Interval::Top);
}

/// Comparisons give the Bool interval, whatever the operands.
#[test]
fn comparisons_are_bool() {
    let m = Module::new();
    assert_eq!(range(&m, &less(lit(1), lit(10))), Interval::range(0, 1));
    assert_eq!(
        range(
            &m,
            &binary(BinOp::Eq, OpTy::Int64, lit64(i64::MIN), lit64(i64::MAX))
        ),
        Interval::range(0, 1)
    );
}

/// `let x = literal` and `let y = x` propagate; the ranges of the locals are recorded.
#[test]
fn let_propagation() {
    let mut m = Module::new();
    let x = int(&mut m, "x");
    let y = int(&mut m, "y");
    let body = Expr::Let {
        var: x,
        value: Box::new(lit(5)),
        body: Box::new(Expr::Let {
            var: y,
            value: Box::new(var(x)),
            body: Box::new(mul(var(y), lit(2))),
        }),
    };
    let f = function(&mut m, "f", body);
    assert_eq!(range(&m, &m.functions[f].body), Interval::point(10));

    let ranges = ranges_of(&m, f);
    assert_eq!(ranges.range(x), Interval::point(5));
    assert_eq!(ranges.range(y), Interval::point(5));
}

/// The join at an `if` is the hull of the branches, and it flows into the bound local.
#[test]
fn the_join_at_an_if_is_the_hull() {
    let mut m = Module::new();
    let c = boolean(&mut m, "c");
    let x = int(&mut m, "x");
    let arm = Expr::If {
        cond: Box::new(var(c)),
        then: Box::new(lit(1)),
        else_: Box::new(lit(10)),
    };
    let body = Expr::Let {
        var: x,
        value: Box::new(arm),
        body: Box::new(add(var(x), lit(1))),
    };
    let f = function(&mut m, "f", body);
    assert_eq!(range(&m, &m.functions[f].body), Interval::range(2, 11));
    assert_eq!(ranges_of(&m, f).range(x), Interval::range(1, 10));
}

/// The join at a `match` is the hull of its arms, and a bind pattern takes the scrutinee's range.
#[test]
fn the_join_at_a_match_is_the_hull() {
    use emela_core::{Arm, Pat};

    let mut m = Module::new();
    let x = int(&mut m, "x");
    let y = int(&mut m, "y");
    let body = Expr::Let {
        var: x,
        value: Box::new(lit(5)),
        body: Box::new(Expr::Match {
            scrutinee: Box::new(var(x)),
            arms: vec![
                Arm {
                    pat: Pat::Bind(y),
                    guard: None,
                    body: var(y),
                },
                Arm {
                    pat: Pat::Wild,
                    guard: None,
                    body: lit(0),
                },
            ],
        }),
    };
    let f = function(&mut m, "f", body);
    assert_eq!(range(&m, &m.functions[f].body), Interval::range(0, 5));
    assert_eq!(ranges_of(&m, f).range(y), Interval::point(5));
}

/// A `for i in 0..10`-shaped loop reaches the fixed point [0, 10] and reports step 1 and 10
/// iterations.
#[test]
fn a_bounded_loop_reaches_a_fixpoint() {
    let mut m = Module::new();
    let i = int(&mut m, "i");
    let hi = int(&mut m, "hi");
    let loop_body = Expr::If {
        cond: Box::new(less(var(i), var(hi))),
        then: Box::new(Expr::Recur(vec![add(var(i), lit(1))])),
        else_: Box::new(var(i)),
    };
    let body = Expr::Let {
        var: hi,
        value: Box::new(lit(10)),
        body: Box::new(Expr::Loop {
            vars: vec![i],
            inits: vec![lit(0)],
            body: Box::new(loop_body),
        }),
    };
    let f = function(&mut m, "count", body);

    let ranges = ranges_of(&m, f);
    assert_eq!(ranges.range(i), Interval::range(0, 10));
    assert_eq!(ranges.range(hi), Interval::point(10));
    assert_eq!(
        ranges.inductions,
        vec![Induction {
            var: i,
            init: Interval::point(0),
            step: 1,
            iterations: Some(10),
        }]
    );

    // The value of the loop is the counter at the guard, 10.
    assert_eq!(range(&m, &m.functions[f].body), Interval::point(10));
}

/// A loop whose guard does not bound the counter deopts to Top within the cap instead of running
/// for an unbounded time.
#[test]
fn an_unbounded_loop_widens_to_top() {
    let mut m = Module::new();
    let i = int(&mut m, "i");
    let body = Expr::Loop {
        vars: vec![i],
        inits: vec![lit(0)],
        body: Box::new(Expr::Recur(vec![add(var(i), lit(1))])),
    };
    let f = function(&mut m, "spin", body);

    let ranges = ranges_of(&m, f);
    assert_eq!(ranges.range(i), Interval::Top);
    // The shape is still an induction with a constant step, but nothing proves the trip count.
    assert_eq!(
        ranges.inductions,
        vec![Induction {
            var: i,
            init: Interval::point(0),
            step: 1,
            iterations: None,
        }]
    );
}

/// A negative step descends and its trip count is proved.
#[test]
fn a_negative_step_counts_down() {
    let mut m = Module::new();
    let i = int(&mut m, "i");
    let loop_body = Expr::If {
        cond: Box::new(binary(BinOp::Gt, OpTy::Int, var(i), lit(0))),
        then: Box::new(Expr::Recur(vec![add(var(i), lit(-1))])),
        else_: Box::new(var(i)),
    };
    let body = Expr::Loop {
        vars: vec![i],
        inits: vec![lit(10)],
        body: Box::new(loop_body),
    };
    let f = function(&mut m, "countdown", body);

    assert_eq!(
        inductions_of(&m, f),
        vec![Induction {
            var: i,
            init: Interval::point(10),
            step: -1,
            iterations: Some(10),
        }]
    );
    assert_eq!(ranges_of(&m, f).range(i), Interval::range(0, 10));
}

/// A loop that can also leave from inside the continuing branch has no provable trip count: the
/// guard does not decide how many back edges are taken.
#[test]
fn a_loop_that_can_leave_early_has_no_iteration_count() {
    let mut m = Module::new();
    let i = int(&mut m, "i");
    let hi = int(&mut m, "hi");
    let c = boolean(&mut m, "c");
    let inner = Expr::If {
        cond: Box::new(var(c)),
        then: Box::new(Expr::Recur(vec![add(var(i), lit(1))])),
        else_: Box::new(lit(0)),
    };
    let loop_body = Expr::If {
        cond: Box::new(less(var(i), var(hi))),
        then: Box::new(inner),
        else_: Box::new(lit(0)),
    };
    let body = Expr::Let {
        var: hi,
        value: Box::new(lit(10)),
        body: Box::new(Expr::Loop {
            vars: vec![i],
            inits: vec![lit(0)],
            body: Box::new(loop_body),
        }),
    };
    let f = function(&mut m, "early", body);
    let found = inductions_of(&m, f);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].step, 1);
    assert_eq!(found[0].iterations, None);
}

/// A step that is not a constant is not an induction variable.
#[test]
fn a_non_constant_step_is_not_an_induction() {
    let mut m = Module::new();
    let i = int(&mut m, "i");
    let n = int(&mut m, "n");
    let body = Expr::Let {
        var: n,
        value: Box::new(var(n)),
        body: Box::new(Expr::Loop {
            vars: vec![i],
            inits: vec![lit(0)],
            body: Box::new(Expr::Recur(vec![add(var(i), var(n))])),
        }),
    };
    let f = function(&mut m, "step", body);
    assert_eq!(inductions_of(&m, f), Vec::new());
}

/// A bound that is not a known point leaves the trip count unknown, not wrong.
#[test]
fn a_variable_bound_leaves_the_iteration_count_unknown() {
    let mut m = Module::new();
    let i = int(&mut m, "i");
    let n = int(&mut m, "n");
    let loop_body = Expr::If {
        cond: Box::new(less(var(i), var(n))),
        then: Box::new(Expr::Recur(vec![add(var(i), lit(1))])),
        else_: Box::new(var(i)),
    };
    let body = Expr::Let {
        var: n,
        value: Box::new(var(n)),
        body: Box::new(Expr::Loop {
            vars: vec![i],
            inits: vec![lit(0)],
            body: Box::new(loop_body),
        }),
    };
    let f = function(&mut m, "until", body);

    let found = inductions_of(&m, f);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].var, i);
    assert_eq!(found[0].step, 1);
    assert_eq!(found[0].iterations, None);
    assert_eq!(
        ranges_of(&m, f).range(i),
        Interval::Top,
        "the loop does not reach a bounded fixed point"
    );
}

/// An `Int64` counter is analysed with 64-bit bounds.
#[test]
fn an_int64_counter_uses_64_bit_bounds() {
    let mut m = Module::new();
    let i = int64_local(&mut m, "i");
    let hi = int64_local(&mut m, "hi");
    let loop_body = Expr::If {
        cond: Box::new(binary(BinOp::Lt, OpTy::Int64, var(i), var(hi))),
        then: Box::new(Expr::Recur(vec![binary(
            BinOp::Add,
            OpTy::Int64,
            var(i),
            lit64(1),
        )])),
        else_: Box::new(var(i)),
    };
    let body = Expr::Let {
        var: hi,
        value: Box::new(lit64(3)),
        body: Box::new(Expr::Loop {
            vars: vec![i],
            inits: vec![lit64(0)],
            body: Box::new(loop_body),
        }),
    };
    let f = function(&mut m, "count64", body);

    let ranges = ranges_of(&m, f);
    assert_eq!(ranges.range(i), Interval::range(0, 3));
    assert_eq!(
        ranges.inductions,
        vec![Induction {
            var: i,
            init: Interval::point(0),
            step: 1,
            iterations: Some(3),
        }]
    );
}

/// The analysis API answers `Known`, carries the ranges of every function and is deterministic.
#[test]
fn the_analysis_api_is_known_and_deterministic() {
    let mut m = Module::new();
    let x = int(&mut m, "x");
    let f = function(
        &mut m,
        "f",
        Expr::Let {
            var: x,
            value: Box::new(lit(7)),
            body: Box::new(var(x)),
        },
    );

    let first = analyze(&m);
    let second = analyze(&m);
    assert_eq!(first, second);
    let Fact::Known(all) = first else {
        panic!("the range analysis is total");
    };
    assert_eq!(all[&f].range(x), Interval::point(7));

    assert_eq!(
        of_function(&m, f),
        of_function(&m, f),
        "two runs give the same result"
    );
}

/// The join is a hull and is monotone; the order is the interval order.
#[test]
fn join_is_a_hull_and_monotone() {
    let a = Interval::range(1, 3);
    let b = Interval::range(2, 8);
    assert_eq!(a.join(b), Interval::range(1, 8));
    assert!(a <= a.join(b));
    assert!(b <= a.join(b));
    assert_eq!(a.join(Interval::Bottom), a);

    // A monotone map on the lattice: joining a larger interval cannot give a smaller result.
    let wider = Interval::range(0, 10);
    assert!(a <= wider);
    assert!(a.join(b) <= wider.join(b));
    assert_eq!(
        Interval::range(0, 2).partial_cmp(&Interval::range(1, 3)),
        None,
        "overlapping but not nested intervals are incomparable"
    );
}

/// `of_expr` is pure with respect to the environment it is given.
#[test]
fn of_expr_does_not_change_the_environment() {
    let mut m = Module::new();
    let x = int(&mut m, "x");
    let env = one(x, Interval::point(3));
    let before = env.clone();
    assert_eq!(of_expr(&m, &add(var(x), lit(1)), &env), Interval::point(4));
    assert_eq!(env, before);
}

/// A `fail` produces no value, so a join with it keeps the other branch.
#[test]
fn a_failing_branch_does_not_widen_the_join() {
    let mut m = Module::new();
    let c = boolean(&mut m, "c");
    let arm = Expr::If {
        cond: Box::new(var(c)),
        then: Box::new(Expr::Fail(Box::new(lit(0)))),
        else_: Box::new(lit(10)),
    };
    assert_eq!(range(&m, &arm), Interval::point(10));
}

/// A local no binder reaches is Top, and an empty module is a known empty map.
#[test]
fn unbound_locals_and_empty_modules_are_known() {
    let mut m = Module::new();
    let x = int(&mut m, "x");
    assert_eq!(range(&m, &var(x)), Interval::Top);

    let empty = Module::new();
    assert!(analyze(&empty).is_known());
    assert_eq!(analyze(&empty), Fact::Known(Default::default()));
}
