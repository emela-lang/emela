//! Tests for the default reduction pipeline (A10/PR-019).
//!
//! A10 makes every implemented reduction rule the default, so this file no longer compares the
//! new rules against the pipeline from before the kernel (the A0/PR-002 skeleton did). It fixes
//! the three properties the default pipeline needs:
//!
//! 1. `Rules::default()` is every rule of the default pipeline on. `bounds` is implemented but
//!    off until its milestone (A11/PR-040), and `cse` is still reserved (A12/PR-041).
//! 2. Each implemented rule can be turned off on its own, and its counter then stays zero.
//! 3. On one corpus program every implemented rule fires at least once. A rule that stops firing
//!    fails the test instead of turning into a silent no-op.
//!
//! The per-rule tests (`tests/zeta.rs`, `tests/beta.rs`, …) still run one rule at a time; this
//! file is about the default combination.

use crate::support;

use emela_core::opt::{CoreFn, CoreFns, Options, Rules, Stats, run_with};
use emela_core::{Module, opt};
use support::lower_files_with;

/// A program that exercises every implemented rule:
///
/// - ζ: `base = 10` is a literal binding lowering leaves in front of the closure;
/// - β: `f` is a linear lambda called once;
/// - η: `g` is a wrapper called twice, so β leaves it to η, and its callee is recursive (a loop),
///   which δ does not expand;
/// - ι: the `Some(1)` match selects its arm;
/// - δ: the direct call `double(21)` is expanded;
/// - fold: `1 + 2` is a literal operation;
/// - algebra: `True && (1 < 2)` has a constant side after folding.
const PROGRAM: &[(&str, &str)] = &[(
    "main.emel",
    "\
fn double(x: Int) -> Int {
  x + x
}

fn countdown(n: Int) -> Int {
  if n <= 0 { 0 } else { n + countdown(n - 1) }
}

fn main() {
  base = 10
  f = |x| x + base
  dbg(f(2))
  dbg(double(21))
  g = |x: Int| countdown(x)
  dbg(g(2))
  dbg(g(1))
  dbg(Some(1) match {
    Some(x) -> x
    None -> 0
  })
  dbg(1 + 2)
  dbg(True && (1 < 2))
}
",
)];

/// Rebuilds the `core_fns` map that lowering passes to `run`, by IR name. `Lowered` does not carry
/// the map (it belongs to lowering), and the test cannot see lowering's own.
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

/// Lowers with the passes off, so every run starts from the same module.
fn prepared() -> (Module, CoreFns) {
    let m = lower_files_with(PROGRAM, false);
    let ids = core_fns(&m);
    (m, ids)
}

/// Lowers a fresh module and runs the pipeline with `rules` and the default fusion laws.
fn run_rules(rules: Rules) -> Stats {
    let (mut m, ids) = prepared();
    run_with(
        &mut m,
        &ids,
        Options {
            rules,
            ..Options::default()
        },
    )
}

/// The default is every rule of the default pipeline on; `bounds` and `cse` are the only flags
/// off, because `bounds` waits for its milestone and `cse` has no rule yet.
#[test]
fn default_rules_are_every_implemented_rule() {
    let default = Rules::default();
    assert!(default.zeta, "{default:?}");
    assert!(default.beta, "{default:?}");
    assert!(default.eta, "{default:?}");
    assert!(default.iota, "{default:?}");
    assert!(default.delta, "{default:?}");
    assert!(default.fold, "{default:?}");
    assert!(default.algebra, "{default:?}");
    assert!(!default.bounds, "{default:?}");
    assert!(!default.cse, "{default:?}");

    assert_eq!(
        default,
        Rules {
            bounds: false,
            cse: false,
            ..Rules::all()
        }
    );
    assert_ne!(default, Rules::none());
    assert_ne!(default, Rules::all());
}

/// The counter of the rule called `name`.
fn counter(stats: &Stats, name: &str) -> u32 {
    match name {
        "zeta" => stats.zeta,
        "beta" => stats.beta,
        "eta" => stats.eta,
        "iota" => stats.iota,
        "delta" => stats.delta,
        "fold" => stats.constant,
        "algebra" => stats.algebra,
        other => panic!("no counter for `{other}`"),
    }
}

/// Every implemented rule fires at least once on the corpus. A zero fails the test: a rule that
/// no longer has a shape on the corpus must be noticed instead of silently doing nothing.
#[test]
fn every_implemented_rule_fires_on_the_corpus() {
    let stats = run_rules(Rules::default());
    for name in ["zeta", "beta", "eta", "iota", "delta", "fold", "algebra"] {
        assert!(
            counter(&stats, name) > 0,
            "`{name}` did not fire on the corpus: {stats:?}"
        );
    }
    assert_eq!(stats.bounds, 0, "{stats:?}");
    assert_eq!(stats.cse, 0, "{stats:?}");
}

/// The way a case clears one rule's flag.
type TurnOff = fn(&mut Rules);

/// Each implemented rule can be turned off on its own: the default with one flag cleared keeps
/// the pipeline converging and leaves that rule's counter at zero.
#[test]
fn each_implemented_rule_can_be_turned_off() {
    let cases: [(&str, TurnOff); 7] = [
        ("zeta", |r| r.zeta = false),
        ("beta", |r| r.beta = false),
        ("eta", |r| r.eta = false),
        ("iota", |r| r.iota = false),
        ("delta", |r| r.delta = false),
        ("fold", |r| r.fold = false),
        ("algebra", |r| r.algebra = false),
    ];
    for (name, turn_off) in cases {
        let mut rules = Rules::default();
        turn_off(&mut rules);
        let stats = run_rules(rules);
        assert_eq!(
            counter(&stats, name),
            0,
            "`{name}` is off but counted a rewrite: {stats:?}"
        );
        assert!(
            !stats.is_empty(),
            "`{name}` off left nothing else to do: {stats:?}"
        );
        assert_eq!(stats.bounds, 0, "{stats:?}");
        assert_eq!(stats.cse, 0, "{stats:?}");
    }
}

/// With every reduction rule off the module still reaches a fixed point. The fusion laws are on
/// and the unswitch has no flag, so the run is not required to be empty; no reduction counter may
/// move.
#[test]
fn rules_none_turns_every_reduction_off() {
    let stats = run_rules(Rules::none());
    assert_eq!(stats.zeta, 0, "{stats:?}");
    assert_eq!(stats.beta, 0, "{stats:?}");
    assert_eq!(stats.eta, 0, "{stats:?}");
    assert_eq!(stats.iota, 0, "{stats:?}");
    assert_eq!(stats.delta, 0, "{stats:?}");
    assert_eq!(stats.constant, 0, "{stats:?}");
    assert_eq!(stats.algebra, 0, "{stats:?}");
    assert!(stats.passes >= 1, "{stats:?}");
    let (mut m, ids) = prepared();
    let stats = opt::run_with(
        &mut m,
        &ids,
        Options {
            rules: Rules::none(),
            laws: emela_core::opt::fusion::Laws::none(),
        },
    );
    // The unswitch has no shape here, so nothing at all happens.
    assert_eq!(stats.hoist, 0, "{stats:?}");
    assert!(stats.is_empty(), "{stats:?}");
    assert_eq!(stats.passes, 1, "{stats:?}");
}
