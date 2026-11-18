//! Tests for the string builder law (D5/PR-032): `String.append` chains to one `concat`.
//!
//! The equation is `String.append(…String.append(a, b)…) = concat([a, b, …])`. Its right-hand side
//! is not an internal node with no source form: interpolation already lowers to the same
//! `Expr::Concat`, which the LIR builds as one `RuntimeOp::Concat` (one allocation). The tests
//! therefore compare the IR of the append chain after the law with the IR of the hand-written
//! interpolation, and pin the cases the law must leave alone:
//!
//! - a chain of two leaves (one append already allocates once);
//! - a chain whose leaves are all the empty string literal (lowering it would make the literal
//!   empty string, which the native target rejects as a constant `String.split` separator);
//! - an interpolation, which is already the law's shape.
//!
//! The driver pair `concat_fused.emel` / `concat_expanded.emel` runs both source forms on every
//! target; the law is off in the driver's default pipeline, so that pair checks that the two
//! forms agree, while this file turns the law on and checks the IR it builds.

mod support;

use emela_core::opt::fusion::{self, Laws};
use emela_core::opt::{CoreFns, Stats};
use emela_core::{Module, dump};

/// The law's left-hand side, as the driver's mutation pair writes it.
const CONCAT_FUSED: &str = include_str!("../../emela-driver/tests/programs/opt/concat_fused.emel");

/// The hand-written right-hand side: interpolation, which lowering already builds as one concat.
const CONCAT_EXPANDED: &str =
    include_str!("../../emela-driver/tests/programs/opt/concat_expanded.emel");

/// A two-leaf chain: one `String.append`, which already allocates once.
const TWO_LEAVES: &str = "\
fn build(a: String, b: String) -> String {
  String.append(a, b)
}
";

