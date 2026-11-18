//! Tests for the Core IR optimisation pipeline (`emela_core::opt`).
//!
//! The passes themselves have their own tests next to them. These tests fix the properties of
//! the pipeline as a whole: the first run reaches the fixed point (a second run changes nothing),
//! the same source gives the same IR, `--no-opt` skips every pass, and the Option fusions fire
//! exactly where they should.

use crate::support;

use emela_core::opt::{CoreFn, CoreFns, Rules};
use emela_core::{dump, opt};
use support::{lower_files_with, lower_files_with_rules, lower_files_with_stats};

/// A program that each of the planned passes has something to do on (Option fusion, range
/// fusion, inlining), so that the idempotence test keeps its meaning once they are added.
const PROGRAM: &[(&str, &str)] = &[(
    "main.emel",
    "\
import Std.Array.{Array}

fn inc(x: Int) -> Int {
  x + 1
}

fn main() {
  xs = Array.from_list([1, 2, 3])
  dbg(Array.get(xs, 1) |> Option.or(0))
  dbg(Array.get(xs, 9) match {
    Some(x) -> x
    None -> -1
  })
  dbg(inc(List.fold(List.range(0, 3), 0, |acc, x| acc + x)))
  dbg(List.map(List.range(0, 3), |x| inc(x * 2)))
  dbg(Array.size(Array.from_list(List.map(List.range(0, 3), |x| inc(x)))))
}
",
)];

/// Programs whose fixed point the first sweep cannot reach: the fusion pattern is made by the
/// body of the callee, and only inlining that body into the caller puts the pattern where the
/// fusion looks. One sweep of the pipeline is not enough; the whole table has to run again.
///
/// The pattern cannot come from a call argument: inlining binds each argument to a fresh local
/// before the body runs, so `g(List.range(0, 3))` becomes `let xs = List.range(0, 3); …` and the
/// range fusion, which matches a direct `List.range` call, does not see it on any sweep.
const AFTER_INLINE: &[&[(&str, &str)]] = &[
    // `span()` is `List.range(0, 3)`, so `List.fold(span(), …)` becomes
    // `List.fold(List.range(0, 3), …)`, which the range fusion rewrites to a loop on the second
    // sweep.
    &[(
        "main.emel",
        "\
fn span() -> List[Int] {
  List.range(0, 3)
}

fn main() {
  dbg(List.fold(span(), 0, |a, b| a + b))
}
",
    )],
    // `first()` is `Array.get(Array.from_list([1]), 0)`, so `Option.or(first(), 0)` becomes
    // `Option.or(Array.get(…), 0)`, which the Option fusion rewrites to `ArrayGetOr` on the
    // second sweep.
    &[(
        "main.emel",
        "\
import Std.Array.{Array}

fn first() -> Option[Int] {
  Array.get(Array.from_list([1]), 0)
}

fn main() {
  dbg(Option.or(first(), 0))
}
",
    )],
];

/// Rebuilds the `core_fns` map that lowering passes to `run`, by IR name. `Lowered` does not
/// carry the map (it belongs to lowering), and the integration test cannot see lowering's own.
fn core_fns(m: &emela_core::Module) -> Vec<(emela_core::FnId, CoreFn)> {
    m.functions
        .iter()
        .filter_map(|(id, f)| {
            let name = f.name.split_once('$').map_or(f.name.as_str(), |(n, _)| n);
            let kind = match name {
                "Option.or" => CoreFn::OptionOr,
                "List.fold" => CoreFn::ListFold,
                "List.map" => CoreFn::ListMap,
                "List.range" => CoreFn::ListRange,
                "List.reverse" => CoreFn::ListReverse,
                _ if name.starts_with("List.") => CoreFn::Collection,
                _ => return None,
            };
            Some((id, kind))
        })
        .collect()
}

