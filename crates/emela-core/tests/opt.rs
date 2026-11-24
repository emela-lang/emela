//! Tests for the Core IR optimisation pipeline (`emela_core::opt`).
//!
//! The passes themselves have their own tests next to them. These tests fix the properties of
//! the pipeline as a whole: running it twice changes nothing, the same source gives the same IR,
//! and `--no-opt` skips every pass.

use crate::support;

use emela_core::{dump, opt};
use support::lower_files_with;

/// A program that each of the planned passes has something to do on (Option fusion, range
/// fusion, inlining), so that the idempotence test keeps its meaning once they are added.
const PROGRAM: &[(&str, &str)] = &[(
    "main.emel",
    "\
import Std.Array.{Array}

fn main() {
  xs = Array.from_list([1, 2, 3])
  dbg(Array.get(xs, 1) |> Option.or(0))
  dbg(List.fold(List.range(0, 3), 0, |acc, x| acc + x))
  dbg(List.map(List.range(0, 3), |x| x * 2))
}
",
)];

/// A second run of the pipeline is a no-op and leaves the IR as the first run left it. A pass
/// that is not idempotent must be fixed, not this test: passes are applied until they agree with
/// themselves.
#[test]
fn a_second_run_is_a_no_op() {
    let mut m = lower_files_with(PROGRAM, true);
    opt::run(&mut m, true);
    let once = dump(&m);
    let second = opt::run(&mut m, true);
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
    let before = dump(&m);
    let stats = opt::run(&mut m, false);
    assert!(stats.is_empty(), "{stats:?}");
    assert_eq!(dump(&m), before);
}
