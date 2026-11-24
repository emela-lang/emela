//! Tests for the implemented fusion laws (D1–D2, D6).
//!
//! Each case is one law with the two sides of its equation written by hand: the left-hand side
//! (the call or match the law matches) and, where the source language can write it, the right-hand
//! side (the loop or test the law builds). The tests fix, for every implemented law,
//!
//! - that the left-hand side is rewritten into the law's internal shape, and the law's counter
//!   counts it;
//! - that the hand-written right-hand side is not the law's shape (the law has nothing to do on
//!   it), so the two programs differ only in what the law writes;
//! - that the rewritten form is idempotent: a second [`fusion::run`] with the same law changes
//!   nothing;
//! - that the rewritten form stays within the law's code-size budget: it is not larger than the
//!   hand-written right-hand side (the checked-match form for `array_get_match`, which has no
//!   source-level unchecked load, is bounded per site instead).
//!
//! The two forms must also print the same: that is the mutation pairs of
//! `crates/emela-driver/tests/programs/opt/` (`*_fused.emel` with `*_expanded.emel`) run by
//! `crates/emela-driver/tests/opt_diff.rs` on native, WASM and JS. The pair that covers each law
//! here is named in its case; a form added here without a pair is still checked for its output by
//! the general `--no-opt` corpus, but the pair is what compares the two forms' output directly.

use crate::support;

use std::collections::HashSet;

use emela_core::opt::fusion::{self, LawId, Laws};
use emela_core::opt::{CoreFn, CoreFns, Stats};
use emela_core::{Callee, Expr, FnId, Module, dump};
use support::lower_files_with;

/// `Option.or(Array.get(a, i), d)`, the left-hand side of `option_or`.
///
/// This is the driver's mutation pair `option_fused.emel` / `option_expanded.emel` (both run by
/// `opt_diff`), so the core test and the output test use the same hand-written programs.
const OPTION_OR_INPUT: &str =
    include_str!("../../emela-driver/tests/programs/opt/option_fused.emel");

/// `match Array.get(a, i) { Some(x) -> …; None -> … }`, the left-hand side of
/// `array_get_match` and the hand-written expansion of `option_or`.
const ARRAY_GET_MATCH_INPUT: &str =
    include_str!("../../emela-driver/tests/programs/opt/option_expanded.emel");

/// [`ARRAY_GET_MATCH_INPUT`] with a second `Array.get` match in the same function, for the
/// per-site budget of the law.
const ARRAY_GET_MATCH_TWO_SITES: &str = "\
import Std.Array.{Array}

fn get_or(a: Array[Int], i: Int, d: Int) -> Int {
  first = Array.get(a, i) match {
    Some(x) -> x
    None -> d
  }
  Array.get(a, i + 1) match {
    Some(x) -> x + first
    None -> d
  }
}

fn main() {
  dbg(get_or(Array.from_list([1, 2]), 0, -1))
}
";

/// `List.fold(List.range(lo, hi), init, f)`, the left-hand side of `range_fold`.
///
/// This is the driver's mutation pair `range_fused.emel` / `range_expanded.emel`.
const RANGE_FOLD_INPUT: &str =
    include_str!("../../emela-driver/tests/programs/opt/range_fused.emel");

/// The hand-written expansion of [`RANGE_FOLD_INPUT`]: the fold as a self tail call, which
/// lowering turns into a loop.
const RANGE_FOLD_EXPANDED: &str =
    include_str!("../../emela-driver/tests/programs/opt/range_expanded.emel");

/// `List.map(List.range(lo, hi), f)`, the left-hand side of `range_map`.
const RANGE_MAP_INPUT: &str = "\
fn doubled(n: Int) -> List[Int] {
  List.map(List.range(0, n), |i| i * 2)
}

fn main() {
  dbg(List.fold(doubled(3), 0, |a, b| a + b))
}
";

/// The hand-written expansion of [`RANGE_MAP_INPUT`]: the map as a self tail call that builds the
/// list in reverse and reverses it once, as the law does.
const RANGE_MAP_EXPANDED: &str = "\
fn doubled(n: Int) -> List[Int] {
  go(0, n, [])
}

fn go(i: Int, n: Int, acc: List[Int]) -> List[Int] {
  if i >= n { List.reverse(acc) } else { go(i + 1, n, [i * 2, ..acc]) }
}

