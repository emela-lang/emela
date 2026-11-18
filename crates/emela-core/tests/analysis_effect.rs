//! Tests of the B1/PR-020 effect and defect summary.
//!
//! The fixtures are lowered with the Core IR optimisations off (`support::lower_files`), so the
//! shape the analysis sees is the one lowering produces. Each test names the fact it pins; the
//! edge cases that a well-typed source cannot produce (a match without a catch-all) are hand-built
//! IR.

mod support;

use emela_core::opt::analysis::effect::{self, EffectSummary};
use emela_core::{
    Arm, BinOp, CtorRef, EnumDef, Expr, Lit, Module, OpTy, Pat, TypeId, VariantDef, dump,
};
use support::lower_files;

/// Lowers one entry module.
fn module(source: &str) -> Module {
    lower_files(&[("main.emel", source)])
}

/// The function of `m` named `name`.
fn function(m: &Module, name: &str) -> emela_core::FnId {
    m.functions
        .iter()
        .find(|(_, f)| f.name == name)
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no function `{name}` in\n{}", dump(m)))
}

/// The summary of the function of `m` named `name`.
fn summary(m: &Module, name: &str) -> EffectSummary {
    effect::of_function(m, function(m, name))
}

/// A summary with only the named fields set.
fn only(
    may_use_effect: bool,
    may_fail: bool,
    may_suspend: bool,
    may_defect: bool,
    may_diverge: bool,
) -> EffectSummary {
    EffectSummary {
        may_use_effect,
        may_fail,
        may_suspend,
        may_defect,
        may_diverge,
    }
}

// ---------------------------------------------------------------------------------------------
// The lattice
// ---------------------------------------------------------------------------------------------

#[test]
fn the_summary_lattice_has_a_bottom_and_a_top() {
    let pure = EffectSummary::pure();
    let unknown = EffectSummary::unknown();
    assert!(pure.is_pure());
    assert!(!unknown.is_pure());
    assert_eq!(
        EffectSummary::default(),
        unknown,
        "default is the safe value"
    );
    assert_eq!(pure.join(pure), pure, "the join is idempotent");
    assert_eq!(pure.join(unknown), unknown, "unknown absorbs");
    assert_eq!(unknown.join(pure), unknown, "the join is commutative");
}

