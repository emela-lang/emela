//! Tests for the Core IR optimisation pipeline (`emela_core::opt`).
//!
//! The passes themselves have their own tests next to them. These tests fix the properties of
//! the pipeline as a whole: running it twice changes nothing, the same source gives the same IR,
//! `--no-opt` skips every pass, and the Option fusions fire exactly where they should.

mod support;

use emela_core::opt::{CoreFn, CoreFns};
use emela_core::{dump, opt};
use support::{lower_files_with, lower_files_with_stats};

/// A program that each of the planned passes has something to do on (Option fusion, range
/// fusion, inlining), so that the idempotence test keeps its meaning once they are added.
const PROGRAM: &[(&str, &str)] = &[(
    "main.emel",
    "\
import Std.Array.{Array}

fn main() {
  xs = Array.from_list([1, 2, 3])
  dbg(Array.get(xs, 1) |> Option.or(0))
  dbg(Array.get(xs, 9) match {
    Some(x) -> x
    None -> -1
  })
  dbg(List.fold(List.range(0, 3), 0, |acc, x| acc + x))
  dbg(List.map(List.range(0, 3), |x| x * 2))
}
",
)];

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
                "List.range" => CoreFn::ListRange,
                _ => return None,
            };
            Some((id, kind))
        })
        .collect()
}

/// A second run of the pipeline is a no-op and leaves the IR as the first run left it. A pass
/// that is not idempotent must be fixed, not this test: passes are applied until they agree with
/// themselves.
#[test]
fn a_second_run_is_a_no_op() {
    let mut m = lower_files_with(PROGRAM, true);
    let mut ids: CoreFns = Default::default();
    for (id, kind) in core_fns(&m) {
        ids.insert(id, kind);
    }
    let first = opt::run(&mut m, &ids, true);
    let once = dump(&m);
    let second = opt::run(&mut m, &ids, true);
    assert!(
        first.is_empty(),
        "the program was already optimised by lowering: {first:?}"
    );
    assert!(second.is_empty(), "a second run changed the IR: {second:?}");
    assert_eq!(dump(&m), once);
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
    let ir = dump(&m);
    assert!(ir.contains("@$Array.get_or"), "{ir}");
    assert!(ir.contains("@$Array.load_unchecked"), "{ir}");
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