/// Lowers with the passes off, then runs the pipeline with `rules` and the implemented fusion laws.
///
/// A test that needs an earlier reduction to expose the shape the pass under test matches uses
/// this: lowering's `core_fns` map is not exposed, so it is rebuilt here for the fusions.
fn lower_and_run(
    files: &[(&str, &str)],
    rules: Rules,
) -> (emela_core::Module, emela_core::opt::Stats) {
    let mut m = lower_files_with(files, false);
    let mut ids: CoreFns = Default::default();
    for (id, kind) in core_fns(&m) {
        ids.insert(id, kind);
    }
    let stats = opt::run_with(
        &mut m,
        &ids,
        emela_core::opt::Options {
            rules,
            ..emela_core::opt::Options::default()
        },
    );
    (m, stats)
}

/// The first run reaches the fixed point: a second run changes nothing and leaves the IR as the
/// first run left it. A pass that is not idempotent must be fixed, not this test: the pipeline
/// repeats the whole table until a sweep changes nothing.
///
/// A program whose first sweep is enough reports `passes == 1`; the programs whose fusion pattern
/// only appears once inlining has run need a second sweep, so `passes == 2`. A run on a module
/// that is already at the fixed point changes nothing and reports `passes == 1`.
#[test]
fn a_second_run_is_a_no_op() {
    let cases = [
        ("program", PROGRAM, 1),
        ("range after inline", AFTER_INLINE[0], 2),
        ("option after inline", AFTER_INLINE[1], 2),
    ];
    for (name, files, passes) in cases {
        // Lowered with the passes off, so the first `run` has all the work to do.
        let mut m = lower_files_with(files, false);
        let mut ids: CoreFns = Default::default();
        for (id, kind) in core_fns(&m) {
            ids.insert(id, kind);
        }
        let first = opt::run(&mut m, &ids, true);
        assert!(
            !first.is_empty(),
            "{name}: the first run changed nothing: {first:?}"
        );
        assert_eq!(first.passes, passes, "{name}: {first:?}");
        let once = dump(&m);
        let second = opt::run(&mut m, &ids, true);
        assert!(
            second.is_empty(),
            "{name}: a second run changed the IR: {second:?}"
        );
        assert_eq!(
            second.passes, 1,
            "{name}: a run at the fixed point takes one sweep: {second:?}"
        );
        assert_eq!(dump(&m), once, "{name}");
    }
}

/// The same source lowers to the same IR every time. A pass must not let hash iteration order or
/// fresh ID allocation decide anything.
#[test]
fn lowering_is_deterministic() {
    assert_eq!(
        dump(&lower_files_with(PROGRAM, true)),
        dump(&lower_files_with(PROGRAM, true))
    );
}

/// With the switch off (`--no-opt`) the passes do not run: the module is unchanged and `Stats` is
/// empty, whatever the passes would have done.
#[test]
fn optimize_false_changes_nothing() {
    let mut m = lower_files_with(PROGRAM, false);
    let mut ids: CoreFns = Default::default();
    for (id, kind) in core_fns(&m) {
        ids.insert(id, kind);
    }
    let before = dump(&m);
    let stats = opt::run(&mut m, &ids, false);
    assert!(stats.is_empty(), "{stats:?}");
    assert_eq!(dump(&m), before);
}

/// The fusions fire on the shapes they are for, and the dump shows the internal builtins and the
/// loop the range becomes.
#[test]
fn fusions_fire() {
    let (m, stats) = lower_files_with_stats(PROGRAM, true);
    assert_eq!(stats.option_or, 1);
    assert_eq!(stats.array_get_match, 1);
    assert_eq!(stats.range_fold, 1);
    assert_eq!(stats.range_map, 1);
    assert_eq!(stats.map_range, 1);
    let ir = dump(&m);
    assert!(ir.contains("@$Array.get_or"), "{ir}");
    assert!(ir.contains("@$Array.load_unchecked"), "{ir}");
    assert!(ir.contains("@$Array.map_range"), "{ir}");
    assert!(ir.contains("loop("), "{ir}");
}

