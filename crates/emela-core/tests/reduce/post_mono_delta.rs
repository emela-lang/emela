//! Tests for the δ stage after monomorphisation (`emela_core::opt::reduce::delta`, second stage).
//!
//! Before monomorphisation a callee whose E or R still has a set type parameter is left as a call.
//! After monomorphisation every function is concrete, so an effect-polymorphic wrapper becomes a
//! candidate. The reserved bundled collection functions must stay calls in this stage too,
//! because lowering rebuilds the `CoreFns` identity map through `Monomorphized::origins`.
//!
//! The observable behaviour of the stage is checked on a real higher-order program by the mutation
//! pair `higher_order_fused.emel` / `higher_order_expanded.emel` in `emela-driver/tests/opt_diff.rs`.

use crate::support;

use emela_core::{Callee, Expr, FnId, Module, dump};
use support::{lower_files_mono_with_stats, lower_files_with_stats};

/// The functions whose IR name starts with `prefix`. A monomorphised copy has its type arguments
/// in the name (`List.reverse[Int]`), and a written function the entry module declares keeps its
/// plain name before monomorphisation (`apply`).
fn functions(m: &Module, prefix: &str) -> Vec<FnId> {
    m.functions
        .iter()
        .filter(|(_, f)| f.name.starts_with(prefix))
        .map(|(id, _)| id)
        .collect()
}

/// The one function whose name starts with `prefix`.
fn function(m: &Module, prefix: &str) -> FnId {
    let found = functions(m, prefix);
    assert_eq!(found.len(), 1, "functions named `{prefix}*`: {found:?}");
    found[0]
}

/// The number of direct calls to any function whose name starts with `prefix` in the body of
/// `owner`. Monomorphisation may produce more than one copy of a generic function.
fn calls_to_family_in(m: &Module, owner: FnId, prefix: &str) -> usize {
    functions(m, prefix)
        .into_iter()
        .map(|f| calls_in(m, owner, f))
        .sum()
}

/// The number of direct calls to `target` in `e`.
fn direct_calls(e: &Expr, target: FnId) -> usize {
    let mut count = match e {
        Expr::Call {
            callee: Callee::Direct { f, .. },
            ..
        } if *f == target => 1,
        _ => 0,
    };
    e.for_each_child(&mut |c| count += direct_calls(c, target));
    count
}

/// The number of direct calls to `target` in the body of `owner`.
fn calls_in(m: &Module, owner: FnId, target: FnId) -> usize {
    direct_calls(&m.functions[owner].body, target)
}

/// The number of direct calls to `target` anywhere in the module.
fn calls_anywhere(m: &Module, target: FnId) -> usize {
    m.functions
        .iter()
        .map(|(_, f)| direct_calls(&f.body, target))
        .sum()
}

/// An effect-polymorphic wrapper. Before monomorphisation `apply` has set type parameters, so δ
/// leaves its call; after monomorphisation its copy is concrete and the call is expanded.
const APPLY: &[(&str, &str)] = &[(
    "main.emel",
    "\
fn apply(f: fn(Int) -> Int, x: Int) -> Int {
  f(x)
}

fn main() {
  fs = List.map([1, 2, 3], |k| { |x| x + k })
  dbg(List.fold(fs, from: 0, f: |acc, f| acc + apply(f, 10)))
}
",
)];

/// Before monomorphisation the wrapper is left as a call: its E and R are set type parameters.
#[test]
fn an_effect_polymorphic_wrapper_is_left_before_monomorphisation() {
    let (m, stats) = lower_files_with_stats(APPLY, true);
    assert_eq!(stats.delta, 0, "{stats:?}");
    let apply = function(&m, "apply");
    assert_eq!(calls_anywhere(&m, apply), 1, "{}", dump(&m));
}

/// After monomorphisation the same call is expanded: the copy has no set type parameters, no
/// failure and no suspension, and its body is small.
#[test]
fn an_effect_polymorphic_wrapper_is_inlined_after_monomorphisation() {
    let (m, stats) = lower_files_mono_with_stats(APPLY, "c", true);
    assert_eq!(stats.delta, 1, "{stats:?}\n{}", dump(&m));
    let apply = function(&m, "apply");
    assert_eq!(calls_anywhere(&m, apply), 0, "{}", dump(&m));
}

/// `--no-opt` keeps the wrapper call; monomorphisation alone does not inline.
#[test]
fn no_opt_keeps_the_wrapper_after_monomorphisation() {
    let (m, stats) = lower_files_mono_with_stats(APPLY, "c", false);
    assert!(stats.is_empty(), "{stats:?}");
    let apply = function(&m, "apply");
    assert_eq!(calls_anywhere(&m, apply), 1, "{}", dump(&m));
}

/// The bundled collection functions are recorded by identity. Their monomorphised copies have
/// new function ids, so the stage's `CoreFns` map has to be rebuilt through
/// `Monomorphized::origins`; an empty map would let δ copy `List.map` and `List.reverse` into
/// `main`. Their bodies are small enough to be candidates without that map.
#[test]
fn reserved_collection_functions_are_not_inlined_after_monomorphisation() {
    let files = [(
        "main.emel",
        "\
fn main() {
  ys = List.map([1, 2, 3], |x| x + 1)
  dbg(List.reverse(ys))
}
",
    )];
    let (m, stats) = lower_files_mono_with_stats(&files, "c", true);
    // Other small functions (the prelude's and the std's) may be expanded in both stages; the
    // reserved collection functions are the calls this test is about.
    assert!(stats.delta > 0, "{stats:?}");
    let main = function(&m, "main");
    for name in ["List.map", "List.reverse"] {
        assert_eq!(calls_to_family_in(&m, main, name), 1, "{}", dump(&m));
    }
}
