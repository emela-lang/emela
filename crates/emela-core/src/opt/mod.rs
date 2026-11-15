//! Optimisation of the Core IR, between lowering and monomorphisation.
//!
//! The passes work on the target-neutral Core IR, so every target gets them. [`run`] is the fixed
//! sequence; there is no generic pass framework. A build turns all of them off together with
//! `--no-opt`, so the same source can be compiled with and without them and the observable
//! behaviour compared.
//!
//! Every pass preserves the observable behaviour of the program: evaluation order, effects,
//! failures, the order and count of the releases of `with`, and drop points. It may change memory
//! use and when a program runs out of memory (spec `[effect.handler]`, `[target.native]`). A pass
//! that cannot keep a contract gives up on that expression instead of changing it.
//!
//! # Pipeline
//!
//! [`run`] calls the passes in this order. A later pass may rely on an earlier one having run, and
//! says so in its rustdoc. Every pass has the shape `fn(&mut Module) -> PassStats` and fills only
//! the counters it measures; [`run`] merges them into one [`Stats`].
//!
//! | Pass | Counters | Added by |
//! |---|---|---|
//! | `option::option_or` | [`Stats::option_or`] | Option fusion |
//! | `option::array_get_match` | [`Stats::array_get_match`] | Option fusion |
//! | `range::fold` | [`Stats::range_fold`] | range fusion |
//! | `range::map` | [`Stats::range_map`] | range fusion |
//! | `inline::inline` | [`Stats::inline`] | inlining |
//!
//! The fusions come before inlining: they match the calls the source wrote, and inlining a
//! function such as `List.fold` first would dissolve the call the range fusion looks for. The
//! table is the order the passes run in.
//!
//! # Pass contract
//!
//! A pass's rustdoc carries these five lines:
//!
//! ```text
//! Preconditions: when the pass applies
//! Postconditions: what it guarantees (the invariants `verify` can show)
//! Preserves: evaluation order, effects, failure edges, drop points, evidence
//! Deopt: leave the expression alone when it reaches `Any`, a container, a closure capture, a
//!        saved frame or an external call
//! Measures: the benches and allocation counts it targets
//! ```
//!
//! In debug builds [`run`] checks the module with [`crate::verify`] after a pass, so a rewrite that
//! breaks typing fails where it was made.

use rustc_hash::FxHashMap;

use crate::ir::{Expr, FnId, Lit, Module};

mod inline;
mod option;
mod range;

/// A fn of the bundled core that a Core IR pass matches by identity, not by name.
///
/// Lowering records the IR function of every copy(`List.fold$suspend3` too) in the map it passes
/// to [`run`]. A user's fn with the same name is a different [`FnId`], so a pass never confuses
/// the two. Inlining excludes every function recorded here: the fusions match them, and copying
/// them into every caller would grow the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreFn {
    /// `Option.or`, which the Option fusion matches.
    OptionOr,
    /// `List.fold`, which the range fusion matches.
    ListFold,
    /// `List.map`, which the range fusion matches.
    ListMap,
    /// `List.range`, which the range fusion removes.
    ListRange,
    /// `List.reverse`, which the `List.map` range fusion calls once at the end.
    ListReverse,
    /// Another function of the core `List` module. Inlining leaves collection functions as calls:
    /// the range fusion matches some of them, and copying them into every caller would grow the
    /// output for no gain.
    Collection,
}

/// The bundled core functions lowering identified, by IR function ([`CoreFn`]).
pub type CoreFns = FxHashMap<FnId, CoreFn>;

/// What each Core IR pass changed in one [`run`], for tests and the differential harness.
///
/// The fields are added with the passes; a pass fills only its own. An empty `Stats` says that no
/// pass changed anything, which is what a test of a pass must not see on the corpus it runs on.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    /// `Option.or(Array.get(a, i), d)` rewritten to the internal `ArrayGetOr`.
    pub option_or: u32,
    /// `match Array.get(a, i) { Some(x) -> …; None -> … }` rewritten to a bounds test and a load.
    pub array_get_match: u32,
    /// `List.fold(List.range(lo, hi), init, f)` rewritten to a loop over an integer counter.
    pub range_fold: u32,
    /// `List.map(List.range(lo, hi), f)` rewritten to a loop that no longer builds the range's
    /// list (the result list is still built).
    pub range_map: u32,
    /// Calls inlined: direct calls replaced by a copy of the callee's body.
    pub inline: u32,
}

