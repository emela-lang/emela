//! Tests for constant folding (`emela_core::opt::reduce::fold`).
//!
//! The rule replaces an operation on literals by the literal it evaluates to. The interesting half
//! of the tests is what must *not* change: a division or remainder by zero (a defect at run time),
//! Float arithmetic, structural equality, and an operation with an effectful operand.
//!
//! Every test lowers with the Core IR optimisations off and then runs the pipeline with only the
//! `fold` rule on ([`support::lower_files_with_rules`]), because the per-rule flags are off by
//! default until A10/PR-019. The observable behaviour of the folded programs is checked by the
//! mutation pair `fold_fused.emel` / `fold_expanded.emel` in `emela-driver/tests/opt_diff.rs`; the
//! tests here fix the shape the rule leaves behind.

mod support;

use emela_core::dump;
use emela_core::opt::{CoreFns, Options, Rules, Stats, run_with};
use support::lower_files_with_rules;

/// Only constant folding is on: no other reduction rule may do part of what a test asserts.
const FOLD: Rules = Rules {
    fold: true,
    ..Rules::none()
};

/// Lowers `main` with `body` as its body, with only constant folding on.
fn main_ir(body: &str) -> (String, Stats) {
    let source = format!("fn main() {{\n{body}\n}}\n");
    let files = [("main.emel", source.as_str())];
    let (m, stats) = lower_files_with_rules(&files, FOLD);
    (dump(&m), stats)
}

/// Lowers the whole `source`, with only constant folding on.
fn program_ir(source: &str) -> (String, Stats) {
    let files = [("main.emel", source)];
    let (m, stats) = lower_files_with_rules(&files, FOLD);
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

/// `1 + 2` is `3`: the operation is gone from the IR.
#[test]
fn literal_arithmetic_folds() {
    let (ir, stats) = main_ir("dbg(1 + 2)");
    assert!(stats.constant >= 1, "{stats:?}");
    assert!(main_only(&ir).contains("@dbg[Int](3)"), "{ir}");
    assert!(!main_only(&ir).contains("+[Int]"), "{ir}");
}

/// Integer addition wraps in two's complement (spec `[target.number]`), so the folded value is
/// the wrapped one, not a defect and not a wider value.
#[test]
fn integer_addition_wraps() {
    let (ir, stats) = main_ir("dbg(2147483647 + 1)");
    assert!(stats.constant >= 1, "{stats:?}");
    assert!(main_only(&ir).contains("@dbg[Int](-2147483648)"), "{ir}");
}

/// The minimum divided by `-1` overflows and wraps to the minimum (spec `[target.number]`). It is
/// written as `-2147483647 - 1` because a source literal cannot be the minimum (spec 2.4); the two
/// subtractions fold before the division, in the same sweep.
#[test]
fn the_minimum_divided_by_negative_one_wraps() {
    let (ir, stats) = main_ir("dbg((-2147483647 - 1) / -1)");
    assert!(stats.constant >= 3, "{stats:?}");
    assert!(main_only(&ir).contains("@dbg[Int](-2147483648)"), "{ir}");
}

/// A division by zero is a defect when the program runs, so it is never folded away. The site is
/// a deopt: the rule saw a fold candidate and kept it for the defect.
#[test]
fn division_by_zero_is_kept() {
    let (ir, stats) = main_ir("dbg(1 / 0)");
    assert_eq!(stats.constant, 0, "{stats:?}");
    assert_eq!(stats.deopt, 1, "{stats:?}");
    assert!(main_only(&ir).contains("/[Int]"), "{ir}");
    assert!(main_only(&ir).contains("@dbg[Int]((1 /[Int] 0))"), "{ir}");
}

/// The same for a remainder by zero.
#[test]
fn remainder_by_zero_is_kept() {
    let (ir, stats) = main_ir("dbg(1 % 0)");
    assert_eq!(stats.constant, 0, "{stats:?}");
    assert_eq!(stats.deopt, 1, "{stats:?}");
    assert!(main_only(&ir).contains("%[Int]"), "{ir}");
    assert!(main_only(&ir).contains("@dbg[Int]((1 %[Int] 0))"), "{ir}");
}

/// Comparisons of integer and Bool literals are Bool values. Bool gets only equality: it is an
/// enum in source, so it has no ordering.
#[test]
fn comparisons_fold_to_a_bool() {
    let (ir, stats) = main_ir("dbg(1 < 2)\ndbg(1 == 2)\ndbg(3 >= 3)\ndbg(True != False)");
    assert!(stats.constant >= 4, "{stats:?}");
    let main = main_only(&ir);
    assert!(main.contains("@dbg[Bool](true)"), "{ir}");
    assert!(main.contains("@dbg[Bool](false)"), "{ir}");
    assert!(!main.contains("<[Int]"), "{ir}");
    assert!(!main.contains("!=[Bool]"), "{ir}");
}

/// `!true` is `false` and `!!true` is `true`: both `Not`s fold, bottom-up.
#[test]
fn not_folds() {
    let (ir, stats) = main_ir("dbg(!True)\ndbg(!!True)");
    assert!(stats.constant >= 3, "{stats:?}");
    let main = main_only(&ir);
    assert!(!main.contains("![Bool]"), "{ir}");
    assert!(main.contains("@dbg[Bool](false)"), "{ir}");
    assert!(main.contains("@dbg[Bool](true)"), "{ir}");
}

/// Negation of an Int literal wraps, so negating the minimum gives the minimum again.
#[test]
fn negation_wraps() {
    let (ir, stats) = main_ir("dbg(-(-2147483647 - 1))");
    assert!(stats.constant >= 2, "{stats:?}");
    assert!(main_only(&ir).contains("@dbg[Int](-2147483648)"), "{ir}");
}

/// Float arithmetic is not folded: IEEE 754 rounding, NaN, the signed zero and infinities must
/// come from the operation, not from a compile-time evaluation.
#[test]
fn float_arithmetic_is_not_folded() {
    let (ir, stats) = main_ir("dbg(1.5 + 2.5)");
    assert_eq!(stats.constant, 0, "{stats:?}");
    assert!(main_only(&ir).contains("+[Float]"), "{ir}");
}

/// Structural equality is not folded: monomorphization may replace it with a call of an `Eq` impl,
/// and a literal's structure is not necessarily what that impl gives.
#[test]
fn structural_equality_is_not_folded() {
    let (ir, stats) = main_ir("dbg([1] == [1])");
    assert_eq!(stats.constant, 0, "{stats:?}");
    assert!(main_only(&ir).contains("==[Structural]"), "{ir}");
}

/// A comparison with an effectful side is not a comparison of literals, so it stays as it is.
#[test]
fn an_effectful_comparison_stays() {
    let (ir, stats) = program_ir(
        "\
fn shout(x: Int) -> Int {
  dbg(x)
  x
}

fn main() {
  dbg(shout(1) < 2)
}
",
    );
    assert_eq!(stats.constant, 0, "{stats:?}");
    assert!(main_only(&ir).contains("<[Int]"), "{ir}");
}

