//! Tests for the array pipeline fusion laws (D4/PR-031): map∘map and fold∘map over `Array`.
//!
//! The two laws compose the function of one `Array` traversal with the function of the traversal
//! that consumes its array:
//!
//! - `array_map_map`: `Array.map(Array.map(a, f), g)` → `Array.map(a, λx. g(f x))`;
//! - `array_fold_map`: `Array.fold(Array.map(a, f), init, g)` →
//!   `Array.fold(a, init, λacc, x. g(acc, f x))`.
//!
//! `tests/fusion_laws.rs` fixes, for each law, that its equation fires on the left-hand side, that
//! the hand-written right-hand side is not the law's shape, that a second run changes nothing and
//! that the fused form is within the code-size budget. This file covers what is specific to the
//! array laws:
//!
//! - the representation: both functions are bundled `Std.Array` functions that lowering records by
//!   identity, not builtins, and the law matches the record (a map the lowering did not record is
//!   left alone);
//! - the composed lambda's size against the two function expressions it composes;
//! - the deopts: an effect in a function value (a `dbg` is observable), a function value whose
//!   body is not visible (a parameter), a map call that is failing through the set parameters of a
//!   generic caller, and two steps that may each reach a defect (the fused order could report the
//!   other one first);
//! - `init`'s move in the fold: one of the three evaluations may stop the program, not two;
//! - the per-body budget: a body gains at most sixteen compositions from one law in one sweep;
//! - idempotence, and that an array pipeline over a mapped range is fused completely in one sweep
//!   (`range_map_array` builds the array the fold law consumes).
//!
//! The driver's mutation pairs (`array_map_map_{fused,expanded}.emel`,
//! `array_fold_map_{fused,expanded}.emel`) run both sides on native, WASM and JS and check that the
//! fused source and the hand-written one-pass expansion print the same. The laws are off in the
//! default pipeline (they are new), so the pair checks today that the two hand-written programs
//! agree and becomes a direct check of the law's output when the law is turned on.

use crate::support;

use emela_core::opt::fusion::{self, LawId, Laws};
use emela_core::opt::{CoreFn, CoreFns, Options, Rules, Stats};
use emela_core::{Expr, Module, dump, opt};
use support::lower_files_with;

/// `Array.map(Array.map(a, f), g)`, the left-hand side of `array_map_map`: the driver's mutation
/// pair.
const ARRAY_MAP_MAP_INPUT: &str =
    include_str!("../../emela-driver/tests/programs/opt/array_map_map_fused.emel");

/// `Array.fold(Array.map(a, f), init, g)`, the left-hand side of `array_fold_map`.
const ARRAY_FOLD_MAP_INPUT: &str =
    include_str!("../../emela-driver/tests/programs/opt/array_fold_map_fused.emel");

/// The most nodes a composed lambda may add over the two function expressions it composes. It
/// mirrors `COMPOSE_OVERHEAD` in `opt::fusion::list`; the array laws share the composition shape.
const COMPOSE_OVERHEAD: usize = 20;

/// What lowering recorded about the bundled functions, by IR function.
///
/// `Lowered` does not carry lowering's map, so the tests rebuild it from the IR names, as
/// `tests/opt.rs` does. The `Std.Array` entries are the identities `D4/PR-031` added.
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

/// A module as lowering makes it (the passes off) and the bundled functions a law matches.
fn fixture(source: &str) -> (Module, CoreFns) {
    let module = lower_files_with(&[("main.emel", source)], false);
    let core_fns = core_fns(&module);
    (module, core_fns)
}

/// The flag of one law only: a test isolates the law it exercises.
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
        LawId::ArrayPipeline => laws.array_pipeline = true,
        LawId::StringConcat => laws.string_concat = true,
    }
    laws
}