fn main() {
  dbg(List.fold(doubled(3), 0, |a, b| a + b))
}
";

/// `Array.from_list(List.map(List.range(lo, hi), f))`, the left-hand side of `range_map_array`.
///
/// This is the driver's mutation pair `array_map_range_fused.emel` /
/// `array_map_range_expanded.emel`.
const RANGE_MAP_ARRAY_INPUT: &str =
    include_str!("../../emela-driver/tests/programs/opt/array_map_range_fused.emel");

/// The hand-written expansion of [`RANGE_MAP_ARRAY_INPUT`]: a fresh array filled in place, with no
/// range and no mapped list.
const RANGE_MAP_ARRAY_EXPANDED: &str =
    include_str!("../../emela-driver/tests/programs/opt/array_map_range_expanded.emel");

/// One law and the two sides of its equation, written by hand.
struct Case {
    /// The law under test.
    id: LawId,
    /// The left-hand side, as a program the law matches.
    input: &'static str,
    /// The user function the law rewrites.
    entry: &'static str,
    /// Text the rewritten `entry` must contain: the internal form the law builds.
    fused: &'static [&'static str],
    /// The right-hand side, as a program. `None` for `array_get_match`: the unchecked load it
    /// builds has no source form, so its budget is the per-site bound below.
    expansion: Option<&'static str>,
}

/// The cases, one per implemented law, in pipeline order.
const CASES: [Case; 5] = [
    Case {
        id: LawId::OptionOr,
        input: OPTION_OR_INPUT,
        entry: "get_or",
        fused: &["@$Array.get_or"],
        expansion: Some(ARRAY_GET_MATCH_INPUT),
    },
    Case {
        id: LawId::ArrayGetMatch,
        input: ARRAY_GET_MATCH_INPUT,
        entry: "get_or",
        fused: &["@$Array.load_unchecked", "@Std.Array.size"],
        expansion: None,
    },
    Case {
        id: LawId::RangeFold,
        input: RANGE_FOLD_INPUT,
        entry: "sum_squares",
        fused: &["loop("],
        expansion: Some(RANGE_FOLD_EXPANDED),
    },
    Case {
        id: LawId::RangeMapArray,
        input: RANGE_MAP_ARRAY_INPUT,
        entry: "squares",
        fused: &["@$Array.map_range"],
        expansion: Some(RANGE_MAP_ARRAY_EXPANDED),
    },
    Case {
        id: LawId::RangeMap,
        input: RANGE_MAP_INPUT,
        entry: "doubled",
        fused: &["loop(", "List.reverse"],
        expansion: Some(RANGE_MAP_EXPANDED),
    },
];

/// The nodes one `array_get_match` site may add: the `let`s of `a` and `i`, the bounds test and
/// the load. The checked form's cost is in the library call, so the fused form spells the test
/// out; the budget is that this cost is the same at every site (see the budget test).
const ARRAY_GET_MATCH_NODES_PER_SITE: usize = 13;

/// What lowering recorded about the bundled core functions, by IR function.
///
/// Lowering's own map is not part of `Lowered`, so the tests rebuild it from the names, as
/// `tests/opt.rs` does.
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

/// A module as lowering makes it (the passes off) and the core functions a law matches.
fn fixture(source: &str) -> (Module, CoreFns) {
    let module = lower_files_with(&[("main.emel", source)], false);
    let core_fns = core_fns(&module);
    (module, core_fns)
}

/// The flag of one law only: the case isolates the law it tests.
fn only(id: LawId) -> Laws {
    let mut laws = Laws::none();
    match id {
        LawId::OptionOr => laws.option_or = true,
        LawId::ArrayGetMatch => laws.array_get_match = true,
        LawId::RangeFold => laws.range_fold = true,
        LawId::RangeMapArray => laws.range_map_array = true,
        LawId::RangeMap => laws.range_map = true,
        LawId::MapMap => laws.map_map = true,
        LawId::FoldMap => laws.fold_map = true,
        LawId::FilterMap => laws.filter_map = true,
        LawId::ArrayPipeline => laws.array_pipeline = true,
        LawId::StringConcat => laws.string_concat = true,
    }
    laws
}

