//! Tests for the η reduction pass (`emela_core::opt::reduce::eta`).
//!
//! η fires on `fn(x…) { f(x…) }` where the arguments are exactly the parameters in order and the
//! lambda's type is exactly the value's type. The tests that expect no reduction are the
//! interesting half: a dropped, reordered or captured argument, an evidence table, a suspension,
//! and a type that only matches up to the verifier's compatibility must all leave the wrapper.
//!
//! The observable behaviour of the reduction is checked by the mutation pair
//! `eta_fused.emel` / `eta_expanded.emel` in `emela-driver/tests/opt_diff.rs`; the tests here fix
//! the shape the pass leaves behind. Each test turns on only η, so a reduction another rule could
//! make cannot hide η's decision.

use crate::support;

use emela_core::opt::{CoreFn, CoreFns, Options, Rules, run_with};
use emela_core::{Callee, Expr, FnId, Module, dump, verify};
use support::{lower_files, lower_files_with_rules};

/// Runs the pipeline on the module lowering produces, with only η on.
fn eta_only() -> Options {
    Options {
        rules: Rules {
            eta: true,
            ..Rules::none()
        },
        ..Options::default()
    }
}

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

/// The dump of the body of `name`, from its header to the next blank line.
fn body_dump<'a>(ir: &'a str, name: &str) -> &'a str {
    let header = format!("fn {name}(");
    let start = ir
        .find(&header)
        .unwrap_or_else(|| panic!("no `{header}` in:\n{ir}"));
    let rest = &ir[start..];
    &rest[..rest.find("\n\n").unwrap_or(rest.len())]
}