/// A user's `fold`, `map` or `range` has the same name and shape as the core ones, but is a
/// different function: the fusion matches the identity lowering recorded, not the name.
#[test]
fn a_users_or_is_not_fused() {
    let files = [(
        "main.emel",
        "\
import Std.Array.{Array}

fn or(opt: Option[Int], default: Int) -> Int {
  opt match {
    Some(x) -> x
    None -> default
  }
}

fn main() {
  dbg(or(Array.get(Array.from_list([1, 2]), 0), 2))
}
",
    )];
    let (_, stats) = lower_files_with_stats(&files, true);
    assert_eq!(stats.option_or, 0, "{stats:?}");
}

/// The same for a user's `fold`, `map` and `range`.
#[test]
fn a_users_list_functions_are_not_fused() {
    let files = [(
        "main.emel",
        "\
fn range(from: Int, to: Int) -> List[Int] {
  if to <= from { [] } else { [to - 1, ..range(from, to - 1)] }
}

fn fold(xs: List[Int], from: Int, f: fn(Int, Int) -> Int) -> Int {
  xs match {
    [] -> from
    [x, ..rest] -> fold(rest, f(from, x), f)
  }
}

fn map(xs: List[Int], f: fn(Int) -> Int) -> List[Int] {
  xs match {
    [] -> []
    [x, ..rest] -> [f(x), ..map(rest, f)]
  }
}

fn main() {
  dbg(fold(range(0, 3), 0, |acc, x| acc + x))
  dbg(map(range(0, 3), |x| x * 2))
}
",
    )];
    let (_, stats) = lower_files_with_stats(&files, true);
    assert_eq!(stats.range_fold, 0, "{stats:?}");
    assert_eq!(stats.range_map, 0, "{stats:?}");
}

/// `()` and `Never` have no representation, so an `Array` of them is not fused (`ArrayGetOr`
/// would have to pass a value of no representation).
#[test]
fn unit_elements_are_not_fused() {
    let files = [(
        "main.emel",
        "\
import Std.Array.{Array}

fn main() {
  xs = Array.from_list([(), ()])
  dbg(Array.get(xs, 0) |> Option.or(()))
  dbg(Array.get(xs, 0) match {
    Some(_) -> 1
    None -> 0
  })
}
",
    )];
    let (_, stats) = lower_files_with_stats(&files, true);
    assert_eq!(stats.option_or, 0, "{stats:?}");
    assert_eq!(stats.array_get_match, 0, "{stats:?}");
}

/// `Array.from_list(List.map(List.range(lo, hi), f))` becomes the internal `ArrayMapRange`, whose
/// type arguments are the element type and the type of `f`. A boundary range (negative `lo`, and
/// `hi` not above `lo`) and an element type that is not `Int` are fused all the same.
#[test]
fn array_map_range_is_fused() {
    let files = [(
        "main.emel",
        "\
import Std.Array.{Array}

fn doubled(n: Int) -> Array[Int] {
  Array.from_list(List.map(List.range(0, n), |i| i * 2))
}

fn main() {
  dbg(Array.to_list(Array.from_list(List.map(List.range(-2, 2), |i| Int.to_string(i)))))
  dbg(Array.to_list(Array.from_list(List.map(List.range(3, 3), |i| i * 2))))
  dbg(Array.to_list(Array.from_list(List.map(List.range(5, 2), |i| i * 2))))
  dbg(Array.to_list(doubled(2)))
}
",
    )];
    let (m, stats) = lower_files_with_stats(&files, true);
    assert_eq!(stats.map_range, 4, "{stats:?}");
    let ir = dump(&m);
    assert!(ir.contains("@$Array.map_range"), "{ir}");
}