/// The change counter of one law.
fn counter(stats: Stats, id: LawId) -> u32 {
    match id {
        LawId::ArrayMapMap => stats.array_map_map,
        LawId::ArrayFoldMap => stats.array_fold_map,
        _ => 0,
    }
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

/// The dump of the one function called `name`, without the rest of the module.
fn function_dump(m: &Module, name: &str) -> String {
    let ir = dump(m);
    let head = format!("fn {name}(");
    let start = ir
        .find(&head)
        .unwrap_or_else(|| panic!("no `{head}` in the dump"));
    let rest = &ir[start..];
    let end = rest.find("\n}\n").expect("a function closes with `}`") + 2;
    rest[..end].to_owned()
}

/// The number of expression nodes in `e`.
fn nodes(e: &Expr) -> usize {
    let mut count = 1;
    e.for_each_child(&mut |c| count += nodes(c));
    count
}

/// The first lambda in `e`: the composed lambda of a fixture, whose entry has no other lambda.
fn composed(e: &Expr) -> Option<&Expr> {
    if matches!(e, Expr::Lambda { .. }) {
        return Some(e);
    }
    let mut found = None;
    e.for_each_child(&mut |c| {
        if found.is_none() {
            found = composed(c);
        }
    });
    found
}

/// The body of the one function called `name`.
fn function<'a>(m: &'a Module, name: &str) -> &'a Expr {
    let mut bodies = m
        .functions
        .iter()
        .filter(|(_, f)| f.name == name)
        .map(|(_, f)| &f.body);
    let body = bodies.next().unwrap_or_else(|| panic!("no `{name}`"));
    assert!(bodies.next().is_none(), "two functions named `{name}`");
    body
}

/// The law, its input and the entry function: the two array laws, in registry order.
const CASES: [(LawId, &str, &str); 2] = [
    (LawId::ArrayMapMap, ARRAY_MAP_MAP_INPUT, "pipeline"),
    (LawId::ArrayFoldMap, ARRAY_FOLD_MAP_INPUT, "total"),
];

/// `Std.Array.map` and `Std.Array.fold` are bundled std functions, not builtins or intrinsics:
/// the IR has a direct call with a body, lowering records the identity, and a computation over an
/// array that is not one of the two is not recorded (so a law looking for the identity cannot see
/// it through a name).
#[test]
fn the_array_functions_are_bundled_functions_recorded_by_identity() {
    let source = "\
import Std.Array.{Array}

fn main() {
  a = Array.from_list([1, 2])
  dbg(Array.map(a, |x| x + 1))
  dbg(Array.fold(a, 0, |acc, x| acc + x))
  dbg(Array.size(a))
}
";
    let (module, core_fns) = fixture(source);
    let recorded = |kind| core_fns.values().filter(|k| **k == kind).count();
    assert_eq!(recorded(CoreFn::ArrayMap), 1, "{core_fns:?}");
    assert_eq!(recorded(CoreFn::ArrayFold), 1, "{core_fns:?}");

    let map = module
        .functions
        .iter()
        .find(|(_, f)| f.name == "Std.Array.map")
        .map(|(_, f)| f)
        .expect("the bundled Std.Array.map is in the module");
    let fold = module
        .functions
        .iter()
        .find(|(_, f)| f.name == "Std.Array.fold")
        .map(|(_, f)| f)
        .expect("the bundled Std.Array.fold is in the module");
    // A builtin has no body; these two have one, and it is the Emela source's.
    assert!(nodes(&map.body) > 1, "Std.Array.map has no body");
    assert!(nodes(&fold.body) > 1, "Std.Array.fold has no body");
    // Both fail through their effect-polymorphic function parameter, so δ never copied them; the
    // identity only makes the call visible to the laws.
    assert!(map.fails && fold.fails);
    assert!(!map.suspends && !fold.suspends);
}

/// The left-hand side becomes one call with a composed lambda, and only that law's counter counts
/// it.
#[test]
fn the_equation_becomes_the_fused_shape() {
    for (id, source, entry) in CASES {
        let (module, applied, stats) = apply(source, only(id));
        assert_eq!(applied, 1, "{}: {stats:?}", id.rule_id());
        assert_eq!(counter(stats, id), 1, "{}", id.rule_id());
        assert_eq!(stats.deopt, 0, "{}", id.rule_id());
        let entry = function_dump(&module, entry);
        assert_eq!(
            entry.matches("Std.Array.map[").count(),
            usize::from(id == LawId::ArrayMapMap),
            "{}: the inner call is still there: {entry}",
            id.rule_id()
        );
        assert_eq!(
            entry.matches("Std.Array.fold[").count(),
            usize::from(id == LawId::ArrayFoldMap),
            "{}",
            id.rule_id()
        );
        assert!(
            entry.contains("fn("),
            "{}: no composed lambda: {entry}",
            id.rule_id()
        );
    }
}