#[test]
fn the_join_is_a_union_and_monotone() {
    let effect = only(true, false, false, false, false);
    let defect = only(false, false, false, true, false);
    assert_eq!(effect.join(defect), only(true, false, false, true, false));
    // Adding a fact never clears one.
    assert_eq!(effect.join(defect).join(effect), effect.join(defect));
    // The join is the least upper bound: each side is below it.
    for a in [
        EffectSummary::pure(),
        effect,
        defect,
        EffectSummary::unknown(),
    ] {
        for b in [
            EffectSummary::pure(),
            effect,
            defect,
            EffectSummary::unknown(),
        ] {
            let joined = a.join(b);
            assert_eq!(joined.join(a), joined);
            assert_eq!(joined.join(b), joined);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Purity, effects, failure and suspension
// ---------------------------------------------------------------------------------------------

/// A function whose body only computes an Int is pure.
#[test]
fn a_pure_arithmetic_function_has_no_flags() {
    let m = module(
        "\
fn compute(a: Int, b: Int) -> Int {
  a * 2 + b
}

fn main() {
  dbg(compute(1, 2))
}
",
    );
    assert_eq!(summary(&m, "compute"), EffectSummary::pure());
    // The same expression, asked for on its own, is pure too.
    let body = &m.functions[function(&m, "compute")].body;
    assert!(effect::of_expr(&m, body).is_pure());
}

/// `dbg` writes to stderr. The builtin table calls it pure as a debugging exception; the summary
/// treats the write as the observable effect it is.
#[test]
fn dbg_is_an_effect_and_nothing_else() {
    let m = module(
        "\
fn shout(x: Int) -> Int {
  dbg(x)
  x + 1
}

fn main() {
  dbg(shout(1))
}
",
    );
    assert_eq!(
        summary(&m, "shout"),
        only(true, false, false, false, false),
        "a stderr write is an effect; it cannot fail, suspend or defect"
    );
}

/// `fail` and a call marked `fails` are failures; the caller's summary joins the callee's.
#[test]
fn a_failing_call_and_a_fail_are_failures() {
    let m = module(
        "\
error NotFound(id: Int)

fn find_user(id: Int) -> Int fails NotFound {
  if id > 0 { id } else { fail NotFound(id:) }
}

fn caller(id: Int) -> Int fails NotFound {
  find_user(id)
}

fn main() {
  dbg(caller(1) escape {
    _ -> 0
  })
}
",
    );
    assert_eq!(
        summary(&m, "find_user"),
        only(false, true, false, false, false),
        "the failing branch is a failure, not a defect"
    );
    assert_eq!(
        summary(&m, "caller"),
        only(false, true, false, false, false),
        "a callee's failure joins into the caller"
    );
    assert_eq!(
        summary(&m, "main"),
        only(true, false, false, false, false),
        "the catch-all escape catches the failure; the dbg is an effect"
    );
}

/// A suspending operation is a suspension. The handler comes from the evidence table, so its body
/// is not visible: it may also defect or diverge.
#[test]
fn a_suspending_call_suspends() {
    let m = module(
        "\
effect Clock {
  suspend fn sleep(ms: Int) -> Int
}

handler InstantClock: Clock {
  suspend fn sleep(ms, resume) { resume(ms) }
}

fn nap() -> Int uses { Clock } {
  (use Clock).sleep(1)
}

fn main() {
  with InstantClock { dbg(nap()) }
}
",
    );
    let nap = summary(&m, "nap");
    assert!(nap.may_suspend, "the call is marked suspends: {nap:?}");
    assert!(nap.may_use_effect, "the operation goes through evidence");
    assert!(
        !nap.may_fail,
        "the failing flag of the call is false: {nap:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// Defects
// ---------------------------------------------------------------------------------------------

/// Integer division and remainder may defect. The divisor is not inspected: `a / 2` is as
/// conservative as `a / b`, because the analysis is not a constant folder. Float division follows
/// IEEE (no defect).
#[test]
fn integer_division_may_defect_even_by_a_literal() {
    let m = module(
        "\
fn divide(a: Int, b: Int) -> Int {
  a / b
}

fn halve(a: Int) -> Int {
  a / 2
}

fn remainder(a: Int) -> Int {
  a % 3
}

fn divide_float(a: Float, b: Float) -> Float {
  a / b
}

fn main() {
  dbg(divide(6, 2) + halve(6) + remainder(7))
  dbg(divide_float(1.0, 2.0))
}
",
    );
    for name in ["divide", "halve", "remainder"] {
        assert_eq!(
            summary(&m, name),
            only(false, false, false, true, false),
            "{name} may divide by zero"
        );
    }
    assert_eq!(
        summary(&m, "divide_float"),
        EffectSummary::pure(),
        "Float division by zero is IEEE, not a defect"
    );
}

/// The operand type decides: Int64 is checked like Int, Float is not.
#[test]
fn an_int64_division_may_defect_and_a_float_division_does_not() {
    let div = |ty: OpTy| Expr::Binary {
        op: BinOp::Div,
        ty,
        lhs: Box::new(Expr::Lit(Lit::Int64(1))),
        rhs: Box::new(Expr::Lit(Lit::Int64(2))),
    };
    let m = Module::new();
    assert!(effect::of_expr(&m, &div(OpTy::Int64)).may_defect);
    assert!(!effect::of_expr(&m, &div(OpTy::Float)).may_defect);
}

/// An assert is a defect when its condition is false. The analysis does not fold the condition.
#[test]
fn assert_may_defect() {
    let m = module(
        "\
fn check(n: Int) {
  assert n > 0
}

fn main() {
  check(1)
}
",
    );
    assert_eq!(summary(&m, "check"), only(false, false, false, true, false));
}

