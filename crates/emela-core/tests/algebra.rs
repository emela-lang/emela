//! Tests for algebraic reduction (`emela_core::opt::reduce::algebra`).
//!
//! The rule removes a constant side of `&&`, `||`, `if` and `!!`. The interesting half of the
//! tests is what must *not* change: a side that is evaluated today and may have an effect (a
//! call), and an `if` whose taken arm is the literal empty string (the native target would turn
//! the run-time separator into a compile error). The `if` guard is also exercised through the
//! driver by `emela-driver/tests/algebra.rs`, which builds and runs the program.
//!
//! Every test lowers with the Core IR optimisations off and then runs the pipeline with only the
//! `algebra` rule on ([`support::lower_files_with_rules`]), because the per-rule flags are off by
//! default until A10/PR-019. The observable behaviour of the reduced programs is checked by the
//! mutation pair `algebra_fused.emel` / `algebra_expanded.emel` in `emela-driver/tests/opt_diff.rs`;
//! the tests here fix the shape the rule leaves behind.

mod support;

use emela_core::dump;
use emela_core::opt::{CoreFns, Options, Rules, Stats, run_with};
use support::lower_files_with_rules;

/// Only algebraic reduction is on: no other reduction rule may do part of what a test asserts.
const ALGEBRA: Rules = Rules {
    algebra: true,
    ..Rules::none()
};

/// Lowers `main` with `body` as its body, with only algebraic reduction on.
fn main_ir(body: &str) -> (String, Stats) {
    let source = format!("fn main() {{\n{body}\n}}\n");
    let files = [("main.emel", source.as_str())];
    let (m, stats) = lower_files_with_rules(&files, ALGEBRA);
    (dump(&m), stats)
}

/// Lowers the whole `source`, with only algebraic reduction on.
fn program_ir(source: &str) -> (String, Stats) {
    let files = [("main.emel", source)];
    let (m, stats) = lower_files_with_rules(&files, ALGEBRA);
    (dump(&m), stats)
}

/// The dump of the body of `main` alone: the module's other functions are not what a test of one
/// expression is about.
fn main_only(ir: &str) -> &str {
    let Some(rest) = ir.split("export fn main").nth(1) else {
        return ir;
    };
    rest.split("\nfn ").next().unwrap_or(rest)
}

/// `true && e` is `e`, `false && e` is `false` (the right side is not evaluated either way), and
/// `e && true` is `e`.
#[test]
fn and_with_a_constant_folds() {
    let (ir, stats) = main_ir(
        "\
  x = True
  dbg(True && x)
  dbg(False && x)
  dbg(x && True)",
    );
    assert!(stats.algebra >= 3, "{stats:?}");
    let main = main_only(&ir);
    assert!(!main.contains("&&[Bool]"), "{ir}");
    assert!(main.contains("@dbg[Bool](false)"), "{ir}");
}

/// `e && false` is `false` when `e` is trivial: dropping it evaluates nothing.
#[test]
fn and_false_drops_a_trivial_left_side() {
    let (ir, stats) = main_ir(
        "\
  x = True
  dbg(x && False)",
    );
    assert!(stats.algebra >= 1, "{stats:?}");
    let main = main_only(&ir);
    assert!(!main.contains("&&[Bool]"), "{ir}");
    assert!(main.contains("@dbg[Bool](false)"), "{ir}");
}

/// The mirror cases of `||`: `false || e` is `e`, `true || e` is `true`, `e || false` is `e`, and
/// `e || true` is `true` when `e` is trivial.
#[test]
fn or_with_a_constant_folds() {
    let (ir, stats) = main_ir(
        "\
  x = False
  dbg(False || x)
  dbg(True || x)
  dbg(x || False)
  dbg(x || True)",
    );
    assert!(stats.algebra >= 4, "{stats:?}");
    let main = main_only(&ir);
    assert!(!main.contains("||[Bool]"), "{ir}");
    assert!(main.contains("@dbg[Bool](true)"), "{ir}");
}