/// The composition is by call, in the order of the two traversals: `g`'s argument is the call of
/// `f`.
#[test]
fn the_composed_lambda_calls_the_inner_function_first() {
    let (module, _, _) = apply(ARRAY_MAP_MAP_INPUT, only(LawId::ArrayMapMap));
    let entry = function_dump(&module, "pipeline");
    assert!(
        entry.contains("twice(inc("),
        "the map∘map composition does not call `inc` inside `twice`: {entry}"
    );

    let (module, _, _) = apply(ARRAY_FOLD_MAP_INPUT, only(LawId::ArrayFoldMap));
    let entry = function_dump(&module, "total");
    assert!(
        entry.contains(", inc("),
        "the fold∘map composition does not pass `inc` to `add`: {entry}"
    );
}

/// The composed lambda costs at most the two function expressions it composes plus the constant:
/// the law writes a call of `f` and a call of `g`, not a copy of their bodies, so the composed
/// lambda's size does not grow with the size of `f` or `g`.
#[test]
fn the_composed_lambda_is_within_its_code_size_budget() {
    for (id, source, entry) in CASES {
        let (module, applied, stats) = apply(source, only(id));
        assert_eq!(applied, 1, "{}: {stats:?}", id.rule_id());
        let body = function(&module, entry);
        let lambda =
            composed(body).unwrap_or_else(|| panic!("{} did not build a lambda", id.rule_id()));
        // `inc` and the second function are references to top-level functions: one node each.
        let inputs = 2;
        assert!(
            nodes(lambda) <= inputs + COMPOSE_OVERHEAD,
            "{}: the composed lambda has {} nodes, over {inputs} + {COMPOSE_OVERHEAD}",
            id.rule_id(),
            nodes(lambda)
        );
    }
}

/// A `dbg` in a function value is an observable write; the law must leave the site alone and count
/// the deopt. The same program is then unchanged, so its stderr is the same with the law on and
/// off.
#[test]
fn an_effect_in_a_function_value_deopts() {
    let map_map = "\
import Std.Array.{Array}

fn run(a: Array[Int]) -> Array[Int] {
  Array.map(Array.map(a, |x| {
    dbg(x)
    x + 1
  }), |y| y * 2)
}

fn main() {
  dbg(run(Array.from_list([1, 2])))
  ()
}
";
    let fold_map = "\
import Std.Array.{Array}

fn run(a: Array[Int]) -> Int {
  Array.fold(Array.map(a, |x| x + 1), 0, |acc, y| {
    dbg(y)
    acc + y
  })
}

fn main() {
  dbg(run(Array.from_list([1, 2])))
  ()
}
";
    for (id, source) in [
        (LawId::ArrayMapMap, map_map),
        (LawId::ArrayFoldMap, fold_map),
    ] {
        let (before, _) = fixture(source);
        let (after, applied, stats) = apply(source, only(id));
        assert_eq!(applied, 0, "{} fused an effectful site", id.rule_id());
        assert_eq!(stats.deopt, 1, "{}: {stats:?}", id.rule_id());
        assert_eq!(counter(stats, id), 0, "{}", id.rule_id());
        assert_eq!(
            dump(&after),
            dump(&before),
            "{} changed an effectful site",
            id.rule_id()
        );
    }
}

/// A function value that is not a lambda literal or a top-level function has no visible body, so
/// the law cannot prove it effect-free: a parameter deopts.
#[test]
fn a_function_value_whose_body_is_not_visible_deopts() {
    let map_map = "\
import Std.Array.{Array}

fn run(a: Array[Int], f: fn(Int) -> Int) -> Array[Int] {
  Array.map(Array.map(a, f), |y| y * 2)
}

fn main() {
  dbg(run(Array.from_list([1, 2]), |x| x + 1))
  ()
}
";
    let fold_map = "\
import Std.Array.{Array}

fn run(a: Array[Int], g: fn(Int, Int) -> Int) -> Int {
  Array.fold(Array.map(a, |x| x + 1), 0, g)
}

fn main() {
  dbg(run(Array.from_list([1, 2]), |acc, y| acc + y))
  ()
}
";
    for (id, source) in [
        (LawId::ArrayMapMap, map_map),
        (LawId::ArrayFoldMap, fold_map),
    ] {
        let (before, _) = fixture(source);
        let (after, applied, stats) = apply(source, only(id));
        assert_eq!(applied, 0, "{} fused an opaque site", id.rule_id());
        assert_eq!(stats.deopt, 1, "{}: {stats:?}", id.rule_id());
        assert_eq!(dump(&after), dump(&before), "{}", id.rule_id());
    }
}

