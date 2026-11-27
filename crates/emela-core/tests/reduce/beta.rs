//! Tests for β reduction (`emela_core::opt::reduce::beta`).
//!
//! β turns `let f = |x| body; … f(a) …` into the body with the parameters bound to the arguments,
//! but only when `f` is used exactly once, as the callee of the call: a lambda passed as a value,
//! called twice, capturing itself, partially applied, suspending, taking the evidence table, or
//! called through a node marked `fails`/`suspends` is left alone. The observable behaviour of the
//! expansion is checked by the mutation pair `beta_fused.emel` / `beta_expanded.emel` in
//! `emela-driver/tests/opt_diff.rs`; the tests here fix the shape the pass leaves behind.
//!
//! The tests lower with the optimisations off and run only β (plus the older passes without a
//! flag), so a binding the test looks for is not removed by ζ first.

use crate::support;

use emela_core::opt::{CoreFns, Options, Rules};
use emela_core::{dump, opt};
use support::lower_files_with_rules;

/// The flags of a β-only run.
fn beta_only() -> Rules {
    Rules {
        beta: true,
        ..Rules::none()
    }
}

/// The body of the function `name` in the dump, without the declarations that follow it.
fn body_of<'a>(ir: &'a str, name: &str) -> &'a str {
    let marker = format!("fn {name}(");
    let start = ir
        .find(&marker)
        .unwrap_or_else(|| panic!("{name} is in the dump:\n{ir}"));
    ir[start..].split("\n\n").next().unwrap()
}