/// A match without a catch-all arm is a defect when no arm matches. The type checker rejects a
/// non-exhaustive source match, so this is hand-built IR; the wildcard arm is the control.
#[test]
fn a_match_without_a_catch_all_may_defect() {
    let mut m = Module::new();
    let shape = m.add_enum(EnumDef {
        name: "Shape".to_owned(),
        params: 0,
        variants: vec![VariantDef::unit("Circle"), VariantDef::unit("Empty")],
        error: false,
        external: false,
    });
    let ty = m.enum_type(shape, vec![]);
    let scrutinee = m.local("s", ty);
    let arm = |pat| Arm {
        pat,
        guard: None,
        body: Expr::Lit(Lit::Unit),
    };
    let circle = Pat::Ctor {
        ctor: CtorRef {
            enum_id: shape,
            variant: 0,
        },
        fields: vec![],
    };
    let partial = Expr::Match {
        scrutinee: Box::new(Expr::Var(scrutinee)),
        arms: vec![arm(circle.clone())],
    };
    let total = Expr::Match {
        scrutinee: Box::new(Expr::Var(scrutinee)),
        arms: vec![arm(circle), arm(Pat::Wild)],
    };
    assert!(
        effect::of_expr(&m, &partial).may_defect,
        "an unmatched value is a defect"
    );
    assert!(
        !effect::of_expr(&m, &total).may_defect,
        "a wildcard arm matches every value"
    );
}

// ---------------------------------------------------------------------------------------------
// Divergence
// ---------------------------------------------------------------------------------------------

/// A recursive function may not terminate, and its callers inherit that. Loopification does not
/// see `fib`, whose recursion is not in tail position, so the call graph keeps the cycle.
#[test]
fn a_recursive_function_diverges() {
    let m = module(
        "\
fn fib(n: Int) -> Int {
  if n < 2 { n } else { fib(n - 1) + fib(n - 2) }
}

fn main() {
  dbg(fib(5))
}
",
    );
    assert_eq!(
        summary(&m, "fib"),
        only(false, false, false, false, true),
        "recursion is divergence, not an effect"
    );
    assert!(
        summary(&m, "main").may_diverge,
        "a caller of a diverging function may diverge"
    );
}

/// Mutual recursion puts both functions on a cycle of the call graph.
#[test]
fn mutual_recursion_diverges_in_both_functions() {
    let m = module(
        "\
fn even(n: Int) -> Bool {
  if n == 0 { True } else { odd(n - 1) }
}

fn odd(n: Int) -> Bool {
  if n == 0 { False } else { even(n - 1) }
}

fn main() {
  dbg(even(4))
}
",
    );
    for name in ["even", "odd"] {
        assert_eq!(
            summary(&m, name),
            only(false, false, false, false, true),
            "{name} is on a cycle"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Calls
// ---------------------------------------------------------------------------------------------

/// A function value is not visible through the variable it is passed in. The function type proves
/// the failure, suspension and evidence facts; a defect is not in the type, and the body may not
/// terminate.
#[test]
fn an_indirect_call_is_conservative() {
    let m = module(
        "\
fn apply(f: fn(Int) -> Int fails {} uses {}, x: Int) -> Int {
  f(x)
}

fn main() {
  dbg(apply(|x| x + 1, 1))
}
",
    );
    assert_eq!(
        summary(&m, "apply"),
        only(false, false, false, true, true),
        "the type says pure, but a defect and divergence are not in it"
    );
}

/// An indirect call whose callee has no function type proves nothing.
#[test]
fn an_indirect_call_without_a_function_type_is_unknown() {
    let mut m = Module::new();
    let callee = m.local("f", TypeId::ANY);
    let call = Expr::Call {
        callee: emela_core::Callee::Indirect(Box::new(Expr::Var(callee))),
        args: vec![],
        fails: false,
        suspends: false,
    };
    assert_eq!(effect::of_expr(&m, &call), EffectSummary::unknown());
}

/// A host function may print, defect and not return; its E is empty (spec 12.5), so a call to it
/// does not fail. Built by hand: an `@external` in source has to sit in a handler operation.
#[test]
fn an_external_call_is_conservative_but_cannot_fail() {
    let mut m = Module::new();
    let host = m.declare("host", false);
    m.define_external(
        host,
        vec![],
        TypeId::UNIT,
        emela_core::External { bindings: vec![] },
    );
    let caller = m.declare("caller", false);
    m.define(
        caller,
        vec![],
        TypeId::UNIT,
        Expr::Call {
            callee: emela_core::Callee::Direct {
                f: host,
                ty_args: vec![],
            },
            args: vec![],
            fails: false,
            suspends: false,
        },
    );
    assert_eq!(
        effect::of_function(&m, caller),
        only(true, false, false, true, true)
    );
    assert_eq!(
        effect::of_function(&m, host),
        only(true, false, false, true, true),
        "the host function's own summary is the same"
    );
}

/// A lambda allocation is pure; the body belongs to the function that may pass or call it.
#[test]
fn a_lambda_allocation_is_pure_but_its_body_belongs_to_the_function() {
    let m = module(
        "\
fn make() -> fn() -> Int fails {} uses {} {
  || dbg(1)
}

fn main() {
  g = make()
  dbg(g())
}
",
    );
    let body = &m.functions[function(&m, "make")].body;
    let lambda = first_lambda(body).expect("the body is a lambda");
    assert!(
        effect::of_expr(&m, lambda).is_pure(),
        "allocating a closure has no effect"
    );
    assert_eq!(
        summary(&m, "make"),
        only(true, false, false, false, false),
        "the lambda's dbg is part of make"
    );
}

