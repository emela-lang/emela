//! Tests for the pipeline fusion laws (D3/PR-030, D6): map∘map, fold∘map and filter∘map.
//!
//! The three laws compose the function of one `List` traversal with the function of the traversal
//! that consumes its list:
//!
//! - `map_map`: `List.map(List.map(xs, f), g)` → `List.map(xs, λx. g(f x))`;
//! - `fold_map`: `List.fold(List.map(xs, f), init, g)` → `List.fold(xs, init, λa, x. g(a, f x))`;
//! - `filter_map`: `List.map(List.filter(xs, p), f)` →
//!   `List.reverse(List.fold(xs, [], λacc, x. if p(x) { [f(x), ..acc] } else { acc }))`.
//!
//! `tests/fusion_laws.rs` fixes, for each law, that its equation fires on the left-hand side, that
//! the hand-written right-hand side is not the law's shape, that a second run changes nothing and
//! that the fused form is within the code-size budget. This file covers what is specific to the
//! pipeline laws:
//!
//! - the composed lambda's size against the two function expressions it composes;
//! - the deopts: an effect in a function value (a `dbg` is observable, so it refuses the law), a
//!   function value whose body is not visible (a parameter), a map call that is failing through the
//!   set parameters of a generic caller, two steps that may each reach a defect (the fused order
//!   could report the other one first), and a map the lowering did not record as the bundled one;
//! - the per-body budget: a body gains at most sixteen compositions from one law in one sweep;
//! - that a range pipeline is fused completely in one sweep, because the pipeline laws run before
//!   the range laws and hand them the call they fuse.
//!
//! The driver's mutation pairs (`map_map_{fused,expanded}.emel`, `fold_map_{fused,expanded}.emel`,
//! `filter_map_{fused,expanded}.emel`) run both sides on native, WASM and JS and check that the fused
//! source and the hand-written one-pass expansion print the same. The laws are off in the default
//! pipeline (they are new), so the pair checks today that the two hand-written programs agree and
//! becomes a direct check of the law's output when the law is turned on.

mod support;

use emela_core::opt::fusion::{self, LawId, Laws};
use emela_core::opt::{CoreFn, CoreFns, Options, Rules, Stats};
use emela_core::{Expr, Module, dump, opt};
use support::lower_files_with;

/// `List.map(List.map(xs, f), g)`, the left-hand side of `map_map`: the driver's mutation pair.
const MAP_MAP_INPUT: &str =
    include_str!("../../emela-driver/tests/programs/opt/map_map_fused.emel");

/// `List.fold(List.map(xs, f), init, g)`, the left-hand side of `fold_map`.
const FOLD_MAP_INPUT: &str =
    include_str!("../../emela-driver/tests/programs/opt/fold_map_fused.emel");

/// `List.map(List.filter(xs, p), f)`, the left-hand side of `filter_map`.
const FILTER_MAP_INPUT: &str =
    include_str!("../../emela-driver/tests/programs/opt/filter_map_fused.emel");

/// The most nodes a composed lambda may add over the two function expressions it composes. It
/// mirrors `COMPOSE_OVERHEAD` in `opt::fusion::list`: the composition is by call, so its size is a
/// constant (the largest, `filter_map`, is a parameter pair, a test, a cons and two calls).
const COMPOSE_OVERHEAD: usize = 20;

/// What lowering recorded about the bundled core functions, by IR function.
///
/// `Lowered` does not carry lowering's map, so the tests rebuild it from the IR names, as
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
                "List.filter" => CoreFn::ListFilter,
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

