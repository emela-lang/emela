//! The fusion laws: the registry, the per-law flags and the per-sweep budget.
//!
//! Fusion replaces a call of a bundled library function with the operation the call stands for
//! (plan §2.2). It is not deforestation of user functions: a law matches a call of a core function
//! [lowering recorded](crate::opt::CoreFn), by identity. The laws are data: [`laws`] returns the
//! [`Law`] descriptors in pipeline order, each with the contract of plan §3.4, so the order, the
//! contract and the run are one table instead of five separate calls. The bodies are in the
//! `option`, `list` and `string` submodules, beside the table, so that a law's contract and its
//! code are one entry and one function.
//!
//! # Registry order
//!
//! [`laws`] lists the implemented laws in the order the pipeline runs them, and then the reserved
//! laws where they will be inserted. [`run`] iterates that table, so the registry order *is* the
//! pipeline order; the order is fixed because
//!
//! - `option_or` and `array_get_match` match the same `Array.get` and do not overlap;
//! - `map_map`, `fold_map` and `filter_map` build the call (`List.map(xs, λx. g(f x))`,
//!   `List.fold(xs, init, λa, x. g(a, f x))`) that `range_fold` and `range_map` fuse next, so they
//!   come before the range laws and a range pipeline is fused completely in one sweep;
//! - `array_map_map` and `array_fold_map` build `Array.map`/`Array.fold` calls that no later law
//!   consumes, so they only have to come after the `List` pipeline laws, whose shape they mirror
//!   (D4/PR-031);
//! - `range_fold` and `range_map_array` remove a range in a different consumer;
//! - `range_map_array` must run before `range_map`, which would otherwise rewrite the
//!   `List.map` inside the `Array.from_list` that the array fusion is looking for.
//!
//! A reserved law has no body, so its position does not affect the pipeline yet; it is the place
//! its body will take when the builtin it needs exists.
//!
//! # Flags
//!
//! [`Laws`] has one flag per law. A law whose flag is off is skipped entirely: it does not
//! rewrite, does not run the verifier and contributes nothing to [`Stats`] — no change counter and
//! no [`Stats::deopt`]. [`Laws::default`] has the five laws the pipeline ran before the registry
//! on and the laws added later off: `string_concat` (D5/PR-032), the three pipeline laws
//! (`map_map`, `fold_map`, `filter_map`, D3/PR-030) and the two array pipeline laws
//! (`array_map_map`, `array_fold_map`, D4/PR-031) are implemented but new, so they are opt-in until
//! they have run on the corpus. [`Laws::all`] turns every flag on. A reserved law never runs, even
//! in [`Laws::all`]: it has no body.
//!
//! # Budget
//!
//! [`MAX_FUSIONS_PER_SWEEP`] caps the number of applications of all fusion laws in one sweep.
//! When the applications of the sweep reach the cap, [`run`] stops and every remaining law is
//! declined, which adds one to [`Stats::deopt`] per law. A deopt is not a rewrite, so it does not
//! keep the fixed-point loop going. The cap is per sweep: the next sweep gets the whole budget
//! again.
//!
//! The cap is enforced between laws, because a law body is atomic here; a law that is stopped by
//! the budget is one deopt, not one per site. The constant is far above what the corpus needs, so
//! it never fires in a default run: it is a guard against a law that matches more sites than
//! expected, and the budget tests call [`run_with_budget`] with a small cap to show that it stops
//! the sweep. The count of a law that overshoots the cap within its own body is the full count of
//! that law; the next law is the first one the cap stops.

use crate::ir::Module;
use crate::opt::{CoreFns, Stats};

mod array;
mod law;
mod list;
mod option;
mod string;

pub use law::{Law, LawId};

/// The most fusion applications one sweep may make, across all laws.
///
/// The cap is a guard, like `MAX_PASSES` in [`crate::opt`]: the corpus stays far below it, and a run
/// that reaches it would mean a law matches sites it should have declined. A sweep that reaches it
/// stops and counts each remaining law as a deopt (see the module documentation).
pub const MAX_FUSIONS_PER_SWEEP: u32 = 4096;

/// The fusion laws in pipeline order, for tests, reports and documentation.
///
/// A reserved law has [`Law::implemented`] false; it is listed so that its id and its contract
/// exist before its body does, and it is never run.
pub fn laws() -> &'static [Law] {
    law::LAWS
}

