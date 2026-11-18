//! Tests for direct-call inlining (δ reduction, `emela_core::opt::reduce::delta`).
//!
//! δ fires only on calls that are safe to copy: a small, non-recursive, non-failing,
//! non-suspending function whose body creates no handler, catches no failure and touches no
//! evidence. The calls that stay are the interesting half of the tests: recursion, mutual
//! recursion, size, the bundled collection functions and derived impls must be left alone.
//!
//! The observable behaviour of the expansions is checked by the mutation pair
//! `inline_fused.emel` / `inline_expanded.emel` in `emela-driver/tests/opt_diff.rs`; the tests
//! here fix the shape the rule leaves behind.

mod support;

use emela_core::opt::CoreFns;
use emela_core::{Callee, Expr, FnId, Module, dump, opt, verify};
use support::{lower_files_with, lower_files_with_stats};

/// The one function called `name`, which lowering keeps under its IR name for the entry module.
fn function(m: &Module, name: &str) -> FnId {
    let found: Vec<FnId> = m
        .functions
        .iter()
        .filter(|(_, f)| f.name == name)
        .map(|(id, _)| id)
        .collect();
    assert_eq!(found.len(), 1, "functions named `{name}`: {found:?}");
    found[0]
}

/// The number of direct calls to `target` in `e`.
fn direct_calls(e: &Expr, target: FnId) -> usize {
    let mut count = match e {
        Expr::Call {
            callee: Callee::Direct { f, .. },
            ..
        } if *f == target => 1,
        _ => 0,
    };
    e.for_each_child(&mut |c| count += direct_calls(c, target));
    count
}

/// The number of direct calls to `target` in the body of `owner`.
fn calls_in(m: &Module, owner: FnId, target: FnId) -> usize {
    direct_calls(&m.functions[owner].body, target)
}

/// The number of direct calls to `target` anywhere in the module.
fn calls_anywhere(m: &Module, target: FnId) -> usize {
    m.functions
        .iter()
        .map(|(_, f)| direct_calls(&f.body, target))
        .sum()
}

/// A call to a small pure function is replaced by its body, and the callee stays in the module
/// for its other callers.
#[test]
fn a_small_pure_function_is_inlined() {
    let files = [(
        "main.emel",
        "\
fn double(x: Int) -> Int {
  x * 2
}

fn main() {
  dbg(double(21))
}
",
    )];
    let (m, stats) = lower_files_with_stats(&files, true);
    assert_eq!(stats.delta, 1, "{stats:?}");
    let double = function(&m, "double");
    assert_eq!(calls_anywhere(&m, double), 0, "{}", dump(&m));
    let ir = dump(&m);
    assert!(ir.contains("*[Int] 2"), "{ir}");
}

/// A self tail call becomes a `Loop` before this pass, so the function is not in a cycle. It is
/// still not copied: a call site would duplicate the loop.
#[test]
fn a_loopified_function_is_not_inlined() {
    let files = [(
        "main.emel",
        "\
fn count(n: Int, acc: Int) -> Int {
  if n == 0 { acc } else { count(n - 1, acc + 1) }
}

fn main() {
  dbg(count(3, 0))
}
",
    )];
    let (m, _) = lower_files_with_stats(&files, true);
    let main = function(&m, "main");
    assert_eq!(calls_in(&m, main, function(&m, "count")), 1, "{}", dump(&m));
}

/// The arguments are bound to fresh locals left to right, once each, before the body runs. The
/// callee ignores nothing here, so an implementation that substitutes by name would evaluate the
/// calls twice and in another order.
#[test]
fn arguments_are_bound_once_in_order() {
    let files = [(
        "main.emel",
        "\
fn add(a: Int, b: Int) -> Int {
  a + b
}

fn shout(x: Int) -> Int {
  dbg(x)
  x
}

fn main() {
  dbg(add(shout(1), shout(2)))
}
",
    )];
    let (m, stats) = lower_files_with_stats(&files, true);
    assert_eq!(stats.delta, 3, "{stats:?}");
    let ir = dump(&m);
    // Only the body of `main`: the same literals occur in other functions of the module.
    let main = ir
        .split("export fn main")
        .nth(1)
        .expect("main is in the dump");
    let main = main.split("\nfn ").next().unwrap();
    // ζ propagates the `let` bindings δ makes, so the argument is printed where it was written.
    let first = main
        .find("@dbg[Int](1)")
        .unwrap_or_else(|| panic!("the first argument is evaluated:\n{main}"));
    let second = main
        .find("@dbg[Int](2)")
        .unwrap_or_else(|| panic!("the second argument is evaluated:\n{main}"));
    assert!(
        first < second,
        "the arguments are evaluated left to right:\n{main}"
    );
    assert_eq!(
        main.matches("@dbg[Int](").count(),
        3,
        "each argument is printed once, and the result once:\n{main}"
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

/// A callee whose body prints is still inlined: the print happens where the call was, in the same
/// order.
#[test]
fn an_effectful_callee_is_inlined() {
    let files = [(
        "main.emel",
        "\
fn shout(x: Int) -> Int {
  dbg(x)
  x
}

fn main() {
  dbg(shout(7))
}
",
    )];
    let (m, stats) = lower_files_with_stats(&files, true);
    assert_eq!(stats.delta, 1, "{stats:?}");
    assert_eq!(calls_anywhere(&m, function(&m, "shout")), 0);
}