/// The flag of one law only: a test isolates the law it exercises.
fn only(id: LawId) -> Laws {
    let mut laws = Laws::none();
    match id {
        LawId::OptionOr => laws.option_or = true,
        LawId::ArrayGetMatch => laws.array_get_match = true,
        LawId::MapMap => laws.map_map = true,
        LawId::FoldMap => laws.fold_map = true,
        LawId::FilterMap => laws.filter_map = true,
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
        LawId::OptionOr => stats.option_or,
        LawId::ArrayGetMatch => stats.array_get_match,
        LawId::MapMap => stats.map_map,
        LawId::FoldMap => stats.fold_map,
        LawId::FilterMap => stats.filter_map,
        LawId::RangeFold => stats.range_fold,
        LawId::RangeMapArray => stats.map_range,
        LawId::RangeMap => stats.range_map,
        LawId::ArrayPipeline | LawId::StringConcat => 0,
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

/// The one function called `name`.
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

/// The law, its left-hand side and the entry function: the three pipeline laws, in registry order.
const CASES: [(LawId, &str, &str); 3] = [
    (LawId::MapMap, MAP_MAP_INPUT, "pipeline"),
    (LawId::FoldMap, FOLD_MAP_INPUT, "total"),
    (LawId::FilterMap, FILTER_MAP_INPUT, "evens_plus_one"),
];

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

/// The composition is by call, in the order of the two traversals: `g`'s argument is the call of
/// `f` (map∘map and fold∘map), and `f` is called inside the branch `p` selects (filter∘map).
#[test]
fn the_composed_lambda_calls_the_inner_function_first() {
    let (module, _, _) = apply(MAP_MAP_INPUT, only(LawId::MapMap));
    let entry = function_dump(&module, "pipeline");
    assert!(
        entry.contains("twice(inc("),
        "the map∘map composition does not call `inc` inside `twice`: {entry}"
    );

    let (module, _, _) = apply(FOLD_MAP_INPUT, only(LawId::FoldMap));
    let entry = function_dump(&module, "total");
    assert!(
        entry.contains(", inc("),
        "the fold∘map composition does not pass `inc` to `add`: {entry}"
    );

    let (module, _, _) = apply(FILTER_MAP_INPUT, only(LawId::FilterMap));
    let entry = function_dump(&module, "evens_plus_one");
    assert!(
        entry.contains("if even(") && entry.contains("inc("),
        "the filter∘map composition does not test before mapping: {entry}"
    );
}

/// A `dbg` in a function value is an observable write; the law must leave the site alone and count
/// the deopt. The same program is then unchanged, so its stderr is the same with the law on and
/// off.
#[test]
fn an_effect_in_a_function_value_deopts() {
    let map_map = "\
fn run(xs: List[Int]) -> List[Int] {
  List.map(List.map(xs, |x| {
    dbg(x)
    x + 1
  }), |y| y * 2)
}

fn main() {
  dbg(run([1, 2]))
  ()
}
";
    let fold_map = "\
fn run(xs: List[Int]) -> Int {
  List.fold(List.map(xs, |x| x + 1), 0, |a, b| {
    dbg(b)
    a + b
  })
}

fn main() {
  dbg(run([1, 2]))
  ()
}
";
    let filter_map = "\
fn run(xs: List[Int]) -> List[Int] {
  List.map(List.filter(xs, |x| {
    dbg(x)
    x % 2 == 0
  }), |y| y + 1)
}

fn main() {
  dbg(run([1, 2]))
  ()
}
";
    for (id, source) in [
        (LawId::MapMap, map_map),
        (LawId::FoldMap, fold_map),
        (LawId::FilterMap, filter_map),
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
fn run(xs: List[Int], f: fn(Int) -> Int) -> List[Int] {
  List.map(List.map(xs, f), |y| y * 2)
}

fn main() {
  dbg(run([1, 2], |x| x + 1))
  ()
}
";
    let fold_map = "\
fn run(xs: List[Int], g: fn(Int, Int) -> Int) -> Int {
  List.fold(List.map(xs, |x| x + 1), 0, g)
}

fn main() {
  dbg(run([1, 2], |a, b| a + b))
  ()
}
";
    for (id, source) in [(LawId::MapMap, map_map), (LawId::FoldMap, fold_map)] {
        let (before, _) = fixture(source);
        let (after, applied, stats) = apply(source, only(id));
        assert_eq!(applied, 0, "{} fused an opaque site", id.rule_id());
        assert_eq!(stats.deopt, 1, "{}: {stats:?}", id.rule_id());
        assert_eq!(dump(&after), dump(&before), "{}", id.rule_id());
    }
}

/// A caller with open error and effect sets passes a `List.map` that may fail; the law refuses it,
/// because the fused lambda has no evidence table to pass on.
#[test]
fn a_failing_map_call_deopts() {
    let source = "\
fn both[A, B](xs: List[A], f: fn(A) -> B, g: fn(B) -> A) -> List[A] {
  List.map(List.map(xs, f), g)
}

fn main() {
  dbg(both([1, 2], |x| x + 1, |y| y * 2))
  ()
}
";
    let (_, applied, stats) = apply(source, only(LawId::MapMap));
    assert_eq!(applied, 0, "{stats:?}");
    assert_eq!(stats.deopt, 1, "{stats:?}");
}

/// The inner call is matched by identity, in the map lowering recorded, not by its name. A module
/// cannot define a function called `List.map` (the module name `List` is reserved), so the way a
/// same-named function could reach the IR is a lowering that did not record it as the bundles
/// `List.map`; the map is then not the law's function and the site stays a call.
#[test]
fn a_map_the_lowering_did_not_record_is_not_fused() {
    let (mut module, mut core_fns) = fixture(MAP_MAP_INPUT);
    let before = dump(&module);
    let map = core_fns
        .iter()
        .find(|(_, kind)| **kind == CoreFn::ListMap)
        .map(|(&id, _)| id)
        .expect("the module has the bundled List.map");
    core_fns.remove(&map);
    let mut stats = Stats::default();
    let applied = fusion::run(&mut module, &core_fns, only(LawId::MapMap), &mut stats);
    assert_eq!(applied, 0, "{stats:?}");
    assert_eq!(
        stats.deopt, 0,
        "an unrecorded map is not this shape: {stats:?}"
    );
    assert_eq!(dump(&module), before);
}