/// A caller with open error and effect sets passes an `Array.map` that may fail; the law refuses
/// it, because the fused lambda has no evidence table to pass on.
#[test]
fn a_failing_map_call_deopts() {
    let source = "\
import Std.Array.{Array}

fn both[A, B](a: Array[A], f: fn(A) -> B, g: fn(B) -> A) -> Array[A] {
  Array.map(Array.map(a, f), g)
}

fn main() {
  dbg(both(Array.from_list([1, 2]), |x| x + 1, |y| y * 2))
  ()
}
";
    let (_, applied, stats) = apply(source, only(LawId::ArrayMapMap));
    assert_eq!(applied, 0, "{stats:?}");
    assert_eq!(stats.deopt, 1, "{stats:?}");
}

/// The inner call is matched by identity, in the map lowering recorded, not by its name. A module
/// cannot define a function called `Std.Array.map` (the std module name is reserved), so the way a
/// same-named function could reach the IR is a lowering that did not record it as the bundled
/// `Std.Array.map`; the map is then not the law's function and the site stays a call.
#[test]
fn a_map_the_lowering_did_not_record_is_not_fused() {
    let (mut module, mut core_fns) = fixture(ARRAY_MAP_MAP_INPUT);
    let before = dump(&module);
    let map = core_fns
        .iter()
        .find(|(_, kind)| **kind == CoreFn::ArrayMap)
        .map(|(&id, _)| id)
        .expect("the module has the bundled Std.Array.map");
    core_fns.remove(&map);
    let mut stats = Stats::default();
    let applied = fusion::run(&mut module, &core_fns, only(LawId::ArrayMapMap), &mut stats);
    assert_eq!(applied, 0, "{stats:?}");
    assert_eq!(
        stats.deopt, 0,
        "an unrecorded map is not this shape: {stats:?}"
    );
    assert_eq!(dump(&module), before);
}

/// A step that may reach a defect (an integer division) is fused when the other step cannot stop
/// the program: that one's calls keep their order, so the first defect is the same. When both may
/// stop, the fused interleaving could report the other one's defect first, so the law deopts.
#[test]
fn only_one_of_the_two_functions_may_stop_the_program() {
    let one = "\
import Std.Array.{Array}

fn run(a: Array[Int]) -> Array[Int] {
  Array.map(Array.map(a, |x| x / 2), |y| y + 1)
}

fn main() {
  dbg(run(Array.from_list([2, 3])))
  ()
}
";
    let both = "\
import Std.Array.{Array}

fn run(a: Array[Int]) -> Array[Int] {
  Array.map(Array.map(a, |x| x / 2), |y| y / 2)
}

fn main() {
  dbg(run(Array.from_list([2, 3])))
  ()
}
";
    let (_, applied, stats) = apply(one, only(LawId::ArrayMapMap));
    assert_eq!(applied, 1, "one stopping step is not fused: {stats:?}");
    assert_eq!(stats.deopt, 0, "{stats:?}");

    let (before, _) = fixture(both);
    let (after, applied, stats) = apply(both, only(LawId::ArrayMapMap));
    assert_eq!(applied, 0, "two stopping steps were fused: {stats:?}");
    assert_eq!(stats.deopt, 1, "{stats:?}");
    assert_eq!(dump(&after), dump(&before));
}

/// `fold_map` also moves `init`: it is evaluated after the whole map before and as the fold's first
/// argument after. `init` may be the one evaluation that stops the program, or one of the two
/// steps, but not both.
#[test]
fn init_may_be_the_one_that_stops_the_program() {
    let quiet_init = "\
import Std.Array.{Array}

fn run(a: Array[Int], n: Int) -> Int {
  Array.fold(Array.map(a, |x| x / 2), n, |acc, y| acc + y)
}

fn main() {
  dbg(run(Array.from_list([2, 3]), 0))
  ()
}
";
    let moving_init = "\
import Std.Array.{Array}

fn run(a: Array[Int], n: Int) -> Int {
  Array.fold(Array.map(a, |x| x / 2), n / 2, |acc, y| acc + y)
}

fn main() {
  dbg(run(Array.from_list([2, 3]), 0))
  ()
}
";
    let (_, applied, stats) = apply(quiet_init, only(LawId::ArrayFoldMap));
    assert_eq!(
        applied, 1,
        "one stopping evaluation is not fused: {stats:?}"
    );
    assert_eq!(stats.deopt, 0, "{stats:?}");

    let (before, _) = fixture(moving_init);
    let (after, applied, stats) = apply(moving_init, only(LawId::ArrayFoldMap));
    assert_eq!(applied, 0, "two stopping evaluations were fused: {stats:?}");
    assert_eq!(stats.deopt, 1, "{stats:?}");
    assert_eq!(dump(&after), dump(&before));
}