/// The builtin has no failure edge and takes no evidence table, so an `f` that may fail or use an
/// effect is left to the list form, which keeps it.
#[test]
fn array_map_range_needs_a_pure_f() {
    let failing = [(
        "main.emel",
        "\
import Std.Array.{Array}

error Boom

fn failing(x: Int) -> Int fails Boom {
  x
}

fn main() {
  dbg(Array.size(Array.from_list(List.map(List.range(0, 3), failing))) escape { Boom -> 0 })
}
",
    )];
    let (_, stats) = lower_files_with_stats(&failing, true);
    assert_eq!(stats.map_range, 0, "{stats:?}");

    let effectful = [(
        "main.emel",
        "\
import Std.Array.{Array}

effect Log {
  fn log(msg: String) -> ()
}

handler Ignore: Log {
  fn log(msg) { () }
}

fn logging(x: Int) -> Int uses { Log } {
  (use Log).log(\"at #{x}\")
  x
}

fn main() {
  with Ignore {
    dbg(Array.size(Array.from_list(List.map(List.range(0, 3), logging))))
  }
}
",
    )];
    let (_, stats) = lower_files_with_stats(&effectful, true);
    assert_eq!(stats.map_range, 0, "{stats:?}");
}

/// `()` has no representation, so `Array[()]` has none either and the builtin (whose elements the
/// loop stores) is not made.
#[test]
fn array_map_range_of_unit_is_not_fused() {
    let files = [(
        "main.emel",
        "\
import Std.Array.{Array}

fn main() {
  dbg(Array.size(Array.from_list(List.map(List.range(0, 3), |i| ()))))
}
",
    )];
    let (_, stats) = lower_files_with_stats(&files, true);
    assert_eq!(stats.map_range, 0, "{stats:?}");
}

/// A user's `from_list` or `map` and a `let`-bound range are not the shape: the fusion matches the
/// bundled `Array.from_list` builtin and a direct `List.range` call.
#[test]
fn a_users_from_list_and_map_are_not_map_range() {
    let files = [(
        "main.emel",
        "\
import Std.Array.{Array}

fn from_list(xs: List[Int]) -> List[Int] {
  xs
}

fn map(xs: List[Int], f: fn(Int) -> Int) -> List[Int] {
  xs
}

fn main() {
  dbg(from_list(map([1, 2], |x| x)))
  xs = List.range(0, 3)
  dbg(Array.size(Array.from_list(List.map(xs, |x| x * 2))))
}
",
    )];
    let (_, stats) = lower_files_with_stats(&files, true);
    assert_eq!(stats.map_range, 0, "{stats:?}");
}

/// The passes run before monomorphisation, so the element type of a generic `Array[A]` is still
/// open when they see it: the instantiation may be `()`. The fused builtins produce the element,
/// so they need a representation (`ArrayGetOr` would carry a value without one, and
/// `ArrayLoadUnchecked` would have no destination in the Flow IR). Neither shape is fused in a
/// generic function, whatever it is instantiated with.
#[test]
fn a_generic_element_is_not_fused() {
    let files = [(
        "main.emel",
        "\
import Std.Array.{Array}

fn or_default[A](a: Array[A], i: Int, d: A) -> A {
  Array.get(a, i) |> Option.or(d)
}

fn each[A](a: Array[A]) -> () {
  Array.get(a, 0) match {
    Some(x) -> { dbg(x)
      () }
    None -> ()
  }
}

fn main() {
  dbg(or_default(Array.from_list([(), ()]), 0, ()))
  each(Array.from_list([(), ()]))
}
",
    )];
    let (m, stats) = lower_files_with_stats(&files, true);
    assert_eq!(stats.option_or, 0, "{stats:?}");
    assert_eq!(stats.array_get_match, 0, "{stats:?}");
    let ir = dump(&m);
    assert!(!ir.contains("@$Array.get_or"), "{ir}");
    assert!(!ir.contains("@$Array.load_unchecked"), "{ir}");
}

/// A lambda handed to a fused `List.fold`/`List.map` over a range is copied into the loop rather
/// than called through the closure it would be. The step is then straight-line code of the
/// enclosing function, which is what lets the backends inline and hoist the array reads in it, and
/// the closure (an allocation, plus a reference for each capture) is not made at all.
#[test]
fn a_range_loop_copies_in_its_lambda() {
    let files = [(
        "main.emel",
        "\
fn main() {
  dbg(List.fold(List.range(0, 3), 0, |acc, x| acc + x))
  dbg(List.map(List.range(0, 3), |x| x * 2))
}
",
    )];
    let (m, stats) = lower_files_with_stats(&files, true);
    assert_eq!(stats.range_fold, 1, "{stats:?}");
    assert_eq!(stats.range_map, 1, "{stats:?}");
    let ir = dump(&m);
    let main = ir
        .split("export fn main")
        .nth(1)
        .expect("main is in the dump");
    let main = main.split("\nfn ").next().unwrap();
    assert!(main.contains("loop("), "{main}");
    // The dump writes a lambda as `fn(x%0: Int) -> Int {` and an indirect call as `(f%1)(...)`.
    assert!(
        !main.contains("fn("),
        "the lambda was not copied in: {main}"
    );
    assert!(!main.contains("(f%"), "the call is still indirect: {main}");
}

