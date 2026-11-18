//! Tests for the fusion law registry (D0/PR-029).
//!
//! These fix the properties the registry has to have: the ids are unique, the registry order is the
//! pipeline order, the flags default to the laws the pipeline always ran, a disabled law
//! contributes nothing, a reserved law never runs, and the per-sweep budget stops a sweep and
//! counts what it stopped as a deopt. The laws added after the registry are implemented but off in
//! `Laws::default` because they are new: the string law (D5/PR-032), so `Laws::all` is what runs it
//! here, the three pipeline laws of D3/PR-030 and the two array pipeline laws of D4/PR-031.

mod support;

use std::collections::HashSet;

use emela_core::opt::fusion::{self, LawId, Laws};
use emela_core::opt::{CoreFn, CoreFns, Options, Stats};
use emela_core::{Module, dump, opt};
use support::lower_files_with;

/// A program each implemented law fires on: `Option.or`, a `match Array.get`, a fold and a map
/// over a range, an array built from a mapped range, a chain of `String.append` calls, the three
/// pipelines of D3/PR-030, and the two array pipelines of D4/PR-031.
const PROGRAM: &[(&str, &str)] = &[(
    "main.emel",
    "\
import Std.Array.{Array}

fn inc(x: Int) -> Int {
  x + 1
}

fn build(a: String, b: String, c: String) -> String {
  String.append(String.append(a, b), c)
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
  dbg(build(\"a\", \"b\", \"c\"))
  dbg(List.map(List.map([1, 2], |x| x + 1), |x| x * 2))
  dbg(List.fold(List.map([1, 2], |x| x + 1), 0, |a, b| a + b))
  dbg(List.map(List.filter([1, 2, 3], |x| x > 1), |x| x * 2))
  dbg(Array.map(Array.map(xs, inc), |x| x * 2))
  dbg(Array.fold(Array.map(xs, inc), 0, |a, b| a + b))
}
",
)];

/// Every implemented law, in pipeline order. The three pipeline laws of D3/PR-030, the two array
/// pipeline laws of D4/PR-031 and the string law of D5/PR-032 are implemented but off in
/// `Laws::default`, because they are new.
const IMPLEMENTED: [LawId; 11] = [
    LawId::OptionOr,
    LawId::ArrayGetMatch,
    LawId::MapMap,
    LawId::FoldMap,
    LawId::FilterMap,
    LawId::ArrayMapMap,
    LawId::ArrayFoldMap,
    LawId::RangeFold,
    LawId::RangeMapArray,
    LawId::RangeMap,
    LawId::StringConcat,
];

/// The laws on in [`Laws::default`]: the five the pipeline ran before the registry. The newer laws
/// are opt-in until they have run on the corpus.
const DEFAULT_ON: [LawId; 5] = [
    LawId::OptionOr,
    LawId::ArrayGetMatch,
    LawId::RangeFold,
    LawId::RangeMapArray,
    LawId::RangeMap,
];

/// The laws registered for a later pull request, in the order they will be inserted.
const RESERVED: [LawId; 1] = [LawId::ArrayPipeline];

/// The registry order: the implemented laws in pipeline order, then the reserved ones.
const REGISTRY_ORDER: [LawId; 12] = [
    LawId::OptionOr,
    LawId::ArrayGetMatch,
    LawId::MapMap,
    LawId::FoldMap,
    LawId::FilterMap,
    LawId::ArrayMapMap,
    LawId::ArrayFoldMap,
    LawId::RangeFold,
    LawId::RangeMapArray,
    LawId::RangeMap,
    LawId::StringConcat,
    LawId::ArrayPipeline,
];

/// What lowering recorded about the bundled core functions, by IR function.
fn core_fns(m: &Module) -> CoreFns {
    m.functions
        .iter()
        .filter_map(|(id, f)| {
            let name = f.name.split_once('$').map_or(f.name.as_str(), |(n, _)| n);
            let kind = match name {
                "Option.or" => CoreFn::OptionOr,
                "List.fold" => CoreFn::ListFold,
                "List.map" => CoreFn::ListMap,
                "List.filter" => CoreFn::ListFilter,
                "List.range" => CoreFn::ListRange,
                "List.reverse" => CoreFn::ListReverse,
                _ if name.starts_with("List.") => CoreFn::Collection,
                "Std.Array.map" => CoreFn::ArrayMap,
                "Std.Array.fold" => CoreFn::ArrayFold,
                _ => return None,
            };
            Some((id, kind))
        })
        .collect()
}

/// A module as lowering makes it (passes off) and the core functions a law matches.
fn fixture() -> (Module, CoreFns) {
    let module = lower_files_with(PROGRAM, false);
    let core_fns = core_fns(&module);
    (module, core_fns)
}

/// The flag of one law only, to show that a law fires without the others.
fn only(id: LawId) -> Laws {
    let mut laws = Laws::none();
    match id {
        LawId::OptionOr => laws.option_or = true,
        LawId::ArrayGetMatch => laws.array_get_match = true,
        LawId::MapMap => laws.map_map = true,
        LawId::FoldMap => laws.fold_map = true,
        LawId::FilterMap => laws.filter_map = true,
        LawId::ArrayMapMap => laws.array_map_map = true,
        LawId::ArrayFoldMap => laws.array_fold_map = true,
        LawId::RangeFold => laws.range_fold = true,
        LawId::RangeMapArray => laws.range_map_array = true,
        LawId::RangeMap => laws.range_map = true,
        LawId::StringConcat => laws.string_concat = true,
        LawId::ArrayPipeline => laws.array_pipeline = true,
    }
    laws
}

/// The change counter of one law, or `None` for a reserved law with no [`Stats`] field.
fn counter(stats: Stats, id: LawId) -> Option<u32> {
    Some(match id {
        LawId::OptionOr => stats.option_or,
        LawId::ArrayGetMatch => stats.array_get_match,
        LawId::MapMap => stats.map_map,
        LawId::FoldMap => stats.fold_map,
        LawId::FilterMap => stats.filter_map,
        LawId::ArrayMapMap => stats.array_map_map,
        LawId::ArrayFoldMap => stats.array_fold_map,
        LawId::RangeFold => stats.range_fold,
        LawId::RangeMapArray => stats.map_range,
        LawId::RangeMap => stats.range_map,
        LawId::StringConcat => stats.string_concat,
        LawId::ArrayPipeline => return None,
    })
}

/// Every law has its own id, so a flag or a report never names two laws.
#[test]
fn the_law_ids_are_unique() {
    let mut seen = HashSet::new();
    for law in fusion::laws() {
        assert!(seen.insert(law.id), "{} appears twice", law.rule_id());
    }
    assert_eq!(seen.len(), REGISTRY_ORDER.len());
    for id in REGISTRY_ORDER {
        assert!(seen.contains(&id), "{} is not registered", id.rule_id());
    }
}

/// The registry is the pipeline: the implemented laws are listed in the order the pipeline runs
/// them, and the reserved ones sit where they will be inserted.
#[test]
fn the_registry_is_in_pipeline_order() {
    let order: Vec<LawId> = fusion::laws().iter().map(|law| law.id).collect();
    assert_eq!(order, REGISTRY_ORDER);
    assert_eq!(&order[..IMPLEMENTED.len()], &IMPLEMENTED);
    assert_eq!(&order[IMPLEMENTED.len()..], &RESERVED);
}

/// `Default` is the pipeline before the registry: the five laws it ran on, every later one off.
#[test]
fn the_default_flags_are_the_five_laws_the_pipeline_ran() {
    let default = Laws::default();
    for id in DEFAULT_ON {
        assert!(default.is_enabled(id), "{} is off by default", id.rule_id());
    }
    for law in fusion::laws() {
        if !DEFAULT_ON.contains(&law.id) {
            assert!(
                !default.is_enabled(law.id),
                "{} is on by default",
                law.rule_id()
            );
        }
    }

    let all = Laws::all();
    let none = Laws::none();
    for id in REGISTRY_ORDER {
        assert!(all.is_enabled(id), "{} is off in all", id.rule_id());
        assert!(!none.is_enabled(id), "{} is on in none", id.rule_id());
    }
    assert_ne!(Laws::default(), Laws::all());
    assert_ne!(Laws::default(), Laws::none());
}

/// Every law carries the nine contract lines of plan §3.4.
#[test]
fn every_law_has_a_complete_contract() {
    for law in fusion::laws() {
        assert_eq!(law.rule_id(), law.id.rule_id());
        for (name, text) in [
            ("Theory", law.theory),
            ("Equation", law.equation),
            ("Preconditions", law.preconditions),
            ("Postconditions", law.postconditions),
            ("Preserves", law.preserves),
            ("Deopt", law.deopt),
            ("Budget", law.budget),
            ("Measures", law.measures),
        ] {
            assert!(!text.trim().is_empty(), "{} has no {name}", law.rule_id());
        }
    }
}

/// A reserved law has no body, and turning its flag on changes neither the IR nor a counter: it
/// cannot run before its pull request lands.
#[test]
fn the_reserved_laws_are_not_implemented_and_do_not_run() {
    for law in fusion::laws() {
        assert_eq!(
            law.implemented(),
            IMPLEMENTED.contains(&law.id),
            "{} has the wrong status",
            law.rule_id()
        );
    }

    let (module, core_fns) = fixture();
    let mut expected = module.clone();
    let mut expected_stats = Stats::default();
    let expected_applied = fusion::run(
        &mut expected,
        &core_fns,
        Laws::default(),
        &mut expected_stats,
    );

    // The reserved flag on, with the implemented laws at their default: the reserved flag must not
    // change anything.
    let reserved_on = Laws {
        array_pipeline: true,
        ..Laws::default()
    };
    let mut with_reserved = module;
    let mut with_reserved_stats = Stats::default();
    let applied = fusion::run(
        &mut with_reserved,
        &core_fns,
        reserved_on,
        &mut with_reserved_stats,
    );

    assert_eq!(applied, expected_applied);
    assert_eq!(with_reserved_stats, expected_stats);
    assert_eq!(dump(&with_reserved), dump(&expected));
}

/// `Laws::none()` rewrites nothing: no counter, no deopt, the same module.
#[test]
fn an_empty_registry_changes_nothing() {
    let (mut module, core_fns) = fixture();
    let before = dump(&module);
    let mut stats = Stats::default();

    let applied = fusion::run(&mut module, &core_fns, Laws::none(), &mut stats);

    assert_eq!(applied, 0);
    assert_eq!(stats, Stats::default(), "{stats:?}");
    assert_eq!(stats.deopt, 0, "{stats:?}");
    assert_eq!(dump(&module), before);
}

/// Each implemented law fires on its own and fills only its own counter; the other laws of a
/// disabled flag stay out entirely.
#[test]
fn one_law_at_a_time_fills_only_its_own_counter() {
    let (module, core_fns) = fixture();
    for id in IMPLEMENTED {
        let mut m = module.clone();
        let mut stats = Stats::default();
        let applied = fusion::run(&mut m, &core_fns, only(id), &mut stats);

        assert!(applied > 0, "{} did not fire", id.rule_id());
        assert_eq!(counter(stats, id), Some(applied), "{}", id.rule_id());
        assert_eq!(stats.deopt, 0, "{}", id.rule_id());
        for other in IMPLEMENTED {
            if other != id {
                assert_eq!(
                    counter(stats, other).unwrap_or(0),
                    0,
                    "{} fired while only {} was on",
                    other.rule_id(),
                    id.rule_id()
                );
            }
        }
        assert_eq!(stats.passes, 0);
        assert!(
            emela_core::verify(&m).is_ok(),
            "{} made ill-typed IR",
            id.rule_id()
        );
    }
}

/// The pipeline runs the registry where the five direct calls were: with the default flags it
/// fires every default-on law, with the laws off it reports no fusion counter, and with every law
/// on it fires the newer ones too. `Stats::deopt` also counts the δ deopts, which are not a law's,
/// so it is not asserted here.
#[test]
fn the_pipeline_runs_the_registry() {
    let (mut fused, fused_ids) = fixture();
    let (mut unfused, unfused_ids) = fixture();

    let with = opt::run_with(&mut fused, &fused_ids, Options::default());
    for id in DEFAULT_ON {
        assert!(
            counter(with, id).unwrap_or(0) > 0,
            "{} did not fire",
            id.rule_id()
        );
    }
    for id in IMPLEMENTED {
        if !DEFAULT_ON.contains(&id) {
            assert_eq!(
                counter(with, id),
                Some(0),
                "{} fired by default",
                id.rule_id()
            );
        }
    }
    let without = opt::run_with(
        &mut unfused,
        &unfused_ids,
        Options {
            laws: Laws::none(),
            ..Options::default()
        },
    );
    for id in IMPLEMENTED {
        assert_eq!(
            counter(without, id).unwrap_or(0),
            0,
            "{} fired",
            id.rule_id()
        );
    }
    assert_ne!(dump(&fused), dump(&unfused));

    let (mut all, all_ids) = fixture();
    let everything = opt::run_with(
        &mut all,
        &all_ids,
        Options {
            laws: Laws::all(),
            ..Options::default()
        },
    );
    for id in IMPLEMENTED {
        assert!(
            counter(everything, id).unwrap_or(0) > 0,
            "{} did not fire in all",
            id.rule_id()
        );
    }
}

/// The fusion of one sweep through the registry is the sum of the default-on laws: the corpus
/// gives one application of each, in the registry order, with no deopt at the default budget. The
/// newer laws are off, so their sites stay as they are.
#[test]
fn a_default_run_applies_each_law_once_on_the_corpus() {
    let (mut module, core_fns) = fixture();
    let mut stats = Stats::default();

    let applied = fusion::run(&mut module, &core_fns, Laws::default(), &mut stats);

    assert_eq!(applied, DEFAULT_ON.len() as u32, "{stats:?}");
    for id in IMPLEMENTED {
        let expected = u32::from(DEFAULT_ON.contains(&id));
        assert_eq!(counter(stats, id), Some(expected), "{}", id.rule_id());
    }
    assert_eq!(stats.deopt, 0, "{stats:?}");
    assert!(
        applied < fusion::MAX_FUSIONS_PER_SWEEP,
        "the default budget must not fire on the corpus"
    );
    assert!(emela_core::verify(&module).is_ok());
}

/// A budget of zero or one stops the sweep: fewer applications, one deopt per law the budget
/// stopped, and the module stays as well typed as the unfused one.
#[test]
fn the_budget_stops_the_sweep_and_counts_deopts() {
    let (module, core_fns) = fixture();
    let before = dump(&module);

    let mut zero = module.clone();
    let mut zero_stats = Stats::default();
    let zero_applied =
        fusion::run_with_budget(&mut zero, &core_fns, Laws::default(), &mut zero_stats, 0);
    assert_eq!(zero_applied, 0);
    assert!(zero_stats.is_empty(), "{zero_stats:?}");
    assert_eq!(zero_stats.deopt, DEFAULT_ON.len() as u32, "{zero_stats:?}");
    assert_eq!(dump(&zero), before);
    assert!(emela_core::verify(&zero).is_ok());

    let mut one = module.clone();
    let mut one_stats = Stats::default();
    let one_applied =
        fusion::run_with_budget(&mut one, &core_fns, Laws::default(), &mut one_stats, 1);
    assert_eq!(one_applied, 1);
    assert_eq!(one_stats.option_or, 1, "{one_stats:?}");
    assert_eq!(
        one_stats.deopt,
        DEFAULT_ON.len() as u32 - 1,
        "{one_stats:?}"
    );
    for id in &DEFAULT_ON[1..] {
        assert_eq!(counter(one_stats, *id), Some(0), "{}", id.rule_id());
    }
    assert_ne!(dump(&one), before);
    assert!(emela_core::verify(&one).is_ok());

    let (mut full, full_ids) = fixture();
    let mut full_stats = Stats::default();
    let full_applied = fusion::run(&mut full, &full_ids, Laws::default(), &mut full_stats);
    assert_eq!(full_applied, DEFAULT_ON.len() as u32);
    assert_eq!(full_stats.deopt, 0, "{full_stats:?}");
    assert_ne!(dump(&full), dump(&one));
    assert_ne!(dump(&full), dump(&zero));
}