/// The closure and the indirect call are gone: the call site is the lambda's body with the
/// argument bound to its parameter. The closure captures `base`, which stays in scope.
#[test]
fn one_use_as_callee_expands() {
    let files = [(
        "main.emel",
        "\
fn main() {
  base = 10
  f = |x| x + base
  dbg(f(2))
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, beta_only());
    assert!(stats.beta > 0, "{stats:?}");
    let ir = dump(&m);
    let main = body_of(&ir, "main");
    assert!(!main.contains("fn("), "the closure is gone:\n{main}");
    assert!(!main.contains(")("), "the indirect call is gone:\n{main}");
    assert!(
        main.contains(": Int = 2"),
        "the argument is bound at the call site:\n{main}"
    );
}

/// A lambda with no parameters leaves no binding at all: the call site becomes the body.
#[test]
fn a_lambda_without_parameters_is_replaced_by_its_body() {
    let files = [(
        "main.emel",
        "\
fn main() {
  f = || 3
  dbg(f())
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, beta_only());
    assert!(stats.beta > 0, "{stats:?}");
    let ir = dump(&m);
    let main = body_of(&ir, "main");
    assert!(!main.contains("fn("), "the closure is gone:\n{main}");
    assert!(
        main.contains("@dbg[Int](3)"),
        "the call is the literal:\n{main}"
    );
}

/// The arguments are bound once each, left to right, before the body. Substituting them into the
/// body instead of binding them would print them twice (or not at all).
#[test]
fn arguments_are_bound_once_left_to_right() {
    let files = [(
        "main.emel",
        "\
fn main() {
  f = |a, b| a - b
  dbg(f(dbg(1), dbg(2)))
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, beta_only());
    assert!(stats.beta > 0, "{stats:?}");
    let ir = dump(&m);
    let main = body_of(&ir, "main");
    let first = main
        .find(": Int = @dbg[Int](1)")
        .unwrap_or_else(|| panic!("the first argument is bound first:\n{main}"));
    let second = main
        .find(": Int = @dbg[Int](2)")
        .unwrap_or_else(|| panic!("the second argument is bound second:\n{main}"));
    let body = main
        .find("-[Int]")
        .unwrap_or_else(|| panic!("the body is at the call site:\n{main}"));
    assert!(
        first < second && second < body,
        "the arguments are bound left to right before the body:\n{main}"
    );
    assert_eq!(
        main.matches("@dbg[Int](1)").count(),
        1,
        "the first argument is evaluated once:\n{main}"
    );
    assert_eq!(
        main.matches("@dbg[Int](2)").count(),
        1,
        "the second argument is evaluated once:\n{main}"
    );
}

/// Two uses of the local: expanding one call would leave the lambda behind and grow the code, so
/// the whole `let` is left alone.
#[test]
fn a_lambda_used_twice_is_not_folded() {
    let files = [(
        "main.emel",
        "\
fn main() {
  f = |x| x + 1
  dbg(f(1) + f(2))
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, beta_only());
    assert_eq!(stats.beta, 0, "{stats:?}");
    let ir = dump(&m);
    let main = body_of(&ir, "main");
    assert!(main.contains("fn("), "the lambda stays:\n{main}");
    assert!(main.contains(")(1, ())"), "the first call stays:\n{main}");
    assert!(main.contains(")(2, ())"), "the second call stays:\n{main}");
}

/// The one use of the local is an argument, not the callee of a call: the lambda escapes as a
/// value, so β does not apply.
#[test]
fn a_lambda_passed_as_a_value_is_not_folded() {
    let files = [(
        "main.emel",
        "\
fn apply_twice(f: fn(Int) -> Int, n: Int) -> Int {
  if n == 0 { f(0) } else { apply_twice(f, n - 1) }
}

fn main() {
  f = |x| x + 1
  dbg(apply_twice(f, 1))
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, beta_only());
    assert_eq!(stats.beta, 0, "{stats:?}");
    let ir = dump(&m);
    let main = body_of(&ir, "main");
    assert!(main.contains("fn("), "the lambda stays:\n{main}");
    assert!(
        main.contains("apply_twice"),
        "the lambda is passed as a value:\n{main}"
    );
}

/// A lambda takes the evidence table as its last parameter, which the expansion binds to the
/// call's table like the other arguments, so the operation in its body still finds the handler.
#[test]
fn a_lambda_that_takes_the_evidence_table_is_folded() {
    let files = [(
        "main.emel",
        "\
effect Log {
  fn info(msg: String) -> ()
}

handler ConsoleLog: Log {
  fn info(msg) {
    dbg(msg)
    ()
  }
}

fn run() -> Int uses { Log } {
  f = |x| {
    (use Log).info(\"f\")
    x + 1
  }
  f(1)
}

fn main() {
  with ConsoleLog { dbg(run()) }
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, beta_only());
    assert!(stats.beta > 0, "{stats:?}");
    let ir = dump(&m);
    let run = body_of(&ir, "run");
    assert!(!run.contains("fn("), "the lambda is gone:\n{run}");
    assert!(!run.contains(")(1"), "the call is gone:\n{run}");
}

/// A suspending lambda captures the rest of the caller's computation; moving its body to the call
/// site would move the suspension point, so it is left as a call.
#[test]
fn a_suspending_lambda_is_not_folded() {
    let files = [(
        "main.emel",
        "\
effect Tick {
  suspend fn tick(n: Int) -> Int
}

handler Instant: Tick {
  suspend fn tick(n, resume) {
    resume(n)
  }
}

fn main() {
  with Instant {
    f = |n| (use Tick).tick(n) + 1
    dbg(f(1))
  }
}
",
    )];
    // The handler's resume, a lambda over the runtime's continuation called once, is folded;
    // the suspending lambda in main is not.
    let (m, _) = lower_files_with_rules(&files, beta_only());
    let ir = dump(&m);
    let main = body_of(&ir, "main");
    assert!(main.contains("fn(n"), "the lambda stays:\n{main}");
    assert!(main.contains(")(1,"), "the call stays:\n{main}");
}

/// An expansion over the budget is abandoned whole, so the closure and the call stay.
#[test]
fn an_expansion_over_budget_is_not_folded() {
    let literals: Vec<String> = (1..=130).map(|i| i.to_string()).collect();
    let source = format!(
        "fn main() {{\n  f = |x| ({})\n  dbg(f(0))\n}}\n",
        literals.join(", ")
    );
    let files = [("main.emel", source.as_str())];
    let (m, stats) = lower_files_with_rules(&files, beta_only());
    assert_eq!(stats.beta, 0, "{stats:?}");
    let ir = dump(&m);
    let main = body_of(&ir, "main");
    assert!(main.contains("fn("), "the lambda stays:\n{main}");
}

/// With β off the binding and the call are exactly as lowering wrote them.
#[test]
fn rules_none_leaves_it() {
    let files = [(
        "main.emel",
        "\
fn main() {
  f = |x| x + 1
  dbg(f(2))
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, Rules::none());
    assert_eq!(stats.beta, 0, "{stats:?}");
    let ir = dump(&m);
    let main = body_of(&ir, "main");
    assert!(main.contains("fn("), "the lambda stays:\n{main}");
    assert!(main.contains(")(2, ())"), "the call stays:\n{main}");
}

/// The first run expands the binding; a second run finds nothing, so the pass is idempotent.
#[test]
fn a_second_run_changes_nothing() {
    let files = [(
        "main.emel",
        "\
fn main() {
  base = 10
  f = |x| x + base
  dbg(f(2))
}
",
    )];
    let (mut m, stats) = lower_files_with_rules(&files, beta_only());
    assert!(stats.beta > 0, "{stats:?}");
    let again = opt::run_with(
        &mut m,
        &CoreFns::default(),
        Options {
            rules: beta_only(),
            ..Options::default()
        },
    );
    assert_eq!(again.beta, 0, "{again:?}");
}
