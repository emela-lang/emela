//! Tests for the A0/PR-002 skeleton: the new rules are off by default and the ones that are
//! still empty change nothing when on.
//!
//! The later rule pull requests turn one flag on at a time. These tests fix the property that
//! makes that safe for parallel integration: a default run is the pipeline from before the
//! reduction kernel, and a run with the rules that are still empty on has the same output. A
//! rule that has landed has its own tests; the comparison here narrows to leave it off.

mod support;

use emela_core::opt::{CoreFn, CoreFns, Options, Rules};
use emela_core::{Module, dump, opt};
use support::lower_files_with;

/// A program the existing passes rewrite (Option fusion, range fusion and inlining), so the
/// comparison covers a module whose base pipeline does real work.
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
  dbg(inc(List.fold(List.range(0, 3), 0, |acc, x| acc + x)))
}
",
)];

fn core_fns(m: &Module) -> CoreFns {
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

/// Lowers with the passes off, so each run starts from the same module.
fn prepared() -> (Module, CoreFns) {
    let m = lower_files_with(PROGRAM, false);
    let ids = core_fns(&m);
    (m, ids)
}

/// `Rules::default()` is the base pipeline: the same `Stats` and the same IR as `Rules::none()`.
#[test]
fn the_default_matches_an_explicitly_empty_pipeline() {
    let (mut default, ids) = prepared();
    let (mut none, none_ids) = prepared();

    let default_stats = opt::run_with(&mut default, &ids, Options::default());
    let none_stats = opt::run_with(
        &mut none,
        &none_ids,
        Options {
            rules: Rules::none(),
            ..Options::default()
        },
    );

    assert!(
        !default_stats.is_empty(),
        "the base pipeline should do its existing work: {default_stats:?}"
    );
    assert_eq!(default_stats, none_stats);
    assert_eq!(dump(&default), dump(&none));
}

/// With every rule that is still empty turned on, and the ones that have landed but do not fire
/// on this program (constant folding), neither `Stats` nor the IR changes. ζ (A2/PR-011) has
/// landed and does fire here, so it is off in this comparison.
#[test]
fn the_empty_rules_still_match_the_base_pipeline() {
    let (mut base, ids) = prepared();
    let (mut all, all_ids) = prepared();

    let base_stats = opt::run_with(&mut base, &ids, Options::default());
    let all_stats = opt::run_with(
        &mut all,
        &all_ids,
        Options {
            rules: Rules {
                zeta: false,
                ..Rules::all()
            },
            ..Options::default()
        },
    );

    assert_eq!(base_stats, all_stats);
    assert_eq!(all_stats.zeta, 0);
    assert_eq!(all_stats.beta, 0);
    assert_eq!(all_stats.eta, 0);
    assert_eq!(all_stats.iota, 0);
    assert_eq!(all_stats.delta, 0);
    assert_eq!(all_stats.constant, 0);
    assert_eq!(all_stats.algebra, 0);
    assert_eq!(all_stats.bounds, 0);
    assert_eq!(all_stats.cse, 0);
    assert_eq!(all_stats.deopt, 0);
    assert_eq!(dump(&base), dump(&all));
}

/// Turning one empty rule on and leaving the rest off is the per-rule shape the later tests use.
/// While the rule is empty, it contributes no counter and no deopt.
#[test]
fn one_empty_rule_on_is_invisible() {
    let (mut base, ids) = prepared();
    let (mut beta, beta_ids) = prepared();

    let base_stats = opt::run_with(&mut base, &ids, Options::default());
    let beta_stats = opt::run_with(
        &mut beta,
        &beta_ids,
        Options {
            rules: Rules {
                beta: true,
                ..Rules::none()
            },
            ..Options::default()
        },
    );

    assert_eq!(beta_stats.beta, 0);
    assert_eq!(beta_stats.deopt, 0);
    assert_eq!(beta_stats, base_stats);
    assert_eq!(dump(&base), dump(&beta));
}