/// The first lambda in `e`, in evaluation order (the test fixture has one at the top level).
fn first_lambda(e: &Expr) -> Option<&Expr> {
    if matches!(e, Expr::Lambda { .. }) {
        return Some(e);
    }
    let mut found = None;
    e.for_each_child(&mut |child| {
        if found.is_none() {
            found = first_lambda(child);
        }
    });
    found
}

/// A `with` of a handler uses an effect. A non-failing operation does not make the body fail.
#[test]
fn a_with_of_a_handler_uses_an_effect_without_failing() {
    let m = module(
        "\
effect Ask {
  fn ask() -> Int
}

handler Asked: Ask {
  fn ask() { 42 }
}

fn main() {
  with Asked {
    h = use Ask
    dbg(h.ask())
  }
}
",
    );
    let main = summary(&m, "main");
    assert!(main.may_use_effect, "installing a handler is an effect");
    assert!(!main.may_fail, "nothing fails: {main:?}");
}

/// A catch-all escape catches the body's failures; one without a catch-all does not.
#[test]
fn an_escape_with_a_catch_all_catches_failures() {
    let m = module(
        "\
error NotFound(id: Int)

fn find_user(id: Int) -> Int fails NotFound {
  if id > 0 { id } else { fail NotFound(id:) }
}

fn safe(id: Int) -> Int {
  find_user(id) escape {
    _ -> 0
  }
}

fn partial(id: Int) -> Int fails NotFound {
  find_user(id) escape {
    NotFound(id:) if id > 5 -> id
  }
}

fn main() {
  dbg(safe(1) + (partial(1) escape {
    _ -> 0
  }))
}
",
    );
    assert!(
        !summary(&m, "safe").may_fail,
        "the wildcard arm catches every failure"
    );
    assert!(
        summary(&m, "partial").may_fail,
        "an arm that may not match leaves a failure to propagate"
    );
}

// ---------------------------------------------------------------------------------------------
// The module map
// ---------------------------------------------------------------------------------------------

/// The map is built in arena order and gives the same answer twice; the analysis fact is known.
#[test]
fn the_module_summary_is_deterministic_and_known() {
    let m = module(
        "\
fn one() -> Int { 1 }
fn two() -> Int { one() }

fn main() {
  dbg(two())
}
",
    );
    let first = effect::of_module(&m);
    let second = effect::of_module(&m);
    assert_eq!(first, second);
    assert_eq!(first.len(), m.functions.len());
    for (id, _) in m.functions.iter() {
        assert!(first.contains_key(&id), "every function has a summary");
    }
    let fact = effect::analyze(&m);
    assert!(fact.is_known());
    assert_eq!(fact.known(), Some(first));
}

/// Adding an effect to a callee never removes one from the caller: the caller's summary is the
/// join of its body and its callees.
#[test]
fn a_callee_summary_joins_into_the_caller() {
    let calm = module(
        "\
fn g() -> Int {
  1
}

fn h() -> Int {
  g()
}

fn main() {
  dbg(h())
}
",
    );
    let loud = module(
        "\
fn g() -> Int {
  dbg(1)
  1
}

fn h() -> Int {
  g()
}

fn main() {
  dbg(h())
}
",
    );
    for name in ["g", "h"] {
        let before = summary(&calm, name);
        let after = summary(&loud, name);
        assert_eq!(
            before.join(after),
            after,
            "{name}: the louder callee only adds facts"
        );
    }
    assert!(!summary(&calm, "h").may_use_effect);
    assert!(
        summary(&loud, "h").may_use_effect,
        "h inherits g's stderr write"
    );
}