/// The `Int64` counterparts of the same operations. The local's `Int64` type is what makes the
/// literals `Int64` in the source (spec 2.4).
#[test]
fn int64_arithmetic_folds() {
    let (ir, stats) = main_ir("x: Int64 = 1 + 2\ndbg(x)");
    assert!(stats.constant >= 1, "{stats:?}");
    assert!(main_only(&ir).contains("3i64"), "{ir}");
    assert!(!main_only(&ir).contains("+[Int64]"), "{ir}");
}

/// `Int64` addition wraps, and the widened range keeps values an `Int` cannot hold.
#[test]
fn int64_addition_wraps() {
    let (ir, stats) = main_ir("x: Int64 = 9223372036854775807 + 1\ndbg(x)");
    assert!(stats.constant >= 1, "{stats:?}");
    assert!(main_only(&ir).contains("-9223372036854775808i64"), "{ir}");
}

/// The minimum `Int64` divided by `-1` wraps to the minimum.
#[test]
fn the_int64_minimum_divided_by_negative_one_wraps() {
    let (ir, stats) = main_ir("x: Int64 = (-9223372036854775807 - 1) / -1\ndbg(x)");
    assert!(stats.constant >= 3, "{stats:?}");
    assert!(main_only(&ir).contains("-9223372036854775808i64"), "{ir}");
}

/// A division by literal zero in `Int64` keeps the defect, as the `Int` one.
#[test]
fn int64_division_by_zero_is_kept() {
    let (ir, stats) = main_ir("x: Int64 = 1 / 0\ndbg(x)");
    assert_eq!(stats.constant, 0, "{stats:?}");
    assert_eq!(stats.deopt, 1, "{stats:?}");
    assert!(main_only(&ir).contains("/[Int64]"), "{ir}");
}

/// Negation of an `Int64` literal wraps.
#[test]
fn int64_negation_wraps() {
    let (ir, stats) = main_ir("x: Int64 = -(-9223372036854775807 - 1)\ndbg(x)");
    assert!(stats.constant >= 2, "{stats:?}");
    assert!(main_only(&ir).contains("-9223372036854775808i64"), "{ir}");
}

/// With the rule off, nothing is folded and the counter stays zero.
#[test]
fn rules_none_keeps_the_operation() {
    let files = [("main.emel", "fn main() {\n  dbg(1 + 2)\n}\n")];
    let (m, stats) = lower_files_with_rules(&files, Rules::none());
    assert_eq!(stats.constant, 0, "{stats:?}");
    let ir = dump(&m);
    assert!(main_only(&ir).contains("+[Int]"), "{ir}");
    assert!(main_only(&ir).contains("@dbg[Int]((1 +[Int] 2))"), "{ir}");
}

/// The rule is idempotent: a run of the pipeline on the IR it already optimised changes nothing.
#[test]
fn a_second_run_changes_nothing() {
    let source = "\
fn main() {
  dbg(1 + 2)
  dbg(1 < 2)
  dbg(!True)
}
";
    let files = [("main.emel", source)];
    let (mut m, _) = lower_files_with_rules(&files, FOLD);
    let once = dump(&m);
    let second = run_with(
        &mut m,
        &CoreFns::default(),
        Options {
            rules: FOLD,
            ..Options::default()
        },
    );
    assert!(second.is_empty(), "{second:?}");
    assert_eq!(dump(&m), once);
}