impl Stats {
    /// Adds what `other` changed to these.
    pub fn merge(&mut self, other: Stats) {
        self.option_or += other.option_or;
        self.array_get_match += other.array_get_match;
        self.range_fold += other.range_fold;
        self.range_map += other.range_map;
        self.inline += other.inline;
    }

    /// Whether no pass changed anything.
    pub fn is_empty(&self) -> bool {
        *self == Stats::default()
    }
}

/// What one pass changed: the same counters as [`Stats`], of which the pass fills only its own.
/// [`run`] merges what every pass returns.
pub type PassStats = Stats;

/// Runs the Core IR optimisation passes on `m` if `optimize`, and returns what they changed.
///
/// The sequence is the order in the pipeline table above; a later pass may rely on an earlier one
/// having run. `core_fns` is what lowering recorded about the bundled core functions (`CoreFn`),
/// by which the fusions recognise `Option.or` and the `List` functions. With `optimize` false the
/// module is left untouched and `Stats` is empty.
pub fn run(m: &mut Module, core_fns: &CoreFns, optimize: bool) -> Stats {
    if !optimize {
        return Stats::default();
    }
    let mut stats = Stats::default();
    stats.merge(option::option_or(m, core_fns));
    check(m);
    stats.merge(option::array_get_match(m, core_fns));
    check(m);
    stats.merge(range::fold(m, core_fns));
    check(m);
    stats.merge(range::map(m, core_fns));
    check(m);
    // Inlining comes last, for the reason in the pipeline docs.
    stats.merge(inline::inline(m, core_fns));
    check(m);
    stats
}

/// Applies `f` to the body of every function, with the module to allocate locals in.
///
/// A pass takes the body out of the function, so it can allocate locals in the module and rewrite
/// the body without borrowing the function.
pub(super) fn each_body(m: &mut Module, f: &mut impl FnMut(&mut Module, &mut Expr)) {
    let ids: Vec<FnId> = m.functions.iter().map(|(id, _)| id).collect();
    for id in ids {
        let mut body = std::mem::replace(&mut m.functions[id].body, Expr::Lit(Lit::Unit));
        f(m, &mut body);
        m.functions[id].body = body;
    }
}

/// Checks that a pass left the module well typed. A failure is a compiler bug: the pass broke its
/// postcondition, not the program.
fn check(m: &Module) {
    #[cfg(debug_assertions)]
    if let Err(errors) = crate::verify(m) {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!(
            "a Core IR optimisation pass produced ill-typed IR (a compiler bug):\n{}",
            lines.join("\n")
        );
    }
    #[cfg(not(debug_assertions))]
    let _ = m;
}

#[cfg(test)]
mod tests {
    use super::{PassStats, Stats};

    /// The total is the sum of what each pass returned: a counter belongs to exactly one pass.
    #[test]
    fn merge_adds_each_pass_counter() {
        let mut total = Stats::default();
        assert!(total.is_empty());

        total.merge(Stats {
            option_or: 1,
            array_get_match: 2,
            ..PassStats::default()
        });
        total.merge(PassStats {
            range_fold: 3,
            range_map: 4,
            inline: 5,
            ..PassStats::default()
        });

        assert_eq!(total.option_or, 1);
        assert_eq!(total.array_get_match, 2);
        assert_eq!(total.range_fold, 3);
        assert_eq!(total.range_map, 4);
        assert_eq!(total.inline, 5);
        assert!(!total.is_empty());
    }

    /// Merging an empty pass changes no counter, so a pass that does nothing is invisible.
    #[test]
    fn merging_an_empty_pass_keeps_the_total() {
        let mut total = Stats {
            option_or: 7,
            ..Stats::default()
        };
        total.merge(Stats::default());
        assert_eq!(
            total,
            Stats {
                option_or: 7,
                ..Stats::default()
            }
        );
    }
}