/// A chain whose leaves are all the empty string literal. Fusing it would make the literal empty
/// string, which changes the native target's compile-time rejection of a constant empty
/// `String.split` separator.
const ALL_EMPTY: &str = "\
fn sep() -> String {
  String.append(String.append(\"\", \"\"), \"\")
}

fn main() {
  dbg(String.split(\"abc\", sep()))
}
";

/// A chain with an empty literal and a value: only the all-empty chain is held back.
const EMPTY_AND_VALUE: &str = "\
fn build(s: String) -> String {
  String.append(String.append(\"\", s), \"\")
}
";

/// A module as lowering makes it, with the Core IR passes off. All roots are lowered, so a
/// function the test names is present even when `main` does not call it.
fn fixture(source: &str) -> Module {
    support::try_lower_off(&[("main.emel", source)], emela_core::Roots::All)
        .unwrap_or_else(|e| panic!("{e:?}"))
}

/// The string law alone.
fn only_string() -> Laws {
    let mut laws = Laws::none();
    laws.string_concat = true;
    laws
}

/// Runs `laws` on `m` and returns the sites rewritten and the counters.
fn apply(m: &mut Module, laws: Laws) -> (u32, Stats) {
    let mut stats = Stats::default();
    let applied = fusion::run(m, &CoreFns::default(), laws, &mut stats);
    assert!(emela_core::verify(m).is_ok(), "the law made ill-typed IR");
    (applied, stats)
}

/// The dump of the one function called `name`, without the rest of the module.
fn function_dump(m: &Module, name: &str) -> String {
    let ir = dump(m);
    let head = format!("fn {name}(");
    let start = ir
        .find(&head)
        .unwrap_or_else(|| panic!("no `{head}` in the dump"));
    let rest = &ir[start..];
    // A function closes with `}` in the first column; the blocks inside it are indented.
    let end = rest.find("\n}\n").expect("a function closes with `}`") + 2;
    rest[..end].to_owned()
}

/// The text of the first `concat(…)` call in `ir`, with the local ids erased, or `None`.
///
/// The two programs are lowered separately, so their local ids may differ; the shape of the
/// concat is what must be the same.
fn concat_shape(ir: &str) -> Option<String> {
    let start = ir.find("concat(")?;
    let call = &ir[start..];
    let mut depth = 0usize;
    let mut end = None;
    for (i, c) in call.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(i + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let mut shape = String::new();
    let mut chars = call[..end?].chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            while chars.peek().is_some_and(char::is_ascii_digit) {
                chars.next();
            }
            shape.push('%');
        } else {
            shape.push(c);
        }
    }
    Some(shape)
}

/// The equation on the hand-written pair: the append chain becomes the same concat as the
/// interpolation written by hand, and the law counts one application.
#[test]
fn an_append_chain_becomes_the_hand_written_concat() {
    let mut fused = fixture(CONCAT_FUSED);
    let before = function_dump(&fused, "build");
    assert!(before.contains("@String.append"), "{before}");
    assert!(!before.contains("concat("), "{before}");

    let (applied, stats) = apply(&mut fused, only_string());
    assert_eq!(applied, 1, "{stats:?}");
    assert_eq!(stats.deopt, 0, "{stats:?}");

    let after = function_dump(&fused, "build");
    assert!(after.contains("concat("), "{after}");
    assert!(!after.contains("@String.append"), "{after}");

    let mut expanded = fixture(CONCAT_EXPANDED);
    let (applied, _) = apply(&mut expanded, only_string());
    assert_eq!(applied, 0, "the interpolation is already the law's shape");
    let hand = function_dump(&expanded, "build");
    assert!(hand.contains("concat("), "{hand}");
    assert!(!hand.contains("@String.append"), "{hand}");

    assert_eq!(
        concat_shape(&after),
        concat_shape(&hand),
        "the fused chain and the hand-written interpolation have different shapes"
    );
}

/// The three parts are value parts of one concat, in left-to-right order: the `dbg` between the
/// operands stays between them, so the law moves no effect.
#[test]
fn the_concat_parts_keep_the_evaluation_order() {
    let mut m = fixture(CONCAT_FUSED);
    let (applied, _) = apply(&mut m, only_string());
    assert_eq!(applied, 1);

    let ir = function_dump(&m, "build");
    let start = ir.find("concat(").expect("one concat");
    let concat = &ir[start..];
    assert_eq!(
        concat.matches("@dbg").count(),
        3,
        "one dbg per operand: {concat}"
    );
    let a = concat.find("a%").expect("a part");
    let b = concat.find("b%").expect("b part");
    let c = concat.find("c%").expect("c part");
    assert!(a < b && b < c, "the parts are not left to right: {concat}");
}

/// A chain of two leaves is one append, which already allocates once: the law leaves it alone.
#[test]
fn a_two_leaf_chain_is_left_alone() {
    let mut m = fixture(TWO_LEAVES);
    let before = dump(&m);
    let (applied, stats) = apply(&mut m, only_string());
    assert_eq!(applied, 0, "{stats:?}");
    assert_eq!(stats.deopt, 0, "{stats:?}");
    assert_eq!(dump(&m), before);
}

/// A chain whose leaves are all the empty string literal stays an append chain: fusing it would
/// make the literal empty string, which changes the native target's compile-time diagnostic for
/// an empty `String.split` separator.
#[test]
fn an_all_empty_chain_is_left_alone() {
    let mut m = fixture(ALL_EMPTY);
    let before = function_dump(&m, "sep");
    assert!(before.contains("@String.append"), "{before}");
    assert!(!before.contains("concat("), "{before}");

    let (applied, stats) = apply(&mut m, only_string());
    assert_eq!(applied, 0, "{stats:?}");
    assert_eq!(stats.deopt, 0, "{stats:?}");
    assert_eq!(function_dump(&m, "sep"), before);
}

/// Only the all-empty chain is held back: an empty part next to a value still becomes one concat,
/// and the LIR drops the empty part as it does for interpolation.
#[test]
fn an_empty_part_next_to_a_value_still_fuses() {
    let mut m = fixture(EMPTY_AND_VALUE);
    let (applied, _) = apply(&mut m, only_string());
    assert_eq!(applied, 1);
    let ir = function_dump(&m, "build");
    assert!(ir.contains("concat("), "{ir}");
    assert!(!ir.contains("@String.append"), "{ir}");
}

/// A second run of the law changes nothing and reports nothing.
#[test]
fn a_second_run_changes_nothing() {
    let mut m = fixture(CONCAT_FUSED);
    let (first, _) = apply(&mut m, only_string());
    assert_eq!(first, 1);
    let once = dump(&m);

    let (second, stats) = apply(&mut m, only_string());
    assert_eq!(second, 0, "{stats:?}");
    assert!(stats.is_empty(), "{stats:?}");
    assert_eq!(stats.passes, 0, "{stats:?}");
    assert_eq!(dump(&m), once);
}

/// The per-sweep budget stops the law and counts it as one deopt; the module stays as it was.
#[test]
fn the_budget_stops_the_law() {
    let mut m = fixture(CONCAT_FUSED);
    let before = dump(&m);
    let mut stats = Stats::default();
    let applied =
        fusion::run_with_budget(&mut m, &CoreFns::default(), only_string(), &mut stats, 0);
    assert_eq!(applied, 0);
    assert_eq!(stats.deopt, 1, "{stats:?}");
    assert!(stats.is_empty(), "a deopt is not a rewrite: {stats:?}");
    assert_eq!(dump(&m), before);

    let mut stats = Stats::default();
    let applied =
        fusion::run_with_budget(&mut m, &CoreFns::default(), only_string(), &mut stats, 1);
    assert_eq!(applied, 1, "{stats:?}");
    assert_eq!(stats.deopt, 0, "{stats:?}");
    assert!(emela_core::verify(&m).is_ok());
}