/// The bounds test and the load of a fused `Array.get` whose array and index do not change in the
/// loop leave the loop: the test becomes one `if` around two copies of the loop, and the row is
/// loaded once instead of once per element. Nothing else about the loop's result changes.
#[test]
fn a_bounds_test_of_a_loop_leaves_it() {
    let files = [(
        "main.emel",
        "\
import Std.Array.{Array}

fn get(m: Array[Array[Int]], i: Int, j: Int) -> Int {
  Array.get(m, i) match {
    Some(row) -> Array.get(row, j) |> Option.or(0)
    None -> 0
  }
}

fn dot(a: Array[Array[Int]], b: Array[Array[Int]], i: Int, j: Int, n: Int) -> Int {
  List.fold(List.range(0, n), from: 0, f: |acc, k| acc + get(a, i, k) * get(b, k, j))
}

fn main() {
  a = Array.from_list([Array.from_list([1, 2]), Array.from_list([3, 4])])
  dbg(dot(a, a, 0, 1, 2))
}
",
    )];
    // ζ removes the `let` copies inlining leaves in front of the work; the unswitch then sees the
    // array and the index as parameters bound outside the loop.
    let (m, stats) = lower_and_run(
        &files,
        Rules {
            zeta: true,
            ..Rules::none()
        },
    );
    assert!(stats.hoist > 0, "{stats:?}");
    let ir = dump(&m);
    let dot = ir.split("fn dot").nth(1).expect("dot is in the dump");
    let dot = dot.split("\nfn ").next().unwrap();
    // `k` is the loop's variable and `i` is fixed, so the test of the row of `a` runs once and
    // the loop is copied into its two arms. The row is loaded before the first loop; the second
    // get depends on `k` and keeps its test in the loop.
    let loop_at = dot.find("loop(").expect("dot has a loop");
    let head = &dot[..loop_at];
    assert!(
        head.contains("Array.size") && head.contains("load_unchecked"),
        "{dot}"
    );
    assert_eq!(dot.matches("loop(").count(), 2, "{dot}");
}

/// A copy of a name that a pattern binds is ζ reduction's business: `let y = x` is substituted
/// away when ζ is on, wherever the `let` is. The loop pass does not move it (see
/// [`a_loop_invariant_let_is_left_to_zeta`]), and ζ's substitution cannot use `x` before the
/// pattern binds it because the whole `let` is inside the same arm. A recursive function is
/// loopified, so its `match` arms are inside a loop.
#[test]
fn a_copy_of_a_pattern_binding_is_propagated() {
    let files = [(
        "main.emel",
        "\
fn sum(xs: List[Int], acc: Int) -> Int {
  xs match {
    [] -> acc
    [x, ..rest] -> {
      y = x
      sum(rest, acc + y)
    }
  }
}

fn main() {
  dbg(sum([1, 2, 3], 0))
}
",
    )];
    let (m, stats) = lower_files_with_rules(
        &files,
        Rules {
            zeta: true,
            ..Rules::none()
        },
    );
    assert!(stats.zeta > 0, "{stats:?}");
    let ir = dump(&m);
    let sum = ir.split("fn sum").nth(1).expect("sum is in the dump");
    assert!(sum.contains("loop("), "sum is loopified: {sum}");
    // `let y = x` is a copy of the pattern binding, and ζ removed it.
    assert!(!sum.contains("let y"), "{sum}");
}

