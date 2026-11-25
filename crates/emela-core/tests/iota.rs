//! Tests for the reduction of a `match` of a known constructor (`emela_core::opt::reduce::iota`).
//!
//! A `match` whose scrutinee is a constructor or a tuple written at the `match` selects its arm
//! before the program runs. The tests fix the three things the rewrite must keep: the value, the
//! once-only left-to-right evaluation of the scrutinee's arguments (including the ones the pattern
//! ignores), and the deopts (a guarded arm, an arm that might match, a scrutinee that is not a
//! constructor or tuple, a Float literal pattern, a pattern deeper than the budget).
//!
//! Every test lowers with the Core IR optimisations off and then runs the pipeline with only the
//! `iota` and `delta` rules on ([`support::lower_files_with_rules`]); δ is what exposes a callee's
//! constructor at the caller's match. ζ and folding stay off, so the `let` bindings the reduction makes and
//! the operations on their values are exactly what the rule left; only the rule under test may do
//! part of what a test asserts.
//!
//! The observable behaviour of the reduction is checked by the mutation pair
//! `iota_fused.emel` / `iota_expanded.emel` in `emela-driver/tests/opt_diff.rs`; the tests here fix
//! the shape the pass leaves behind.

use crate::support;

use emela_core::dump;
use emela_core::opt::{CoreFns, Options, Rules, run_with};
use support::{lower_files, lower_files_with_rules};

/// ι and δ are on: δ is what puts a callee's constructor where the caller matches it (ι must run
/// after δ), and δ ran unconditionally before A10/PR-019 gated it. No other reduction rule may do
/// part of what a test asserts.
const IOTA: Rules = Rules {
    iota: true,
    delta: true,
    ..Rules::none()
};

/// The body of the function `name` in the dump.
fn body_of<'a>(ir: &'a str, name: &str) -> &'a str {
    let start = format!("fn {name}");
    let rest = ir
        .split(&start)
        .nth(1)
        .unwrap_or_else(|| panic!("{name} is in the dump:\n{ir}"));
    rest.split("\nfn ").next().unwrap()
}

/// The index of the first occurrence of `needle` in `haystack`, with a message if it is absent.
fn at(haystack: &str, needle: &str) -> usize {
    haystack
        .find(needle)
        .unwrap_or_else(|| panic!("`{needle}` is in the dump:\n{haystack}"))
}