/// One function body gains at most sixteen compositions from one law in one sweep; the site over
/// the budget is left alone and counted as a deopt. The traversal order is fixed, so the same sites
/// are fused on every run.
#[test]
fn the_budget_stops_a_function_body() {
    let mut source = String::from("fn run(xs: List[Int]) -> Int {\n");
    for i in 0..17 {
        source.push_str(&format!(
            "  _ = List.map(List.map(xs, |x| x + {i}), |y| y * 2)\n"
        ));
    }
    source.push_str("  0\n}\n\nfn main() {\n  dbg(run([1, 2]))\n  ()\n}\n");

    let (module, applied, stats) = apply(&source, only(LawId::MapMap));
    assert_eq!(applied, 16, "{stats:?}");
    assert_eq!(stats.map_map, 16, "{stats:?}");
    assert_eq!(
        stats.deopt, 1,
        "the site over the budget is not a deopt: {stats:?}"
    );
    assert!(emela_core::verify(&module).is_ok());
}

/// The pipeline laws run before the range laws, so a fold of a mapped range is fused completely in
/// one sweep: `fold_map` builds the fold over the range, and `range_fold` turns that fold into a
/// loop. No `List.map` call is left, and a second sweep changes nothing.
#[test]
fn a_range_pipeline_is_fused_completely_in_one_sweep() {
    let source = "\
fn main() {
  dbg(List.fold(List.map(List.range(0, 3), |x| x + 1), 0, |a, b| a + b))
  ()
}
";
    let (mut module, core_fns) = fixture(source);
    let options = Options {
        rules: Rules::none(),
        laws: Laws::all(),
    };
    let stats = opt::run_with(&mut module, &core_fns, options);
    assert_eq!(stats.fold_map, 1, "{stats:?}");
    assert_eq!(stats.range_fold, 1, "{stats:?}");
    let main = function_dump(&module, "main");
    assert!(main.contains("loop("), "no loop in the entry: {main}");
    assert!(
        !main.contains("List.map["),
        "the mapped range is still a list: {main}"
    );

    let again = opt::run_with(&mut module, &core_fns, options);
    assert!(
        again.is_empty(),
        "a second sweep changed the module: {again:?}"
    );
    assert_eq!(again.passes, 1, "{again:?}");
}

/// A generic caller is fused too: the composed lambda's parameter and result are the caller's type
/// parameters, and monomorphization substitutes them like those of any other lambda. The fused
/// module therefore verifies both before and after monomorphization.
#[test]
fn a_generic_composed_lambda_survives_monomorphization() {
    let source = "\
fn nested[A](xs: List[A]) -> List[A] {
  List.map(List.map(xs, |x| x), |x| x)
}

fn main() {
  dbg(nested([1, 2]))
  ()
}
";
    let (module, applied, stats) = apply(source, only(LawId::MapMap));
    assert_eq!(applied, 1, "{stats:?}");
    assert!(emela_core::verify(&module).is_ok());
    let mono = emela_core::monomorphize(&module);
    assert!(
        emela_core::verify_monomorphic(&mono).is_ok(),
        "the fused generic lambda does not survive monomorphization"
    );
}

/// A step that may reach a defect (an integer division) is fused when the other step cannot stop
/// the program: that one's calls keep their order, so the first defect is the same. When both may
/// stop, the fused interleaving could report the other one's defect first, so the law deopts.
#[test]
fn only_one_of_the_two_functions_may_stop_the_program() {
    let one = "\
fn run(xs: List[Int]) -> List[Int] {
  List.map(List.map(xs, |x| x / 2), |y| y + 1)
}

fn main() {
  dbg(run([2, 3]))
  ()
}
";
    let both = "\
fn run(xs: List[Int]) -> List[Int] {
  List.map(List.map(xs, |x| x / 2), |y| y / 2)
}

fn main() {
  dbg(run([2, 3]))
  ()
}
";
    let (_, applied, stats) = apply(one, only(LawId::MapMap));
    assert_eq!(applied, 1, "one stopping step is not fused: {stats:?}");
    assert_eq!(stats.deopt, 0, "{stats:?}");

    let (before, _) = fixture(both);
    let (after, applied, stats) = apply(both, only(LawId::MapMap));
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
fn run(xs: List[Int], n: Int) -> Int {
  List.fold(List.map(xs, |x| x / 2), n, |a, b| a + b)
}

fn main() {
  dbg(run([2, 3], 0))
  ()
}
";
    let moving_init = "\
fn run(xs: List[Int], n: Int) -> Int {
  List.fold(List.map(xs, |x| x / 2), n / 2, |a, b| a + b)
}

fn main() {
  dbg(run([2, 3], 0))
  ()
}
";
    let (_, applied, stats) = apply(quiet_init, only(LawId::FoldMap));
    assert_eq!(
        applied, 1,
        "one stopping evaluation is not fused: {stats:?}"
    );
    assert_eq!(stats.deopt, 0, "{stats:?}");

    let (before, _) = fixture(moving_init);
    let (after, applied, stats) = apply(moving_init, only(LawId::FoldMap));
    assert_eq!(applied, 0, "two stopping evaluations were fused: {stats:?}");
    assert_eq!(stats.deopt, 1, "{stats:?}");
    assert_eq!(dump(&after), dump(&before));
}