/// Which fusion laws [`run`] may run, one flag per law.
///
/// A law whose flag is off is skipped: it rewrites nothing, is not verified and adds nothing to
/// [`Stats`], not even a deopt. The reserved law ([`LawId::ArrayPipeline`]) has no body, so its
/// flag has no effect until the internal builtin it builds exists. `string_concat` (D5/PR-032),
/// the three pipeline laws of D3/PR-030 ([`Laws::map_map`], [`Laws::fold_map`],
/// [`Laws::filter_map`]) and the two array pipeline laws of D4/PR-031 ([`Laws::array_map_map`],
/// [`Laws::array_fold_map`]) have bodies but are off in [`Laws::default`], because they are new.
///
/// [`Laws::default`] turns on the five laws the pipeline ran before the registry;
/// [`Laws::none`] turns every law off; [`Laws::all`] turns every flag on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Laws {
    /// `Option.or(Array.get(a, i), d)` to `ArrayGetOr`.
    pub option_or: bool,
    /// `match Array.get(a, i) { … }` to a bounds test and a load.
    pub array_get_match: bool,
    /// `List.map(List.map(xs, f), g)` to one traversal. Off in [`Laws::default`] (D3/PR-030).
    pub map_map: bool,
    /// `List.fold(List.map(xs, f), init, g)` to one traversal. Off in [`Laws::default`]
    /// (D3/PR-030).
    pub fold_map: bool,
    /// `List.map(List.filter(xs, p), f)` to one traversal. Off in [`Laws::default`] (D3/PR-030).
    pub filter_map: bool,
    /// `Array.map(Array.map(a, f), g)` to one traversal. Off in [`Laws::default`] (D4/PR-031).
    pub array_map_map: bool,
    /// `Array.fold(Array.map(a, f), init, g)` to one traversal. Off in [`Laws::default`]
    /// (D4/PR-031).
    pub array_fold_map: bool,
    /// `List.fold(List.range(lo, hi), …)` to a loop over a counter.
    pub range_fold: bool,
    /// `Array.from_list(List.map(List.range(…), f))` to `ArrayMapRange`.
    pub range_map_array: bool,
    /// `List.map(List.range(lo, hi), f)` to a loop over a counter.
    pub range_map: bool,
    /// An `Array` pipeline over a range. Reserved (D4/PR-031).
    pub array_pipeline: bool,
    /// A chain of string concatenations to one allocation (D5/PR-032).
    pub string_concat: bool,
}

impl Laws {
    /// Every flag on, including the reserved laws (which have no body and never run).
    pub const fn all() -> Self {
        Self {
            option_or: true,
            array_get_match: true,
            map_map: true,
            fold_map: true,
            filter_map: true,
            array_map_map: true,
            array_fold_map: true,
            range_fold: true,
            range_map_array: true,
            range_map: true,
            array_pipeline: true,
            string_concat: true,
        }
    }

    /// Every flag off: [`run`] rewrites nothing and reports nothing.
    pub const fn none() -> Self {
        Self {
            option_or: false,
            array_get_match: false,
            map_map: false,
            fold_map: false,
            filter_map: false,
            array_map_map: false,
            array_fold_map: false,
            range_fold: false,
            range_map_array: false,
            range_map: false,
            array_pipeline: false,
            string_concat: false,
        }
    }

    /// Whether `id` is turned on. A reserved law can be on and still never run.
    pub const fn is_enabled(self, id: LawId) -> bool {
        match id {
            LawId::OptionOr => self.option_or,
            LawId::ArrayGetMatch => self.array_get_match,
            LawId::MapMap => self.map_map,
            LawId::FoldMap => self.fold_map,
            LawId::FilterMap => self.filter_map,
            LawId::ArrayMapMap => self.array_map_map,
            LawId::ArrayFoldMap => self.array_fold_map,
            LawId::RangeFold => self.range_fold,
            LawId::RangeMapArray => self.range_map_array,
            LawId::RangeMap => self.range_map,
            LawId::ArrayPipeline => self.array_pipeline,
            LawId::StringConcat => self.string_concat,
        }
    }
}

impl Default for Laws {
    /// The five laws the pipeline ran before the registry on, the newer laws and the reserved one
    /// off: `Options::default()` is the pipeline from before the registry. `string_concat`
    /// (D5/PR-032), the D3/PR-030 laws and the D4/PR-031 laws are implemented but new, so they are
    /// opt-in.
    fn default() -> Self {
        Self {
            option_or: true,
            array_get_match: true,
            map_map: false,
            fold_map: false,
            filter_map: false,
            array_map_map: false,
            array_fold_map: false,
            range_fold: true,
            range_map_array: true,
            range_map: true,
            array_pipeline: false,
            string_concat: false,
        }
    }
}

/// Runs the enabled fusion laws on `m`, in the registry order, and adds what they changed to
/// `stats`. Returns how many sites the laws rewrote.
///
/// A disabled law is skipped and contributes nothing; a reserved law is never run. The module is
/// verified after each law that runs, at the points the direct calls had before the registry.
/// The applications of the sweep are capped by [`MAX_FUSIONS_PER_SWEEP`]; [`run_with_budget`] takes
/// the cap as an argument.
pub fn run(m: &mut Module, core_fns: &CoreFns, laws: Laws, stats: &mut Stats) -> u32 {
    run_with_budget(m, core_fns, laws, stats, MAX_FUSIONS_PER_SWEEP)
}

/// [`run`] with an explicit cap on the applications of one sweep.
///
/// The budget tests use a small cap to show that reaching it stops the sweep: each enabled law the
/// cap stops adds one to [`Stats::deopt`], and the laws that already ran are kept. A budget of
/// zero runs no law, whatever the flags say.
pub fn run_with_budget(
    m: &mut Module,
    core_fns: &CoreFns,
    laws: Laws,
    stats: &mut Stats,
    budget: u32,
) -> u32 {
    let mut applied = 0;
    for law in self::laws() {
        let Some(body) = law.body() else {
            // Reserved: no body, whatever the flag says.
            continue;
        };
        if !laws.is_enabled(law.id) {
            continue;
        }
        if applied >= budget {
            stats.deopt += 1;
            continue;
        }
        let done = body(m, core_fns);
        applied += done.applied;
        stats.merge(done.stats);
        super::check(m);
    }
    applied
}