/// `match Some(1)` with the `Some` arm binding the value: the field becomes the binding.
#[test]
fn a_some_of_a_literal_reduces_to_its_arm() {
    let files = [(
        "main.emel",
        "\
fn main() {
  dbg(Some(1) match {
    Some(x) -> x
    None -> 0
  })
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, IOTA);
    assert_eq!(stats.iota, 1, "{stats:?}");
    let ir = dump(&m);
    let main = body_of(&ir, "main");
    assert!(!main.contains("match"), "{main}");
    assert!(!main.contains("Option.Some"), "{main}");
    // The reduction binds the field to `x`; ζ is off, so the binding stays.
    assert!(main.contains("let x"), "{main}");
    assert!(main.contains("x%"), "{main}");
}

/// The `None` arm is selected, so the body does not mention the `Some` field at all.
#[test]
fn a_none_selects_the_none_arm() {
    let files = [(
        "main.emel",
        "\
fn main() {
  dbg(None match {
    Some(x) -> x
    None -> 0
  })
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, IOTA);
    assert_eq!(stats.iota, 1, "{stats:?}");
    let ir = dump(&m);
    let main = body_of(&ir, "main");
    assert!(!main.contains("match"), "{main}");
    assert!(main.contains("@dbg[Int](0)"), "{main}");
}

/// A record is destructured field by field, in field order, without building the record.
#[test]
fn a_record_is_destructured_field_by_field() {
    let files = [(
        "main.emel",
        "\
type Point(x: Int, y: Int)

fn main() {
  dbg(Point(x: 1, y: 2) match {
    Point(x, y) -> x + y
  })
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, IOTA);
    assert_eq!(stats.iota, 1, "{stats:?}");
    let ir = dump(&m);
    let main = body_of(&ir, "main");
    assert!(!main.contains("match"), "{main}");
    assert!(!main.contains("Point("), "{main}");
    // The reduction binds `x` and `y` in field order; ζ is off, so both bindings stay and the sum
    // adds the two locals.
    assert!(at(main, "let x") < at(main, "let y"), "{main}");
    assert!(main.contains("+[Int]"), "{main}");
}

/// A tuple is destructured like a constructor, with one binding per field.
#[test]
fn a_tuple_is_destructured() {
    let files = [(
        "main.emel",
        "\
fn main() {
  dbg((1, 2) match {
    (a, b) -> a + b
  })
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, IOTA);
    assert_eq!(stats.iota, 1, "{stats:?}");
    let ir = dump(&m);
    let main = body_of(&ir, "main");
    assert!(!main.contains("match"), "{main}");
    // The reduction binds the fields to `a` and `b`; ζ is off, so both bindings stay.
    assert!(at(main, "let a") < at(main, "let b"), "{main}");
    assert!(main.contains("+[Int]"), "{main}");
}

/// A guard is evaluated after the pattern matches and may depend on the program's state: the arm
/// is not known to be selected, so the `match` stays and the site is a deopt.
#[test]
fn a_guarded_arm_is_not_reduced() {
    let files = [(
        "main.emel",
        "\
fn main() {
  dbg(Some(1) match {
    Some(x) if x > 0 -> 1
    _ -> 0
  })
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, IOTA);
    assert_eq!(stats.iota, 0, "{stats:?}");
    assert_eq!(stats.deopt, 1, "{stats:?}");
    let ir = dump(&m);
    let main = body_of(&ir, "main");
    assert!(main.contains("match Option.Some"), "{main}");
    assert!(main.contains("if (x"), "{main}");
}

/// A `No` arm above the selected one is skipped, as the run time does: the later `Some` arm is
/// the one the value selects.
#[test]
fn an_arm_that_does_not_match_is_skipped() {
    let files = [(
        "main.emel",
        "\
fn main() {
  dbg(Some(1) match {
    None -> 0
    Some(x) -> x
  })
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, IOTA);
    assert_eq!(stats.iota, 1, "{stats:?}");
    let ir = dump(&m);
    let main = body_of(&ir, "main");
    assert!(!main.contains("match"), "{main}");
    // The selected arm binds the field to `x`; ζ is off, so the binding stays.
    assert!(main.contains("let x"), "{main}");
    assert!(!main.contains("Option.Some"), "{main}");
}

/// An arm that might match stops the reduction: the first arm compares the `Some` field with a
/// literal, but the field is a call, and whether the envelope matches is not known without running
/// the program. The `match` is left whole, so its evaluation order and effects are unchanged.
#[test]
fn an_unknown_arm_stops_the_reduction() {
    let files = [(
        "main.emel",
        "\
fn shout(x: Int) -> Int {
  dbg(x)
  x
}

fn main() {
  dbg(Some(shout(1)) match {
    Some(0) -> 0
    Some(x) -> x
    None -> -1
  })
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, IOTA);
    assert_eq!(stats.iota, 0, "{stats:?}");
    // The site is examined once per sweep: inlining `shout` changes the module in the first
    // sweep, so the rule sees the `match` again in the second.
    assert!(stats.deopt > 0, "{stats:?}");
    let ir = dump(&m);
    let main = body_of(&ir, "main");
    assert!(main.contains("match"), "{main}");
    assert!(main.contains("Option.Some(0)"), "{main}");
}

/// The scrutinee's arguments are evaluated once each, left to right, including the field the
/// pattern ignores (`_`), which the reduction binds to a fresh local. `shout` prints its argument,
/// so the dump shows the order and the number of the evaluations.
#[test]
fn every_argument_is_evaluated_once_in_order() {
    let files = [(
        "main.emel",
        "\
fn shout(x: Int) -> Int {
  dbg(x)
  x
}

fn main() {
  dbg((shout(1), shout(2)) match {
    (a, _) -> a
  })
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, IOTA);
    assert_eq!(stats.iota, 1, "{stats:?}");
    let ir = dump(&m);
    let main = body_of(&ir, "main");
    assert!(!main.contains("match"), "{main}");
    assert_eq!(
        main.matches("@dbg[Int](").count(),
        3,
        "each argument is printed once, and the result once:\n{main}"
    );
    // Inlining binds each argument of `shout` to its parameter, and ζ is off, so the dump shows
    // the literals at those bindings rather than inside the `dbg` calls.
    let first = at(main, ": Int = 1");
    let second = at(main, ": Int = 2");
    assert!(
        first < second,
        "the arguments are evaluated left to right:\n{main}"
    );
    assert_eq!(
        main.matches(": Int = 1").count(),
        1,
        "the first argument is evaluated once:\n{main}"
    );
    assert_eq!(
        main.matches(": Int = 2").count(),
        1,
        "the second argument is evaluated once:\n{main}"
    );
    assert!(
        main.contains("let t"),
        "the ignored field is evaluated into a fresh local:\n{main}"
    );
}

/// A tuple nested in a constructor is destructured in place: the fields of the inner tuple are
/// evaluated once each, in order, and the inner tuple and the `Some` are not allocated.
#[test]
fn a_nested_constructor_is_destructured_in_place() {
    let files = [(
        "main.emel",
        "\
fn shout(x: Int) -> Int {
  dbg(x)
  x
}

fn main() {
  dbg(Some((shout(1), shout(2))) match {
    Some((a, _)) -> a
    None -> 0
  })
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, IOTA);
    assert_eq!(stats.iota, 1, "{stats:?}");
    let ir = dump(&m);
    let main = body_of(&ir, "main");
    assert!(!main.contains("match"), "{main}");
    assert!(!main.contains("Option.Some"), "{main}");
    assert_eq!(
        main.matches("@dbg[Int](").count(),
        3,
        "each argument is printed once, and the result once:\n{main}"
    );
    let first = at(main, ": Int = 1");
    let second = at(main, ": Int = 2");
    assert!(first < second, "{main}");
}

/// A catch-all binding is a plain `let` of the whole scrutinee: the value is evaluated once, even
/// when its pattern does not take it apart.
#[test]
fn a_catch_all_becomes_a_let() {
    let files = [(
        "main.emel",
        "\
fn main() {
  dbg(Some(1) match {
    x -> x
  })
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, IOTA);
    assert_eq!(stats.iota, 1, "{stats:?}");
    let ir = dump(&m);
    let main = body_of(&ir, "main");
    assert!(!main.contains("match"), "{main}");
    assert!(
        main.contains("let x") && main.contains("Option.Some[Int](1)"),
        "{main}"
    );
}

/// A catch-all `_` whose scrutinee has an effect still evaluates the scrutinee once, into a fresh
/// local whose value the body ignores.
#[test]
fn a_catch_all_wild_evaluates_an_effectful_scrutinee() {
    let files = [(
        "main.emel",
        "\
fn shout(x: Int) -> Int {
  dbg(x)
  x
}

fn main() {
  dbg((shout(1), shout(2)) match {
    _ -> 0
  })
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, IOTA);
    assert_eq!(stats.iota, 1, "{stats:?}");
    let ir = dump(&m);
    let main = body_of(&ir, "main");
    assert!(!main.contains("match"), "{main}");
    assert!(main.contains("let t"), "{main}");
    assert_eq!(
        main.matches("@dbg[Int](").count(),
        3,
        "each argument is printed once, and the result once:\n{main}"
    );
    let first = at(main, ": Int = 1");
    let second = at(main, ": Int = 2");
    assert!(first < second, "{main}");
}

/// A catch-all `_` over a scrutinee without an effect needs no local: dropping the trivial value
/// changes nothing the program can observe.
#[test]
fn a_catch_all_wild_drops_a_trivial_scrutinee() {
    let files = [(
        "main.emel",
        "\
fn main() {
  dbg((1, 2) match {
    _ -> 7
  })
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, IOTA);
    assert_eq!(stats.iota, 1, "{stats:?}");
    let ir = dump(&m);
    let main = body_of(&ir, "main");
    assert!(!main.contains("match"), "{main}");
    assert!(!main.contains("let t"), "{main}");
    assert!(main.contains("@dbg[Int](7)"), "{main}");
}

/// `match Array.get` is the Option fusion's shape, and the fusion runs before this pass: the
/// bounds test and the checked load are there, and this pass did not count a reduction.
#[test]
fn a_match_of_array_get_is_left_to_the_option_fusion() {
    let files = [(
        "main.emel",
        "\
import Std.Array.{Array}

fn main() {
  xs = Array.from_list([1, 2, 3])
  dbg(Array.get(xs, 1) match {
    Some(x) -> x
    None -> -1
  })
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, IOTA);
    assert_eq!(stats.array_get_match, 1, "{stats:?}");
    assert_eq!(stats.iota, 0, "{stats:?}");
    // A `match` of any other scrutinee is not a candidate, so it is not a deopt either.
    assert_eq!(stats.deopt, 0, "{stats:?}");
    let ir = dump(&m);
    let main = body_of(&ir, "main");
    assert!(!main.contains("match"), "{main}");
    assert!(main.contains("@$Array.load_unchecked"), "{main}");
}

/// A Float literal pattern is never compared: the specification matches Float with `==`, where
/// `0.0` matches `-0.0` (spec `[fn.match]`), so a bit comparison would select the wrong arm
/// for this value. A comparison by `f64` equality would get NaN wrong instead. The match is left
/// at run time and the site is a deopt.
#[test]
fn a_signed_zero_float_pattern_is_left_to_run_time() {
    let files = [(
        "main.emel",
        "\
fn main() {
  dbg((-0.0, 2) match {
    (0.0, y) -> y
    _ -> 0
  })
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, IOTA);
    assert_eq!(stats.iota, 0, "{stats:?}");
    assert_eq!(stats.deopt, 1, "{stats:?}");
    let ir = dump(&m);
    let main = body_of(&ir, "main");
    assert!(main.contains("match"), "{main}");
    assert!(main.contains("0.0"), "{main}");
}

/// A Float literal nested in a constructor pattern is a deopt for the same reason; the rule does
/// not reduce the `match` even though the constructor and the fields have the right shapes.
#[test]
fn a_float_literal_pattern_in_a_constructor_deopts() {
    let files = [(
        "main.emel",
        "\
fn main() {
  dbg(Some(1.0) match {
    Some(1.0) -> 1
    _ -> 0
  })
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, IOTA);
    assert_eq!(stats.iota, 0, "{stats:?}");
    assert_eq!(stats.deopt, 1, "{stats:?}");
    let ir = dump(&m);
    let main = body_of(&ir, "main");
    assert!(main.contains("match Option.Some"), "{main}");
}

/// The source of a `match` on a tuple of `layers` nested tuple patterns, with a binding at the
/// innermost field.
fn nested_tuple_match(layers: usize) -> String {
    let mut value = "1".to_string();
    let mut pattern = "x".to_string();
    for _ in 0..layers {
        value = format!("({value}, 0)");
        pattern = format!("({pattern}, _)");
    }
    format!("fn main() {{\n  dbg({value} match {{\n    {pattern} -> x\n  }})\n}}\n")
}

/// The pattern-depth budget keeps the recursion bounded. A pattern nested 64 levels deep is still
/// reduced; one level deeper is a deopt and is left to run time, so a compiler-generated pattern
/// cannot overflow the stack.
#[test]
fn a_pattern_deeper_than_the_budget_is_left_to_run_time() {
    let source = nested_tuple_match(64);
    let files = [("main.emel", source.as_str())];
    let (m, stats) = lower_files_with_rules(&files, IOTA);
    assert_eq!(stats.iota, 1, "{stats:?}");
    assert!(!dump(&m).contains("match ("), "the deep match is reduced");

    let source = nested_tuple_match(65);
    let files = [("main.emel", source.as_str())];
    let (m, stats) = lower_files_with_rules(&files, IOTA);
    assert_eq!(stats.iota, 0, "{stats:?}");
    assert_eq!(stats.deopt, 1, "{stats:?}");
    let ir = dump(&m);
    let main = body_of(&ir, "main");
    assert!(main.contains("match ("), "{main}");
}

/// Running the pass again changes nothing: the reduction removes the `match`, and the bindings it
/// leaves are not a shape it reduces.
#[test]
fn a_second_run_changes_nothing() {
    let files = [(
        "main.emel",
        "\
fn shout(x: Int) -> Int {
  dbg(x)
  x
}

fn main() {
  dbg(Some(shout(1)) match {
    Some(x) -> x
    None -> 0
  })
}
",
    )];
    let mut m = lower_files(&files);
    let core_fns = CoreFns::default();
    let options = Options {
        rules: IOTA,
        ..Options::default()
    };
    let first = run_with(&mut m, &core_fns, options);
    assert!(first.iota > 0, "{first:?}");
    let once = dump(&m);
    let second = run_with(&mut m, &core_fns, options);
    assert!(second.is_empty(), "a second run changed the IR: {second:?}");
    assert_eq!(dump(&m), once);
    emela_core::verify(&m).unwrap();
}
