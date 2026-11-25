//! B7 analysis properties of the Core IR analyses: the lattice laws, monotonicity, determinism
//! and a corpus smoke test for [`effect`], [`uses`], [`callgraph`] and [`ranges`].
//!
//! These tests are about the analyses themselves, not about a rule that consumes one. They pin
//! the part of the contract a consumer may rely on:
//!
//! - a "may" fact is monotone: adding a source adds facts and never clears one;
//! - every lattice join is commutative, associative and idempotent, and has the documented bounds;
//! - a well-formed module gets `Known` facts, so the analyses are green on their own;
//! - two runs and two arena orders give the same facts.
//!
//! The per-analysis edge cases (a divisor that may be zero, an unprovable loop bound, an external
//! call, …) are in `analysis_effect.rs` and `analysis_ranges.rs`; this file does not repeat them.

use crate::support;

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use emela_core::opt::analysis::effect::EffectSummary;
use emela_core::opt::analysis::ranges::{Env, Interval, of_expr};
use emela_core::opt::analysis::{Fact, callgraph, effect, ranges, uses};
use emela_core::{BinOp, Callee, Expr, FnId, Lit, Local, Module, OpTy, TypeId};
use support::lower_files;

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

/// The function of `m` named `name`.
fn function(m: &Module, name: &str) -> FnId {
    m.functions
        .iter()
        .find(|(_, f)| f.name == name)
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no function `{name}` in\n{}", emela_core::dump(m)))
}

/// Every one of the 32 effect summaries, one per set of "may" fields.
fn every_summary() -> Vec<EffectSummary> {
    (0u32..32)
        .map(|bits| EffectSummary {
            may_use_effect: bits & 1 != 0,
            may_fail: bits & 2 != 0,
            may_suspend: bits & 4 != 0,
            may_defect: bits & 8 != 0,
            may_diverge: bits & 16 != 0,
        })
        .collect()
}

/// The pointwise order of a summary: `a <= b` when every "may" fact of `a` holds in `b`.
fn effect_le(a: EffectSummary, b: EffectSummary) -> bool {
    (!a.may_use_effect || b.may_use_effect)
        && (!a.may_fail || b.may_fail)
        && (!a.may_suspend || b.may_suspend)
        && (!a.may_defect || b.may_defect)
        && (!a.may_diverge || b.may_diverge)
}

/// A direct call of `f` with no arguments.
fn call(f: FnId) -> Expr {
    Expr::Call {
        callee: Callee::Direct {
            f,
            ty_args: Vec::new(),
        },
        args: Vec::new(),
        fails: false,
        suspends: false,
    }
}

/// `f` used as a function value.
fn value(f: FnId) -> Expr {
    Expr::Fn {
        f,
        ty_args: Vec::new(),
    }
}

/// A read of `local`.
fn var(local: Local) -> Expr {
    Expr::Var(local)
}

/// `a + b` on `Int`.
fn add(lhs: Expr, rhs: Expr) -> Expr {
    Expr::Binary {
        op: BinOp::Add,
        ty: OpTy::Int,
        lhs: Box::new(lhs),
        rhs: Box::new(rhs),
    }
}

/// The names of the functions of `m`, in arena order.
fn function_names(m: &Module) -> Vec<String> {
    m.functions.iter().map(|(_, f)| f.name.clone()).collect()
}

/// The effect summary of each function, keyed by name, so two modules can be compared even when
/// their arenas were built in another order.
fn summaries_by_name(m: &Module) -> BTreeMap<String, EffectSummary> {
    let summaries = effect::of_module(m);
    m.functions
        .iter()
        .map(|(id, f)| (f.name.clone(), summaries[&id]))
        .collect()
}

