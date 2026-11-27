//! The array pipeline laws: map after map, and a fold after a map (D4/PR-031).
//!
//! `Std.Array.map` and `Std.Array.fold` are bundled std functions written in Emela. Their call
//! shape is the one the `List` pipeline laws match — four type arguments (the element type, the
//! result or accumulator type and the two set parameters), the array first and the evidence table
//! last — so the two laws are the `List` ones with [`CoreFn::ArrayMap`] and [`CoreFn::ArrayFold`]
//! in place of [`CoreFn::ListMap`] and [`CoreFn::ListFold`]. The bodies are in [`super::list`],
//! beside the shapes; the two functions here are the law entries, so the registry's table and the
//! contract's prose stay beside the counter each law fills.
//!
//! # Why an array law is not a list law
//!
//! The two traversals the laws remove build an *array* between them. `Array.map` builds a list of
//! the results and calls `Array.from_list` on it, so the intermediate value is one allocation of a
//! flat block of elements plus the list it was built from; `Array.fold` then walks the second
//! array. Composing the functions removes both traversals of the intermediate array. A list
//! pipeline law cannot see the calls, because they are calls of different bundled functions.
//!
//! # Representation
//!
//! Lowering records `Std.Array.map` and `Std.Array.fold` as [`CoreFn::ArrayMap`] and
//! [`CoreFn::ArrayFold`] ([`crate::lower`]'s `core_fn`). Both functions are marked as failing in
//! the IR — their function parameter is effect-polymorphic, so a call to the parameter may fail —
//! and δ never copies a function that fails, so recording the identities changes no other pass's
//! decision: the call stays a call, and the laws can match it by identity, never by name.
//!
//! `Std.Map` has no `map` and no law here. It has `map_values`, whose call shape is a different
//! one (three value type parameters, a three-parameter fold step), and it rebuilds a tree rather
//! than a flat block; a fold after it is a separate law with its own contract, not this shape.
//!
//! # Registry entries
//!
//! These are the bodies of the registry entries [`LawId::ArrayMapMap`] and
//! [`LawId::ArrayFoldMap`], in that registry order; the registry's flags, order and budget are in
//! [`super`], and the contract text of each law is beside its entry in [`super::law`].

use crate::ir::Module;
use crate::opt::{CoreFn, CoreFns, PassStats};

use super::LawRun;
use super::list::{compose_all, compose_fold_over, compose_map_over};

/// Rewrites `Array.map(Array.map(a, f), g)` to `Array.map(a, λx. g(f x))`.
///
/// Preconditions: the callee is the bundled `Std.Array.map` ([`CoreFn::ArrayMap`]) with its four
/// type arguments and three arguments (`a`, `f`, `ev`), and its `a` argument is a direct call of
/// the same function in the same shape; neither call fails or suspends. `f` and `g` are a lambda
/// literal or a reference to a top-level function, their instantiated types are pure (no failure,
/// no suspension, no evidence table), and their bodies may neither use an effect nor fail nor
/// suspend. At most one of `f` and `g` may reach a defect or diverge. The site is within the
/// code-size budget.
/// Postconditions: the result is the array of `g(f(x))` for each element `x` of `a`, in the order
/// of `a`. `f` and `g` are called once per element, `f` before `g`; the intermediate array is not
/// made. The fused call keeps the outer call's type arguments and evidence argument, so it is the
/// same `Array.map` call the source's outer call was.
/// Preserves: evaluation order, effects, failures, suspension points and drop points. `f` and `g`
/// are quiet, so the per-element interleaving `g(f x)` has the same observable behaviour as the
/// two traversals; at most one of them may stop the program (a defect or a divergence), so the
/// first stop is the same call of the same function. `a` is still evaluated first and the evidence
/// argument last; a lambda literal is made once before the call instead of before its own
/// traversal, which is not observable because making a lambda has no effect. The inner call's
/// evidence argument (a local or `()`) is dropped with the inner call.
/// Deopt: any other shape (a user's `map`, an indirect call, other type arguments or arguments);
/// a function value that is neither a lambda literal nor a reference to a top-level function; a
/// `dbg` or any other effect, failure or suspension in `f` or `g`; two steps that may each stop
/// the program; a site over the code-size budget. A deopt at a nested site is counted in
/// `Stats::deopt`.
/// Measures: `Stats::array_map_map`; the array pipelines of a program that maps an array twice (the
/// intermediate array, which is one flat allocation plus the list `Array.map` builds it from).
pub(super) fn map_map(m: &mut Module, core_fns: &CoreFns) -> LawRun {
    let run = compose_all(m, core_fns, |m, e, core_fns, summaries| {
        compose_map_over(m, e, core_fns, summaries, CoreFn::ArrayMap)
    });
    LawRun {
        applied: run.applied,
        stats: PassStats {
            array_map_map: run.applied,
            deopt: run.deopt,
            ..PassStats::default()
        },
    }
}

/// Rewrites `Array.fold(Array.map(a, f), init, g)` to
/// `Array.fold(a, init, λacc, x. g(acc, f x))`.
///
/// Preconditions: the callee is the bundled `Std.Array.fold` ([`CoreFn::ArrayFold`]) with its four
/// type arguments and four arguments (`a`, `from`, `f`, `ev`), and its `a` argument is a direct
/// call of `Std.Array.map` ([`CoreFn::ArrayMap`]) in the shape above; neither call fails or
/// suspends. `f` and `g` are a lambda literal or a reference to a top-level function, their
/// instantiated types are pure, and their bodies may neither use an effect nor fail nor suspend.
/// Among `f`, `g` and `init`, at most one may stop the program. The site is within the code-size
/// budget.
/// Postconditions: the result is the last accumulator of folding `g(acc, f(x))` over `a`, or `init`
/// when `a` is empty. `f` and `g` are called once per element, `f` before `g`, in the order of
/// `a`; the intermediate array is not made. The fold keeps its accumulator type, its type
/// arguments and its evidence argument.
/// Preserves: evaluation order, effects, failures, suspension points and drop points. `f` and `g`
/// are quiet, so the interleaving of their calls is not observable. `init` moves from after the
/// whole map to the fold's first argument; that is not observable because at most one of `init`,
/// `f` and `g` may produce an event or stop the program, so no event of another one can be
/// overtaken. `a` is still evaluated first and the evidence argument last. The inner call's
/// evidence argument (a local or `()`) is dropped with the inner call.
/// Deopt: the same refusals as [`map_map`], with `Std.Array.fold` as the outer call.
/// Measures: `Stats::array_fold_map`; the fold pipelines of a program that folds a mapped array
/// (the intermediate array, which is one flat allocation plus the list `Array.map` builds it
/// from).
pub(super) fn fold_map(m: &mut Module, core_fns: &CoreFns) -> LawRun {
    let run = compose_all(m, core_fns, |m, e, core_fns, summaries| {
        compose_fold_over(
            m,
            e,
            core_fns,
            summaries,
            CoreFn::ArrayMap,
            CoreFn::ArrayFold,
        )
    });
    LawRun {
        applied: run.applied,
        stats: PassStats {
            array_fold_map: run.applied,
            deopt: run.deopt,
            ..PassStats::default()
        },
    }
}