/// The change counter of one law. A reserved law has no counter yet.
fn counter(stats: Stats, id: LawId) -> u32 {
    match id {
        LawId::OptionOr => stats.option_or,
        LawId::ArrayGetMatch => stats.array_get_match,
        LawId::RangeFold => stats.range_fold,
        LawId::RangeMapArray => stats.map_range,
        LawId::RangeMap => stats.range_map,
        LawId::MapMap
        | LawId::FoldMap
        | LawId::FilterMap
        | LawId::ArrayPipeline
        | LawId::StringConcat => 0,
    }
}

/// The one function called `name`.
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

/// The number of expression nodes in `e`.
fn nodes(e: &Expr) -> usize {
    let mut count = 1;
    e.for_each_child(&mut |c| count += nodes(c));
    count
}

/// The number of expression nodes the body of `name` contains, plus those of the user functions it
/// calls directly (transitively). The core functions a law emits or removes (`List.reverse`,
/// `Array.repeat`, …) are left out: they are in the module either way, and the budget compares the
/// code the law writes into the caller with the code the hand-written expansion writes.
fn body_nodes(m: &Module, name: &str) -> usize {
    fn calls(e: &Expr, out: &mut Vec<FnId>) {
        if let Expr::Call {
            callee: Callee::Direct { f, .. },
            ..
        } = e
        {
            out.push(*f);
        }
        e.for_each_child(&mut |c| calls(c, out));
    }

    let mut seen = Vec::new();
    let mut stack = vec![function(m, name)];
    let mut total = 0;
    while let Some(f) = stack.pop() {
        if seen.contains(&f) {
            continue;
        }
        seen.push(f);
        let body = &m.functions[f].body;
        total += nodes(body);
        let mut callees = Vec::new();
        calls(body, &mut callees);
        for callee in callees {
            // A user function has a plain name; a bundled core function is qualified (`List.*`).
            if (u32::from(callee.into_raw()) as usize) < m.functions.len()
                && !m.functions[callee].name.contains('.')
            {
                stack.push(callee);
            }
        }
    }
    total
}

/// Runs `laws` on the module of `source`, returning the IR, the sites rewritten and the counters.
fn apply(source: &str, laws: Laws) -> (Module, u32, Stats) {
    let (mut module, core_fns) = fixture(source);
    let mut stats = Stats::default();
    let applied = fusion::run(&mut module, &core_fns, laws, &mut stats);
    assert!(
        emela_core::verify(&module).is_ok(),
        "the law made ill-typed IR"
    );
    (module, applied, stats)
}

/// Every implemented law has a case here, and that case makes its registered body fire: the
/// registry has no entry that no test exercises, and no case names a law the registry does not
/// implement.
#[test]
fn every_implemented_law_has_a_case_that_uses_its_body() {
    let cases: HashSet<LawId> = CASES.iter().map(|case| case.id).collect();
    assert_eq!(cases.len(), CASES.len(), "a law has two cases");
    for law in fusion::laws() {
        assert_eq!(
            law.implemented(),
            cases.contains(&law.id),
            "{} and the cases disagree about whether it has a body",
            law.rule_id()
        );
    }

    for case in &CASES {
        let (module, applied, stats) = apply(case.input, only(case.id));
        assert!(applied > 0, "{} did not fire", case.id.rule_id());
        assert_eq!(counter(stats, case.id), applied, "{}", case.id.rule_id());
        assert_eq!(stats.deopt, 0, "{}", case.id.rule_id());
        let entry = function_dump(&module, case.entry);
        for token in case.fused {
            assert!(
                entry.contains(token),
                "{} did not build `{token}` in `{}`: {entry}",
                case.id.rule_id(),
                case.entry
            );
        }
    }
}

/// The left-hand side of each equation becomes the law's internal form, and only that law's
/// counter counts it.
#[test]
fn a_law_rewrites_its_equation_into_its_fused_shape() {
    for case in &CASES {
        let before = {
            let (module, _) = fixture(case.input);
            function_dump(&module, case.entry)
        };
        for token in case.fused {
            assert!(
                !before.contains(token),
                "{}: the input already has `{token}`",
                case.id.rule_id()
            );
        }

        let (module, applied, stats) = apply(case.input, only(case.id));
        assert_eq!(applied, 1, "{}: {stats:?}", case.id.rule_id());
        assert_eq!(counter(stats, case.id), 1, "{}", case.id.rule_id());
        assert_eq!(stats.passes, 0, "{}", case.id.rule_id());
        let entry = function_dump(&module, case.entry);
        for token in case.fused {
            assert!(
                entry.contains(token),
                "{} did not build `{token}` in `{}`: {entry}",
                case.id.rule_id(),
                case.entry
            );
        }
    }
}