/// One function body gains at most sixteen compositions from one law in one sweep; the site over
/// the budget is left alone and counted as a deopt. The traversal order is fixed, so the same sites
/// are fused on every run.
#[test]
fn the_budget_stops_a_function_body() {
    let mut source = String::from("import Std.Array.{Array}\n\nfn run(a: Array[Int]) -> Int {\n");
    for i in 0..17 {
        source.push_str(&format!(
            "  _ = Array.map(Array.map(a, |x| x + {i}), |y| y * 2)\n"
        ));
    }
    source.push_str("  0\n}\n\nfn main() {\n  dbg(run(Array.from_list([1, 2])))\n  ()\n}\n");

    let (module, applied, stats) = apply(&source, only(LawId::ArrayMapMap));
    assert_eq!(applied, 16, "{stats:?}");
    assert_eq!(stats.array_map_map, 16, "{stats:?}");
    assert_eq!(
        stats.deopt, 1,
        "the site over the budget is not a deopt: {stats:?}"
    );
    assert!(emela_core::verify(&module).is_ok());
}

/// A second run of the same law changes nothing: the fused form is a fixed point.
#[test]
fn a_second_run_changes_nothing() {
    for (id, source, _) in CASES {
        let (mut module, core_fns) = fixture(source);
        let mut first_stats = Stats::default();
        let first = fusion::run(&mut module, &core_fns, only(id), &mut first_stats);
        assert!(first > 0, "{} did not fire", id.rule_id());
        let once = dump(&module);

        let mut second_stats = Stats::default();
        let second = fusion::run(&mut module, &core_fns, only(id), &mut second_stats);
        assert_eq!(second, 0, "{} fired again", id.rule_id());
        assert!(
            second_stats.is_empty(),
            "{}: {second_stats:?}",
            id.rule_id()
        );
        assert_eq!(dump(&module), once, "{} changed the IR", id.rule_id());
    }
}

/// An array pipeline over a mapped range is fused completely in one sweep: `array_fold_map` builds
/// the fold over the array, and `range_map_array` turns the `Array.from_list(List.map(…))` it folds
/// into `$Array.map_range`. No `Array.map` call is left, and a second sweep changes nothing.
#[test]
fn a_range_pipeline_and_an_array_pipeline_are_fused_in_one_sweep() {
    let source = "\
import Std.Array.{Array}

fn main() {
  dbg(Array.fold(
    Array.map(Array.from_list(List.map(List.range(0, 3), |x| x + 1)), |y| y * 2),
    0,
    |acc, y| acc + y,
  ))
  ()
}
";
    let (mut module, core_fns) = fixture(source);
    let options = Options {
        rules: Rules::none(),
        laws: Laws::all(),
    };
    let stats = opt::run_with(&mut module, &core_fns, options);
    assert_eq!(stats.array_fold_map, 1, "{stats:?}");
    assert_eq!(stats.map_range, 1, "{stats:?}");
    let main = function_dump(&module, "main");
    assert!(main.contains("@$Array.map_range"), "{main}");
    assert!(!main.contains("Std.Array.map["), "{main}");
    assert!(!main.contains("List.map["), "{main}");

    let again = opt::run_with(&mut module, &core_fns, options);
    assert!(
        again.is_empty(),
        "a second sweep changed the module: {again:?}"
    );
}

/// A generic caller is fused too: the composed lambda's parameter and result are the caller's type
/// parameters, and monomorphization substitutes them like those of any other lambda.
#[test]
fn a_generic_composed_lambda_survives_monomorphization() {
    let source = "\
import Std.Array.{Array}

fn nested[A](a: Array[A]) -> Array[A] {
  Array.map(Array.map(a, |x| x), |x| x)
}

fn main() {
  dbg(nested(Array.from_list([1, 2])))
  ()
}
";
    let (module, applied, stats) = apply(source, only(LawId::ArrayMapMap));
    assert_eq!(applied, 1, "{stats:?}");
    assert!(emela_core::verify(&module).is_ok());
    let mono = emela_core::monomorphize(&module);
    assert!(
        emela_core::verify_monomorphic(&mono).is_ok(),
        "the fused generic lambda does not survive monomorphization"
    );
}