/// The use counts of a function, keyed by the local's hint.
fn use_hints(m: &Module, f: FnId) -> BTreeMap<String, u32> {
    uses::of_function(m, f)
        .into_iter()
        .map(|(local, count)| (m.locals[local].hint.clone(), count))
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Effect: the summary lattice
// ---------------------------------------------------------------------------------------------

/// The join is a least upper bound: commutative, associative and idempotent, with `pure` as the
/// identity and `unknown` as the absorbing top. The order is the pointwise implication of the
/// "may" fields, so `pure <= every summary <= unknown`.
#[test]
fn the_effect_join_is_a_lattice() {
    let summaries = every_summary();
    let pure = EffectSummary::pure();
    let unknown = EffectSummary::unknown();
    for a in &summaries {
        assert_eq!(a.join(*a), *a, "idempotent: {a:?}");
        assert_eq!(a.join(pure), *a, "pure is the identity");
        assert_eq!(a.join(unknown), unknown, "unknown absorbs");
        assert!(effect_le(pure, *a), "pure is the bottom: {a:?}");
        assert!(effect_le(*a, unknown), "unknown is the top: {a:?}");
        for b in &summaries {
            assert_eq!(a.join(*b), b.join(*a), "commutative: {a:?}, {b:?}");
            let joined = a.join(*b);
            assert!(effect_le(*a, joined), "{a:?} <= its join");
            assert!(effect_le(*b, joined), "{b:?} <= its join");
            for c in &summaries {
                assert_eq!(
                    a.join(b.join(*c)),
                    a.join(*b).join(*c),
                    "associative: {a:?}, {b:?}, {c:?}"
                );
            }
        }
    }
}

/// A summary joins only by setting fields: a consumer that stores a `Known` summary never loses a
/// fact by joining another one.
#[test]
fn joining_a_summary_never_clears_a_fact() {
    for a in every_summary() {
        for b in every_summary() {
            let joined = a.join(b);
            assert!(effect_le(a, joined));
            assert!(effect_le(b, joined));
        }
    }
}

/// Adding an effect source to a module never removes a fact from the function that contains it or
/// from a caller. Each pair changes one kind of source: an effect, a failure, a defect and a
/// recursion (divergence).
#[test]
fn adding_an_effect_source_never_removes_a_fact() {
    let pairs = [
        (
            "effect",
            "\
fn g() -> Int { 1 }
fn h() -> Int { g() }
fn main() { dbg(h()) }
",
            "\
fn g() -> Int {
  dbg(1)
  1
}
fn h() -> Int { g() }
fn main() { dbg(h()) }
",
        ),
        (
            "failure",
            "\
error NotFound(id: Int)
fn g() -> Int { 1 }
fn h() -> Int { g() }
fn main() { dbg(h()) }
",
            "\
error NotFound(id: Int)
fn g() -> Int fails NotFound { fail NotFound(id: 0) }
fn h() -> Int fails NotFound { g() }
fn main() {
  dbg(h() escape {
    _ -> 0
  })
}
",
        ),
        (
            "defect",
            "\
fn g(a: Int) -> Int { a }
fn h(a: Int) -> Int { g(a) }
fn main() { dbg(h(1)) }
",
            "\
fn g(a: Int) -> Int { a / 2 }
fn h(a: Int) -> Int { g(a) }
fn main() { dbg(h(1)) }
",
        ),
        (
            "divergence",
            "\
fn g(n: Int) -> Int { n }
fn h(n: Int) -> Int { g(n) }
fn main() { dbg(h(1)) }
",
            "\
fn g(n: Int) -> Int {
  if n > 0 { g(n - 1) } else { n }
}
fn h(n: Int) -> Int { g(n) }
fn main() { dbg(h(1)) }
",
        ),
    ];

    for (kind, calm, loud) in pairs {
        let calm = lower_files(&[("main.emel", calm)]);
        let loud = lower_files(&[("main.emel", loud)]);
        for name in ["g", "h"] {
            let before = effect::of_function(&calm, function(&calm, name));
            let after = effect::of_function(&loud, function(&loud, name));
            assert!(
                effect_le(before, after),
                "{kind}: `{name}` lost a fact: {before:?} -> {after:?}"
            );
            assert_eq!(
                before.join(after),
                after,
                "{kind}: `{name}` is not monotone"
            );
        }
        assert_ne!(
            effect::of_function(&calm, function(&calm, "g")),
            effect::of_function(&loud, function(&loud, "g")),
            "{kind}: the test program did not add a source"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Use counts
// ---------------------------------------------------------------------------------------------

/// A use is a read, so the count of a local is additive over the expression tree: concatenating
/// two expressions adds their counts, and adding a read increases the count.
#[test]
fn use_counts_are_additive_and_never_decrease() {
    let mut m = Module::new();
    let x = m.local("x", TypeId::INT);
    let y = m.local("y", TypeId::INT);
    let once = var(x);
    let twice = Expr::Tuple(vec![var(x), var(x)]);
    assert_eq!(uses::of_expr(&once, x), 1);
    assert_eq!(uses::of_expr(&twice, x), 2);
    assert_eq!(
        uses::of_expr(&Expr::Tuple(vec![once.clone(), twice.clone()]), x),
        uses::of_expr(&once, x) + uses::of_expr(&twice, x)
    );
    assert_eq!(
        uses::of_expr(&add(once.clone(), var(y)), x),
        uses::of_expr(&once, x)
    );
    assert_eq!(
        uses::of_expr(&add(once.clone(), var(y)), y),
        1,
        "the other operand is counted on its own"
    );
    assert!(
        uses::of_expr(&twice, x) >= uses::of_expr(&once, x),
        "adding a read never lowers the count"
    );
    // A binder is not a read of the bound local.
    let bound = Expr::Let {
        var: y,
        value: Box::new(Expr::Lit(Lit::Int(1))),
        body: Box::new(var(y)),
    };
    assert_eq!(uses::of_expr(&bound, y), 1, "only the body reads `y`");
}

/// The module-level count is the sum of the per-function counts; locals are unique in a module, so
/// merging the maps loses nothing.
#[test]
fn the_module_use_counts_are_the_sum_of_the_functions() {
    let mut m = Module::new();
    let x = m.local("x", TypeId::INT);
    let y = m.local("y", TypeId::INT);
    let f = m.declare("f", false);
    m.define(
        f,
        Vec::new(),
        TypeId::INT,
        Expr::Let {
            var: x,
            value: Box::new(Expr::Lit(Lit::Int(1))),
            body: Box::new(add(var(x), var(x))),
        },
    );
    let g = m.declare("g", false);
    m.define(
        g,
        Vec::new(),
        TypeId::INT,
        Expr::Let {
            var: y,
            value: Box::new(Expr::Lit(Lit::Int(2))),
            body: Box::new(var(y)),
        },
    );

    let Fact::Known(all) = uses::analyze(&m) else {
        panic!("use counts are total");
    };
    assert_eq!(all[&x], 2);
    assert_eq!(all[&y], 1);
    let by_function = uses::of_module(&m);
    for (id, counts) in &by_function {
        for (local, count) in counts {
            assert_eq!(
                all[local], *count,
                "the module count of a local of function {id:?} differs"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Call graph
// ---------------------------------------------------------------------------------------------

/// Adding a call (or a function value) only adds edges: every adjacency list before is contained
/// in the one after, on both the callee and the caller side. A cycle that existed stays a cycle.
#[test]
fn adding_a_call_never_removes_an_edge() {
    let mut m = Module::new();
    let a = m.declare("a", false);
    let b = m.declare("b", false);
    let c = m.declare("c", false);
    let d = m.declare("d", false);
    m.define(a, Vec::new(), TypeId::UNIT, call(b));
    m.define(b, Vec::new(), TypeId::UNIT, Expr::Lit(Lit::Unit));
    m.define(c, Vec::new(), TypeId::UNIT, Expr::Lit(Lit::Unit));
    m.define(d, Vec::new(), TypeId::UNIT, Expr::Lit(Lit::Unit));

    let before = callgraph::CallGraph::of_module(&m);
    // Redefining `a` adds a direct call and a function value; nothing is removed.
    m.define(
        a,
        Vec::new(),
        TypeId::UNIT,
        Expr::Tuple(vec![call(b), call(c), value(d), call(b)]),
    );
    let after = callgraph::CallGraph::of_module(&m);

    for (id, _) in m.functions.iter() {
        let old: BTreeSet<_> = before.callees[&id].iter().copied().collect();
        let new: BTreeSet<_> = after.callees[&id].iter().copied().collect();
        assert!(
            old.is_subset(&new),
            "function {id:?} lost a callee: {old:?} -> {new:?}"
        );
        for callee in old {
            assert!(
                after.callers[&callee].contains(&id),
                "the reverse edge disappeared"
            );
        }
        if before.is_recursive(id) {
            assert!(after.is_recursive(id), "a cycle disappeared");
        }
    }
    assert_eq!(after.callees[&a], vec![b, c, d], "sorted and deduplicated");
}

/// The adjacency lists are sorted by `FnId` (`m.functions` order), so the graph and its components
/// are the same on two runs and do not depend on the order the calls appear in a body.
#[test]
fn the_call_graph_is_sorted_deduplicated_and_deterministic() {
    let mut m = Module::new();
    let a = m.declare("a", false);
    let b = m.declare("b", false);
    let c = m.declare("c", false);
    let d = m.declare("d", false);
    m.define(
        a,
        Vec::new(),
        TypeId::UNIT,
        Expr::Tuple(vec![call(d), call(b), call(d), value(c)]),
    );
    m.define(b, Vec::new(), TypeId::UNIT, call(c));
    m.define(c, Vec::new(), TypeId::UNIT, Expr::Lit(Lit::Unit));
    m.define(d, Vec::new(), TypeId::UNIT, Expr::Lit(Lit::Unit));

    let first = callgraph::CallGraph::of_module(&m);
    let second = callgraph::CallGraph::of_module(&m);
    assert_eq!(first, second);
    assert_eq!(first.sccs(), second.sccs());
    assert_eq!(first.callees[&a], vec![b, c, d]);
    assert_eq!(first.sccs(), vec![vec![c], vec![b], vec![d], vec![a]]);
}

// ---------------------------------------------------------------------------------------------
// Ranges: the interval lattice and monotone transfer
// ---------------------------------------------------------------------------------------------

/// The sample intervals of the join/meet laws: both ends, a point and overlapping and nested
/// bounded intervals.
fn sample_intervals() -> Vec<Interval> {
    vec![
        Interval::Bottom,
        Interval::Top,
        Interval::point(0),
        Interval::range(1, 3),
        Interval::range(2, 8),
        Interval::range(-5, 5),
        Interval::range(0, 100),
        Interval::range(-100, 0),
        Interval::range(1, 3),
    ]
}

/// The join is the hull and a least upper bound; the meet is the intersection and a greatest
/// lower bound. Both are commutative, associative and idempotent, with the documented bounds:
/// `Bottom <= every interval <= Top`.
#[test]
fn the_interval_join_and_meet_are_lattices() {
    let intervals = sample_intervals();
    for a in &intervals {
        assert!(
            Interval::Bottom <= *a && *a <= Interval::Top,
            "the ends of the order: {a:?}"
        );
        assert_eq!(a.join(*a), *a, "join idempotent: {a:?}");
        assert_eq!(a.meet(*a), *a, "meet idempotent: {a:?}");
        assert_eq!(a.join(Interval::Bottom), *a, "Bottom is the join identity");
        assert_eq!(a.meet(Interval::Top), *a, "Top is the meet identity");
        assert_eq!(a.join(Interval::Top), Interval::Top, "Top absorbs a join");
        assert_eq!(
            a.meet(Interval::Bottom),
            Interval::Bottom,
            "Bottom absorbs a meet"
        );
        for b in &intervals {
            assert_eq!(a.join(*b), b.join(*a), "join commutative");
            assert_eq!(a.meet(*b), b.meet(*a), "meet commutative");
            let joined = a.join(*b);
            assert!(*a <= joined && *b <= joined, "the join is an upper bound");
            let met = a.meet(*b);
            assert!(met <= *a && met <= *b, "the meet is a lower bound");
            for c in &intervals {
                assert_eq!(
                    a.join(b.join(*c)),
                    a.join(*b).join(*c),
                    "join associative: {a:?}, {b:?}, {c:?}"
                );
                assert_eq!(
                    a.meet(b.meet(*c)),
                    a.meet(*b).meet(*c),
                    "meet associative: {a:?}, {b:?}, {c:?}"
                );
                if *a <= *c && *b <= *c {
                    assert!(joined <= *c, "the join is the least upper bound");
                }
                if *c <= *a && *c <= *b {
                    assert!(*c <= met, "the meet is the greatest lower bound");
                }
            }
        }
    }
}

/// The join of two bounded intervals is exactly the smallest interval that contains both values,
/// and two overlapping intervals that are not nested stay incomparable.
#[test]
fn the_join_is_the_hull_and_the_order_is_partial() {
    assert_eq!(
        Interval::range(1, 3).join(Interval::range(2, 8)),
        Interval::range(1, 8)
    );
    assert_eq!(
        Interval::range(-4, 0).join(Interval::range(0, 9)),
        Interval::range(-4, 9)
    );
    assert_eq!(
        Interval::range(0, 2).partial_cmp(&Interval::range(1, 3)),
        None,
        "overlapping but not nested"
    );
    assert!(Interval::range(0, 2) < Interval::range(0, 3));
    assert!(Interval::range(0, 3) < Interval::Top);
    assert!(Interval::Bottom < Interval::range(0, 3));
}

/// The arithmetic transfer is monotone: widening an operand interval never narrows the result.
/// The samples stay inside `Int`, and a divisor that may be zero or a result that may wrap is
/// `Top`, which is above every result.
#[test]
fn the_range_transfer_is_monotone_in_its_operands() {
    let mut m = Module::new();
    let x = m.local("x", TypeId::INT);
    let y = m.local("y", TypeId::INT);
    let operations = [BinOp::Add, BinOp::Sub, BinOp::Mul, BinOp::Div, BinOp::Rem];
    let samples = [
        Interval::range(-10, -5),
        Interval::point(0),
        Interval::range(1, 3),
        Interval::range(2, 8),
        Interval::range(-2, 3),
        Interval::range(4, 5),
        Interval::range(1, 100),
    ];

    for op in operations {
        let expr = Expr::Binary {
            op,
            ty: OpTy::Int,
            lhs: Box::new(var(x)),
            rhs: Box::new(var(y)),
        };
        let eval = |left: Interval, right: Interval| {
            let mut env = Env::new();
            env.set(x, left);
            env.set(y, right);
            of_expr(&m, &expr, &env)
        };
        for a in samples {
            for wider_a in samples {
                if !matches!(
                    a.partial_cmp(&wider_a),
                    Some(Ordering::Less | Ordering::Equal)
                ) {
                    continue;
                }
                for b in samples {
                    for wider_b in samples {
                        if !matches!(
                            b.partial_cmp(&wider_b),
                            Some(Ordering::Less | Ordering::Equal)
                        ) {
                            continue;
                        }
                        // Division and remainder by a range containing 0 are Top; skip them so
                        // that the test compares results rather than two deopts.
                        if matches!(op, BinOp::Div | BinOp::Rem)
                            && (b.contains(0) || wider_b.contains(0))
                        {
                            continue;
                        }
                        let narrow = eval(a, b);
                        let wide = eval(wider_a, wider_b);
                        assert!(
                            narrow <= wide,
                            "{op:?}: {a:?} {b:?} -> {narrow:?} is not below {wider_a:?} \
                             {wider_b:?} -> {wide:?}"
                        );
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Determinism and module order
// ---------------------------------------------------------------------------------------------

/// Builds one module with `one`, `two` and `three` declared in `order` (`0` is `one`), then
/// defines the same bodies. The only difference is the arena order of the three functions.
fn module_in_order(order: [u8; 3]) -> Module {
    let names = ["one", "two", "three"];
    let mut m = Module::new();
    let mut ids: [Option<FnId>; 3] = [None; 3];
    for &index in &order {
        ids[index as usize] = Some(m.declare(names[index as usize], false));
    }
    let one = ids[0].expect("one");
    let two = ids[1].expect("two");
    let three = ids[2].expect("three");

    m.define(one, Vec::new(), TypeId::INT, Expr::Lit(Lit::Int(1)));
    m.define(
        two,
        Vec::new(),
        TypeId::INT,
        add(call(one), Expr::Lit(Lit::Int(2))),
    );
    m.define(
        three,
        Vec::new(),
        TypeId::INT,
        Expr::Binary {
            op: BinOp::Div,
            ty: OpTy::Int,
            lhs: Box::new(Expr::Lit(Lit::Int(1))),
            rhs: Box::new(Expr::Lit(Lit::Int(2))),
        },
    );
    m
}

/// Each analysis gives the same fact twice, and the facts follow the functions in arena order: a
/// module built with a different declaration order has the same per-name facts, and reading the
/// output in `m.functions` order visits every function exactly once.
#[test]
fn the_core_analyses_are_deterministic_and_follow_arena_order() {
    let first = module_in_order([0, 1, 2]);
    let second = module_in_order([2, 1, 0]);
    assert_eq!(function_names(&first), ["one", "two", "three"]);
    assert_eq!(function_names(&second), ["three", "two", "one"]);
    assert_ne!(function_names(&first), function_names(&second));

    // Each analysis on one module is the same on the second run.
    assert_eq!(effect::analyze(&first), effect::analyze(&first));
    assert_eq!(uses::analyze(&first), uses::analyze(&first));
    assert_eq!(
        callgraph::analyze(&first),
        callgraph::analyze(&first),
        "the call graph is deterministic"
    );
    assert_eq!(ranges::analyze(&first), ranges::analyze(&first));

    // The per-name facts do not depend on the arena order.
    assert_eq!(summaries_by_name(&first), summaries_by_name(&second));
    for name in ["one", "two", "three"] {
        assert_eq!(
            use_hints(&first, function(&first, name)),
            use_hints(&second, function(&second, name)),
            "use counts of {name}"
        );
    }

    let graph_first = callgraph::CallGraph::of_module(&first);
    let graph_second = callgraph::CallGraph::of_module(&second);
    let edges = |m: &Module, graph: &callgraph::CallGraph| {
        m.functions
            .iter()
            .map(|(id, f)| {
                let targets: Vec<String> = graph.callees[&id]
                    .iter()
                    .map(|t| m.functions[*t].name.clone())
                    .collect();
                (f.name.clone(), targets)
            })
            .collect::<BTreeMap<_, _>>()
    };
    assert_eq!(edges(&first, &graph_first), edges(&second, &graph_second));

    // Reading every output in arena order visits every function once: the order is the module's,
    // not a hash map's.
    let summaries = effect::of_module(&first);
    let counts = uses::of_module(&first);
    let graph = callgraph::CallGraph::of_module(&first);
    let all_ranges = ranges::analyze(&first).known().expect("ranges are total");
    for (id, _) in first.functions.iter() {
        assert!(summaries.contains_key(&id));
        assert!(counts.contains_key(&id));
        assert!(graph.callees.contains_key(&id));
        assert!(all_ranges.contains_key(&id));
    }
    assert_eq!(summaries.len(), first.functions.len());
    assert_eq!(all_ranges.len(), first.functions.len());
}

/// On a well-formed module every analysis answers `Known` with one entry per function (or per
/// local), and `Unknown` stays the default of a fact, not a value any of them returns.
#[test]
fn the_core_analyses_are_known_on_a_well_formed_module() {
    let m = module_in_order([0, 1, 2]);
    assert!(effect::analyze(&m).is_known());
    assert!(uses::analyze(&m).is_known());
    assert!(callgraph::analyze(&m).is_known());
    assert!(ranges::analyze(&m).is_known());

    let empty = Module::new();
    assert_eq!(effect::analyze(&empty), Fact::Known(Default::default()));
    assert_eq!(uses::analyze(&empty), Fact::Known(Default::default()));
    assert_eq!(callgraph::analyze(&empty), Fact::Known(Default::default()));
    assert_eq!(ranges::analyze(&empty), Fact::Known(Default::default()));

    // `Unknown` is the top and the default, not an empty value.
    let unknown: Fact<u32> = Fact::default();
    assert!(unknown.is_unknown());
    assert_eq!(unknown.known(), None);
}

// ---------------------------------------------------------------------------------------------
// Corpus smoke
// ---------------------------------------------------------------------------------------------

/// A handful of lowered codegen programs. They are included as source so the test is fast and
/// independent of the working directory.
const CORPUS: &[(&str, &str)] = &[
    (
        "array",
        include_str!("../../emela-codegen-c/tests/programs/array.emel"),
    ),
    (
        "closures",
        include_str!("../../emela-codegen-c/tests/programs/closures.emel"),
    ),
    (
        "data",
        include_str!("../../emela-codegen-c/tests/programs/data.emel"),
    ),
    (
        "string_concat",
        include_str!("../../emela-codegen-c/tests/programs/string_concat.emel"),
    ),
];

/// Runs every Core analysis on the corpus and checks that the maps are well formed: each key names
/// a function or a local of the module, the call graph is closed over the module, and no analysis
/// panics.
#[test]
fn the_core_analyses_smoke_the_codegen_corpus() {
    for (name, source) in CORPUS {
        let m = lower_files(&[("main.emel", source)]);

        let summaries = effect::analyze(&m)
            .known()
            .unwrap_or_else(|| panic!("{name}: the effect analysis is not total"));
        assert_eq!(
            summaries.len(),
            m.functions.len(),
            "{name}: every function has a summary"
        );
        for (id, _) in m.functions.iter() {
            assert!(summaries.contains_key(&id), "{name}: a function is missing");
        }

        let counts = uses::analyze(&m)
            .known()
            .unwrap_or_else(|| panic!("{name}: the use analysis is not total"));
        for (local, count) in &counts {
            assert!(
                (u32::from(local.into_raw()) as usize) < m.locals.len(),
                "{name}: a use count names a local outside the module"
            );
            assert!(count > &0, "{name}: a counted local has no use");
        }

        let graph = callgraph::analyze(&m)
            .known()
            .unwrap_or_else(|| panic!("{name}: the call graph is not total"));
        assert_eq!(
            graph.callees.len(),
            m.functions.len(),
            "{name}: every function is a node"
        );
        for (caller, callees) in &graph.callees {
            assert!((u32::from(caller.into_raw()) as usize) < m.functions.len());
            for callee in callees {
                assert!(
                    (u32::from(callee.into_raw()) as usize) < m.functions.len(),
                    "{name}: an edge names a function outside the module"
                );
            }
        }
        for (callee, callers) in &graph.callers {
            for caller in callers {
                assert!(
                    graph.callees[caller].contains(callee),
                    "{name}: a reverse edge has no forward edge"
                );
            }
        }

        let all_ranges = ranges::analyze(&m)
            .known()
            .unwrap_or_else(|| panic!("{name}: the range analysis is not total"));
        assert_eq!(
            all_ranges.len(),
            m.functions.len(),
            "{name}: every function has ranges"
        );
        for (id, ranges) in &all_ranges {
            assert!((u32::from(id.into_raw()) as usize) < m.functions.len());
            for local in ranges.ranges.keys() {
                assert!(
                    (u32::from(local.into_raw()) as usize) < m.locals.len(),
                    "{name}: a range names a local outside the module"
                );
            }
        }
    }
}

/// The union of two analyses over disjoint modules is not a fact about either one: this pins that
/// the maps are keyed by the arena ids of their own module, so a test cannot confuse a `FnId` of
/// one with another.
#[test]
fn a_fact_map_is_keyed_by_the_ids_of_its_own_module() {
    let a = module_in_order([0, 1, 2]);
    let mut b = Module::new();
    for name in ["other_one", "other_two"] {
        let f = b.declare(name, false);
        b.define(f, Vec::new(), TypeId::INT, Expr::Lit(Lit::Int(1)));
    }
    let summaries = effect::of_module(&a);
    for (id, _) in b.functions.iter() {
        // A `FnId` is an index into the module's arena, so an id beyond `a`'s arena is not one of
        // `a`'s functions even if the numeric value exists in the map's type.
        if (u32::from(id.into_raw()) as usize) < a.functions.len() {
            assert!(summaries.contains_key(&id));
        }
    }
    assert_eq!(summaries.len(), a.functions.len());
}