/// The right-hand side written by hand is the same program without the law's shape: the law has
/// nothing to rewrite there, and the two forms are what the `opt_diff` mutation pair runs.
#[test]
fn a_law_leaves_its_hand_written_expansion_alone() {
    for case in &CASES {
        let Some(expansion) = case.expansion else {
            continue;
        };
        let (module, applied, stats) = apply(expansion, only(case.id));
        assert_eq!(
            applied,
            0,
            "{} rewrote its hand-written expansion: {stats:?}",
            case.id.rule_id()
        );
        assert_eq!(counter(stats, case.id), 0, "{}", case.id.rule_id());
        let entry = function_dump(&module, case.entry);
        for token in case.fused {
            assert!(
                !entry.contains(token),
                "{}: the expansion already had `{token}` before the law",
                case.id.rule_id()
            );
        }
    }
}

/// A second run of the same law changes nothing and adds no counter: the fused form is a fixed
/// point of the law that made it.
#[test]
fn a_second_run_of_a_law_changes_nothing() {
    for case in &CASES {
        let (mut module, core_fns) = fixture(case.input);
        let mut first_stats = Stats::default();
        let first = fusion::run(&mut module, &core_fns, only(case.id), &mut first_stats);
        assert!(first > 0, "{} did not fire", case.id.rule_id());
        let once = dump(&module);

        let mut second_stats = Stats::default();
        let second = fusion::run(&mut module, &core_fns, only(case.id), &mut second_stats);
        assert_eq!(second, 0, "{} fired again", case.id.rule_id());
        assert!(
            second_stats.is_empty(),
            "{}: {second_stats:?}",
            case.id.rule_id()
        );
        assert_eq!(second_stats.passes, 0, "{}", case.id.rule_id());
        assert_eq!(
            dump(&module),
            once,
            "{} changed the IR on a second run",
            case.id.rule_id()
        );
    }
}

/// The fused form is not larger than the hand-written right-hand side: the loop, the bounds test
/// and the array builtin the law writes cost what writing them by hand costs, or less.
#[test]
fn the_fused_form_is_within_its_code_size_budget() {
    for case in &CASES {
        let Some(expansion) = case.expansion else {
            continue;
        };
        let (fused, applied, _) = apply(case.input, only(case.id));
        assert!(applied > 0, "{} did not fire", case.id.rule_id());
        let (expanded, _, _) = apply(expansion, only(case.id));

        let fused_nodes = body_nodes(&fused, case.entry);
        let expanded_nodes = body_nodes(&expanded, case.entry);
        assert!(
            fused_nodes <= expanded_nodes,
            "{}: the fused form has {fused_nodes} nodes, the hand-written expansion {expanded_nodes}",
            case.id.rule_id()
        );
    }
}

/// `array_get_match` has no source form for the unchecked load it builds, so its budget is the
/// cost of the bounds test and the load at one site: the rewrite adds at most that constant, and
/// a second site costs exactly the same as the first.
#[test]
fn array_get_match_costs_a_constant_per_site() {
    let (input, _) = fixture(ARRAY_GET_MATCH_INPUT);
    let before_one = body_nodes(&input, "get_or");
    let (fused, applied, _) = apply(ARRAY_GET_MATCH_INPUT, only(LawId::ArrayGetMatch));
    assert_eq!(applied, 1);
    let after_one = body_nodes(&fused, "get_or");
    let per_site = after_one - before_one;
    assert!(
        per_site <= ARRAY_GET_MATCH_NODES_PER_SITE,
        "one site added {per_site} nodes, over the {ARRAY_GET_MATCH_NODES_PER_SITE}-node budget"
    );

    let (input, _) = fixture(ARRAY_GET_MATCH_TWO_SITES);
    let before_two = body_nodes(&input, "get_or");
    let (fused, applied, _) = apply(ARRAY_GET_MATCH_TWO_SITES, only(LawId::ArrayGetMatch));
    assert_eq!(applied, 2);
    let after_two = body_nodes(&fused, "get_or");
    assert_eq!(
        after_two - before_two,
        2 * per_site,
        "the second site did not cost the same as the first"
    );
}