/// A failing function is not copied: its failure edge must stay a call.
#[test]
fn a_failing_function_is_not_inlined() {
    let files = [(
        "main.emel",
        "\
error Stop

fn positive(x: Int) -> Int fails Stop {
  if x > 0 { x } else { fail Stop }
}

fn main() {
  dbg(positive(1) escape { Stop -> -1 })
}
",
    )];
    let (m, _) = lower_files_with_stats(&files, true);
    let positive = function(&m, "positive");
    assert_eq!(calls_anywhere(&m, positive), 1, "{}", dump(&m));
}

/// A non-tail self call keeps the function recursive, so it is not copied.
#[test]
fn a_recursive_function_is_not_inlined() {
    let files = [(
        "main.emel",
        "\
fn fact(n: Int) -> Int {
  if n == 0 { 1 } else { n * fact(n - 1) }
}

fn main() {
  dbg(fact(5))
}
",
    )];
    let (m, _) = lower_files_with_stats(&files, true);
    let main = function(&m, "main");
    assert_eq!(calls_in(&m, main, function(&m, "fact")), 1, "{}", dump(&m));
}

/// Mutual recursion is a cycle too: copying either function would expand forever.
#[test]
fn mutually_recursive_functions_are_not_inlined() {
    let files = [(
        "main.emel",
        "\
fn even(n: Int) -> Bool {
  if n == 0 { True } else { odd(n - 1) }
}

fn odd(n: Int) -> Bool {
  if n == 0 { False } else { even(n - 1) }
}

fn main() {
  dbg(even(4))
  dbg(odd(4))
}
",
    )];
    let (m, _) = lower_files_with_stats(&files, true);
    let main = function(&m, "main");
    assert_eq!(calls_in(&m, main, function(&m, "even")), 1, "{}", dump(&m));
    assert_eq!(calls_in(&m, main, function(&m, "odd")), 1, "{}", dump(&m));
}

/// A body over the size limit is left as a call, so the pass does not grow the output.
#[test]
fn a_large_function_is_not_inlined() {
    let files = [(
        "main.emel",
        "\
fn wide(a: Int) -> Int {
  a + a + a + a + a + a + a + a + a + a + a + a + a + a + a + a + a + a
}

fn main() {
  dbg(wide(1))
}
",
    )];
    let (m, _) = lower_files_with_stats(&files, true);
    assert_eq!(calls_anywhere(&m, function(&m, "wide")), 1, "{}", dump(&m));
}

/// A generic callee is copied with its type arguments substituted in the locals, lambdas and
/// nested calls. Lowering verifies the module, so a missed substitution would fail there.
#[test]
fn a_generic_function_is_inlined_with_its_types() {
    let files = [(
        "main.emel",
        "\
fn pair[A](x: A) -> (A, A) {
  (x, x)
}

fn main() {
  dbg(pair(1))
  dbg(pair(\"a\"))
}
",
    )];
    let (m, stats) = lower_files_with_stats(&files, true);
    assert_eq!(calls_anywhere(&m, function(&m, "pair")), 0);
    assert!(stats.delta >= 2, "{stats:?}");
    let ir = dump(&m);
    assert!(ir.contains(": Int"), "{ir}");
    assert!(ir.contains(": String"), "{ir}");
}

/// The bundled collection functions are matched by name on the source (`List.fold(List.range)`)
/// and matched by identity here; inlining them would dissolve the calls T12 looks for and grow
/// every user function that maps a list.
#[test]
fn collection_functions_are_not_inlined() {
    let files = [(
        "main.emel",
        "\
fn main() {
  dbg(List.reverse([1, 2, 3]))
}
",
    )];
    let (m, _) = lower_files_with_stats(&files, true);
    let reverse = function(&m, "List.reverse");
    assert_eq!(
        calls_in(&m, function(&m, "main"), reverse),
        1,
        "{}",
        dump(&m)
    );
}

/// `--no-opt` leaves every call in place.
#[test]
fn optimize_off_keeps_the_calls() {
    let files = [(
        "main.emel",
        "\
fn double(x: Int) -> Int {
  x * 2
}

fn main() {
  dbg(double(21))
}
",
    )];
    let m = lower_files_with(&files, false);
    assert_eq!(calls_anywhere(&m, function(&m, "double")), 1);
}

/// Re-running the pipeline after lowering with the passes off inlines the same calls once: the
/// second run changes nothing, whichever order the functions are visited in.
#[test]
fn a_second_run_changes_nothing() {
    let files = [(
        "main.emel",
        "\
fn double(x: Int) -> Int {
  x * 2
}

fn shout(x: Int) -> Int {
  dbg(x)
  x
}

fn main() {
  dbg(double(shout(21)))
}
",
    )];
    let mut m = lower_files_with(&files, false);
    let ids = CoreFns::default();
    let first = opt::run(&mut m, &ids, true);
    assert!(first.delta > 0, "{first:?}");
    let once = dump(&m);
    let second = opt::run(&mut m, &ids, true);
    assert!(second.is_empty(), "a second run changed the IR: {second:?}");
    assert_eq!(dump(&m), once);
    verify(&m).unwrap();
}