/// A call is not trivial: `e && false` must keep the call, which may print, fail or have another
/// effect, even though its value cannot change the result. The site is a deopt, not a miss.
#[test]
fn and_false_keeps_a_non_trivial_left_side() {
    let (ir, stats) = program_ir(
        "\
fn shout(x: Int) -> Bool {
  dbg(x)
  True
}

fn main() {
  dbg(shout(1) && False)
}
",
    );
    assert_eq!(stats.algebra, 0, "{stats:?}");
    assert!(stats.deopt >= 1, "{stats:?}");
    assert!(main_only(&ir).contains("&&[Bool]"), "{ir}");
}

/// `e || true` with a non-trivial `e` keeps the evaluation of `e` for the same reason.
#[test]
fn or_true_keeps_a_non_trivial_left_side() {
    let (ir, stats) = program_ir(
        "\
fn shout(x: Int) -> Bool {
  dbg(x)
  True
}

fn main() {
  dbg(shout(1) || True)
}
",
    );
    assert_eq!(stats.algebra, 0, "{stats:?}");
    assert!(stats.deopt >= 1, "{stats:?}");
    assert!(main_only(&ir).contains("||[Bool]"), "{ir}");
}

/// `if` on a literal is the arm it takes, and the other arm is not evaluated.
#[test]
fn if_on_a_literal_takes_its_arm() {
    let (ir, stats) = main_ir(
        "\
  dbg(if True { 1 } else { 2 })
  dbg(if False { 3 } else { 4 })",
    );
    assert!(stats.algebra >= 2, "{stats:?}");
    let main = main_only(&ir);
    assert!(!main.contains("if "), "{main}");
    assert!(main.contains("@dbg[Int](1)"), "{main}");
    assert!(main.contains("@dbg[Int](4)"), "{main}");
}

/// An `if` whose selected arm is the literal empty string is not folded: the native target rejects
/// a constant empty separator at compile time (`[target.native.unsupported]`), while a separator
/// that is empty at run time is a run-time defect, so the `if` has to stay. The site is a deopt.
#[test]
fn an_empty_string_arm_is_not_folded() {
    let (ir, stats) = main_ir("dbg(if True { \"\" } else { \",\" })");
    assert_eq!(stats.algebra, 0, "{stats:?}");
    assert!(stats.deopt >= 1, "{stats:?}");
    let main = main_only(&ir);
    assert!(main.contains("if "), "{main}");
}

/// Only the empty string is held back: an `if` with any other string arm folds to that arm.
#[test]
fn a_non_empty_string_arm_is_folded() {
    let (ir, stats) = main_ir("dbg(if True { \"a\" } else { \"b\" })");
    assert!(stats.algebra >= 1, "{stats:?}");
    let main = main_only(&ir);
    assert!(!main.contains("if "), "{main}");
    assert!(main.contains("\"a\""), "{main}");
}

/// `!!e` is `e` when both `!`s are the Bool one.
#[test]
fn not_folds() {
    let (ir, stats) = main_ir(
        "\
  x = True
  dbg(!!x)",
    );
    assert!(stats.algebra >= 1, "{stats:?}");
    let main = main_only(&ir);
    assert!(!main.contains("![Bool]"), "{ir}");
    assert!(main.contains("@dbg[Bool](x"), "{ir}");
}

/// With the rule off, nothing is reduced and the counter stays zero.
#[test]
fn rules_none_keeps_the_operation() {
    let files = [("main.emel", "fn main() {\n  dbg(True && False)\n}\n")];
    let (m, stats) = lower_files_with_rules(&files, Rules::none());
    assert_eq!(stats.algebra, 0, "{stats:?}");
    let ir = dump(&m);
    assert!(main_only(&ir).contains("&&[Bool]"), "{ir}");
}

/// The rule is idempotent: a run of the pipeline on the IR it already optimised changes nothing.
#[test]
fn a_second_run_changes_nothing() {
    let source = "\
fn main() {
  x = True
  dbg(True && x)
  dbg(x && False)
  dbg(False || x)
  dbg(x || False)
  dbg(!!x)
  dbg(if True { 1 } else { 2 })
  dbg(if True { \"\" } else { \",\" })
}
";
    let files = [("main.emel", source)];
    let (mut m, _) = lower_files_with_rules(&files, ALGEBRA);
    let once = dump(&m);
    let second = run_with(&mut m, &CoreFns::default(), Options { rules: ALGEBRA });
    assert!(second.is_empty(), "{second:?}");
    assert_eq!(dump(&m), once);
}