/// The number of lambda nodes in `e`.
fn lambdas(e: &Expr) -> usize {
    let mut count = usize::from(matches!(e, Expr::Lambda { .. }));
    e.for_each_child(&mut |c| count += lambdas(c));
    count
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

/// The body of `main`.
fn main_body(m: &Module) -> &Expr {
    &m.functions[function(m, "main")].body
}

/// The bundled core functions of `m`, by the IR names lowering gives them.
///
/// The test helper runs the pipeline with an empty [`CoreFns`], because a rule test does not
/// depend on a fusion. The ordering test does: it needs the range fusion to recognise `List.fold`
/// and `List.range`, which lowering records in a map it does not expose.
fn core_fns(m: &Module) -> CoreFns {
    m.functions
        .iter()
        .filter_map(|(id, f)| {
            let name = f.name.split_once('$').map_or(f.name.as_str(), |(n, _)| n);
            let kind = match name {
                "List.fold" => CoreFn::ListFold,
                "List.map" => CoreFn::ListMap,
                "List.range" => CoreFn::ListRange,
                "List.reverse" => CoreFn::ListReverse,
                _ => return None,
            };
            Some((id, kind))
        })
        .collect()
}

/// `List.map(xs, |x| Show.show(x))` becomes `List.map(xs, Show.show)`: the closure is gone and the
/// function value is passed where it was.
#[test]
fn a_call_wrapper_becomes_the_function_value() {
    let files = [(
        "main.emel",
        "\
fn main() {
  xs = [1, 2, 3]
  dbg(List.map(xs, |x| Show.show(x)))
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, eta_only().rules);
    assert!(stats.eta > 0, "{stats:?}");
    let ir = dump(&m);
    let main = body_dump(&ir, "main");
    assert!(!main.contains("fn("), "the wrapper is gone:\n{main}");
    assert!(main.contains("Show.show"), "the value is passed:\n{main}");
}

/// The wrapper does not pass its parameter, so it is not the identity on the arguments. It is
/// called twice, so that β leaves the binding to η and the dump shows η's decision.
#[test]
fn a_dropped_argument_is_not_reduced() {
    let files = [(
        "main.emel",
        "\
fn zero() -> Int {
  1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1
}

fn main() {
  g = |x: Int| zero()
  dbg(g(0))
  dbg(g(1))
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, eta_only().rules);
    assert_eq!(stats.eta, 0, "{stats:?}");
    assert!(body_dump(&dump(&m), "main").contains("fn("), "{}", dump(&m));
}

/// The wrapper reorders its parameters, so passing the function value would swap the arguments of
/// every call. It is called twice, so that β leaves the binding to η.
#[test]
fn arguments_in_another_order_are_not_reduced() {
    let files = [(
        "main.emel",
        "\
fn sub(a: Int, b: Int) -> Int {
  a - b + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1
}

fn main() {
  g = |a: Int, b: Int| sub(b, a)
  dbg(g(3, 1))
  dbg(g(4, 2))
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, eta_only().rules);
    assert_eq!(stats.eta, 0, "{stats:?}");
    assert!(body_dump(&dump(&m), "main").contains("fn("), "{}", dump(&m));
}

/// The wrapper passes more arguments than it takes parameters. It is called twice, so that β
/// leaves the binding to η.
#[test]
fn extra_arguments_are_not_reduced() {
    let files = [(
        "main.emel",
        "\
fn sub(a: Int, b: Int) -> Int {
  a - b + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1
}

fn main() {
  g = |x: Int| sub(x, x)
  dbg(g(3))
  dbg(g(4))
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, eta_only().rules);
    assert_eq!(stats.eta, 0, "{stats:?}");
    assert!(body_dump(&dump(&m), "main").contains("fn("), "{}", dump(&m));
}

/// The wrapper passes a captured local instead of one of its parameters. It is called twice, so
/// that β leaves the binding to η.
#[test]
fn a_captured_value_is_not_reduced() {
    let files = [(
        "main.emel",
        "\
fn sub(a: Int, b: Int) -> Int {
  a - b + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1
}

fn main() {
  k = 10
  g = |a: Int, b: Int| sub(a, k)
  dbg(g(3, 1))
  dbg(g(4, 2))
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, eta_only().rules);
    assert_eq!(stats.eta, 0, "{stats:?}");
    assert!(body_dump(&dump(&m), "main").contains("fn("), "{}", dump(&m));
}

/// A body that is not the call is not a wrapper, whatever it captures. The function value is
/// called twice, so that β leaves the binding to η.
#[test]
fn a_body_that_is_not_the_call_is_not_reduced() {
    let files = [(
        "main.emel",
        "\
fn main() {
  k = 1
  g = |x: Int| x + k
  dbg(g(2))
  dbg(g(3))
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, eta_only().rules);
    assert_eq!(stats.eta, 0, "{stats:?}");
    assert!(body_dump(&dump(&m), "main").contains("fn("), "{}", dump(&m));
}

/// A lambda takes the evidence table as a last parameter and passes it on to a callee whose R
/// needs it: the wrapper is the callee's function value.
#[test]
fn a_lambda_that_forwards_the_evidence_table_is_reduced() {
    let files = [(
        "main.emel",
        "\
effect Log { fn info(msg: String) -> () }

handler ConsoleLog: Log {
  fn info(msg) {
    dbg(msg)
    ()
  }
}

fn shout(x: Int) -> Int uses { Log } {
  (use Log).info(\"shout\")
  x
}

fn main() {
  with ConsoleLog {
    g = |x: Int| shout(x)
    dbg(g(1))
  }
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, eta_only().rules);
    assert!(stats.eta > 0, "{stats:?}");
    assert!(
        !body_dump(&dump(&m), "main").contains("= fn("),
        "{}",
        dump(&m)
    );
}

/// A suspending lambda is a suspension point of its own; its value is not the callee's.
#[test]
fn a_suspending_lambda_is_not_reduced() {
    let files = [(
        "main.emel",
        "\
fn wait(x: Int) -> Int uses { Async } {
  x
}

fn main() -> Int uses { Async } {
  g = |x: Int| wait(x)
  dbg(g(1))
  0
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, eta_only().rules);
    assert_eq!(stats.eta, 0, "{stats:?}");
    assert!(body_dump(&dump(&m), "main").contains("fn("), "{}", dump(&m));
}

/// The verifier accepts a lambda whose parameter type is narrower than the callee's (`Never` flows
/// into anything); the value's type decides the calling convention, so the wrapper stays. The
/// value is called twice, so that β leaves the binding to η.
#[test]
fn a_type_that_only_matches_up_to_compatibility_is_not_reduced() {
    let files = [(
        "main.emel",
        "\
fn one(x: Int) -> Int {
  x + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1
}

fn main() -> Int {
  g = |x: Never| one(x)
  g(panic(\"never\"))
  g(panic(\"never\"))
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, eta_only().rules);
    assert_eq!(stats.eta, 0, "{stats:?}");
    assert!(body_dump(&dump(&m), "main").contains("fn("), "{}", dump(&m));
}

/// A call through a local function value is the same as that value: the lambda goes, the local
/// stays. The wrapper is called twice, so that β leaves the binding to η.
#[test]
fn a_call_through_a_local_becomes_the_local() {
    let files = [(
        "main.emel",
        "\
fn main() {
  g: fn(Int) -> String = Show.show
  h = |x: Int| g(x)
  dbg(h(1))
  dbg(h(2))
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, eta_only().rules);
    assert!(stats.eta > 0, "{stats:?}");
    let ir = dump(&m);
    let main = body_dump(&ir, "main");
    assert_eq!(lambdas(main_body(&m)), 0, "the wrapper is gone:\n{main}");
    // `let h = g` stays: ζ is off in this test, so the calls still go through `h`.
    assert!(
        main.contains(" = g%"),
        "the local replaces the lambda:\n{main}"
    );
}

/// The range fusion runs before η and copies the lambda into the loop. η must not have taken it
/// first, or the loop's step would be an indirect call through a closure.
#[test]
fn the_range_fusion_copies_its_lambda_before_eta() {
    let files = [(
        "main.emel",
        "\
fn add(a: Int, b: Int) -> Int {
  a + b + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1
}

fn main() {
  dbg(List.fold(List.range(0, 4), 0, |a, b| add(a, b)))
}
",
    )];
    // The fusion needs lowering's record of the bundled core functions, which the helper does not
    // expose, so this test runs the pipeline itself.
    let mut m = lower_files(&files);
    let core_fns = core_fns(&m);
    let stats = run_with(&mut m, &core_fns, eta_only());
    assert_eq!(stats.eta, 0, "{stats:?}");
    assert!(stats.range_fold > 0, "{stats:?}");
    let main = main_body(&m);
    assert_eq!(lambdas(main), 0, "{}", dump(&m));
    assert_eq!(
        direct_calls(main, function(&m, "add")),
        1,
        "the loop's step is the copied call:\n{}",
        dump(&m)
    );
}

/// `Rules::none()` leaves every wrapper in place and does not run η.
#[test]
fn optimize_off_keeps_the_wrappers() {
    let files = [(
        "main.emel",
        "\
fn main() {
  xs = [1, 2, 3]
  dbg(List.map(xs, |x| Show.show(x)))
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, Rules::none());
    assert_eq!(stats.eta, 0, "{stats:?}");
    assert_eq!(lambdas(main_body(&m)), 1, "{}", dump(&m));
}

/// A second run of the pipeline after lowering with the passes off finds no wrapper: the first
/// run removed it and the replacement is not one.
#[test]
fn a_second_run_changes_nothing() {
    let files = [(
        "main.emel",
        "\
fn main() {
  xs = [1, 2, 3]
  dbg(List.map(xs, |x| Show.show(x)))
}
",
    )];
    let mut m = lower_files(&files);
    let first = run_with(&mut m, &CoreFns::default(), eta_only());
    assert!(first.eta > 0, "{first:?}");
    let once = dump(&m);
    let second = run_with(&mut m, &CoreFns::default(), eta_only());
    assert_eq!(second.eta, 0, "a second run reduced again: {second:?}");
    assert!(second.is_empty(), "a second run changed the IR: {second:?}");
    assert_eq!(dump(&m), once);
    verify(&m).unwrap();
}