/// The loop pass is unswitching only: a `let` whose value is a name bound outside the loop stays
/// where it is, and ζ is the rule that propagates it. The A9/PR-018 split removed the let half of
/// the old `hoist`; this test pins that the loop pass does not take it back.
#[test]
fn a_loop_invariant_let_is_left_to_zeta() {
    let files = [(
        "main.emel",
        "\
fn sum(xs: List[Int], acc: Int, base: Int) -> Int {
  xs match {
    [] -> acc
    [x, ..rest] -> {
      z = base
      y = x + z
      sum(rest, acc + y, base)
    }
  }
}

fn main() {
  dbg(sum([1, 2, 3], 0, 10))
}
",
    )];
    // Every reduction rule off: only the ungated loop pass and δ run.
    let (m, stats) = lower_files_with_rules(&files, Rules::none());
    assert_eq!(stats.hoist, 0, "{stats:?}");
    let ir = dump(&m);
    let sum = ir.split("fn sum").nth(1).expect("sum is in the dump");
    let loop_at = sum.find("loop(").expect("sum is loopified");
    // `let z = base` reads a parameter bound outside the loop, but no let is hoisted any more.
    assert!(sum[loop_at..].contains("let z"), "{sum}");
}

/// The same hoist for an array whose elements are records: a fixed position (`Array.get(a, 0)`
/// or a variable bound outside the loop) is tested and loaded once, and the loop's body reads
/// the name it bound. The element representation is the backend's business, so the pass is the
/// same word arrays and by-value arrays.
#[test]
fn a_fixed_record_read_leaves_the_loop() {
    let files = [(
        "main.emel",
        "\
import Std.Array.{Array}

type Point(x: Int, y: Int)

fn sum(points: Array[Point], n: Int) -> Int {
  List.fold(List.range(0, n), from: 0, f: |acc, k| {
    p = Array.get(points, 0) match {
      Some(p) -> p
      None -> Point(x: 0, y: 0)
    }
    q = Array.get(points, k) match {
      Some(q) -> q
      None -> Point(x: 0, y: 0)
    }
    acc + p.x + p.y + q.x + q.y
  })
}

fn main() {
  dbg(sum(Array.from_list([Point(x: 1, y: 2), Point(x: 3, y: 4)]), 2))
}
",
    )];
    // As above, ζ is what exposes the invariant array and index to the unswitch; the let half of
    // the old hoist did it before A9/PR-018.
    let (m, stats) = lower_and_run(
        &files,
        Rules {
            zeta: true,
            ..Rules::none()
        },
    );
    assert!(stats.hoist > 0, "{stats:?}");
    let ir = dump(&m);
    let sum = ir.split("fn sum").nth(1).expect("sum is in the dump");
    let sum = sum.split("\nfn ").next().unwrap();
    // The index is the literal 0, so the test is `(0 >= 0)`; it and its load are before the first
    // loop, and the loop keeps the test of the position that changes with `k`.
    let loop_at = sum.find("loop(").expect("sum has a loop");
    let head = &sum[..loop_at];
    assert!(
        head.contains("Array.size[Point]") && head.contains("load_unchecked[Point]"),
        "{sum}"
    );
    assert_eq!(sum.matches("loop(").count(), 2, "{sum}");
}
