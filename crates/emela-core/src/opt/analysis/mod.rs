//! Analyses of the Core IR, shared by the rules that need them.
//!
//! An analysis answers a question about the module (`which functions may fail`, `where is a local
//! used`, `which calls are in a cycle`, `what range may a local hold`) without changing it. The
//! reduction rules and the later passes ask for a fact instead of walking the IR again.
//!
//! # Contract
//!
//! ```text
//! Analysis ID: see [`AnalysisId`]
//! Domain / lattice: each analysis documents its own; every fact has a top element `Unknown`
//! Transfer: each analysis documents how one expression or function is summarised
//! Soundness: a `Known` fact is true. `Unknown` means the analysis could not prove anything;
//!            it never means "none" or "pure".
//! Deopt (unknown): a consumer that gets `Unknown` must leave the expression it wanted to rewrite
//!                  alone and count the decision as a deopt. It must not fall back to a default
//!                  that assumes the property (for example "no effects" or "cannot fail").
//! Consumers: the rules and later passes named in the module documentation of each analysis
//! ```
//!
//! # Status
//!
//! B0/PR-002 fixes the API. Every analysis in [`effect`], [`uses`], [`callgraph`] and [`ranges`]
//! currently returns `Unknown`; B1/PR-020, B2/PR-021 and B5/PR-024 fill them.

use std::any::Any;

use rustc_hash::FxHashMap;

use crate::ir::Module;

pub mod callgraph;
pub mod effect;
pub mod ranges;
pub mod uses;

/// A fact an analysis may or may not know.
///
/// `Unknown` is the top of every analysis's lattice and the default. It is not an "empty" or
/// "negative" fact: a consumer must treat it as "no answer" and deopt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fact<T> {
    /// The analysis proved the value.
    Known(T),
    /// The analysis has no answer.
    Unknown,
}

impl<T> Fact<T> {
    /// Whether the fact is `Known`.
    pub fn is_known(&self) -> bool {
        matches!(self, Fact::Known(_))
    }

    /// Whether the fact is `Unknown`.
    pub fn is_unknown(&self) -> bool {
        matches!(self, Fact::Unknown)
    }

    /// The value, if the analysis proved it.
    pub fn as_known(&self) -> Option<&T> {
        match self {
            Fact::Known(value) => Some(value),
            Fact::Unknown => None,
        }
    }

    /// The value, consuming the fact.
    pub fn known(self) -> Option<T> {
        match self {
            Fact::Known(value) => Some(value),
            Fact::Unknown => None,
        }
    }
}

impl<T> Default for Fact<T> {
    /// The conservative default: no answer.
    fn default() -> Self {
        Fact::Unknown
    }
}

/// What a concrete analysis computes, and the key under which [`Cache`] keeps it.
///
/// An analysis does not rewrite the module. A consumer asks for its output, stores it in the cache
/// with [`Analysis::ID`], and invalidates the cache entry after a rewrite that could change the
/// answer.
pub trait Analysis {
    /// The facts the analysis computes.
    type Output;

    /// The identity of the analysis in a [`Cache`] and in deopt reports.
    const ID: AnalysisId;

    /// Computes the analysis of `m`.
    ///
    /// Returning `Unknown` (inside the output) is the rule, not an error: the default is
    /// conservative until the analysis is implemented.
    fn analyze(&self, m: &Module) -> Self::Output;
}

/// The analyses of the Core IR.
///
/// The variants are stable identifiers for caches and for the statistics a rule reports. A new
/// analysis adds a variant; it does not change an existing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnalysisId {
    /// Effects, failures, suspensions and defects of a function (`effect`).
    Effect,
    /// Use counts of locals (`uses`).
    Uses,
    /// Calls between functions and their strongly connected components (`callgraph`).
    CallGraph,
    /// Ranges of integer locals and induction variables (`ranges`).
    Ranges,
}

struct Entry {
    /// The generation the value was computed in. A value from an older generation is stale even
    /// if it is still in the map, because an invalidation does not have to remove every entry.
    generation: u64,
    value: Box<dyn Any>,
}

/// A cache of analysis results, keyed by [`AnalysisId`].
///
/// The cache is invalidated on request: [`Cache::invalidate`] drops one analysis and
/// [`Cache::invalidate_all`] drops every result and starts a new generation. A consumer that
/// rewrites the IR calls [`Cache::invalidate_all`] (or the narrower invalidation when only one
/// analysis is affected), so no rule reads a fact that described the module before the rewrite.
///
/// The values are type-erased with [`Any`]. A getter asks for the concrete type the analysis
/// produces; a wrong type or a stale entry returns `None`, and the caller recomputes.
#[derive(Default)]
pub struct Cache {
    generation: u64,
    entries: FxHashMap<AnalysisId, Entry>,
}

impl Cache {
    /// An empty cache at generation zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// The current generation. It changes at every [`Cache::invalidate_all`].
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The cached value of `id`, if it was computed in the current generation and has type `T`.
    pub fn get<T: 'static>(&self, id: AnalysisId) -> Option<&T> {
        let entry = self.entries.get(&id)?;
        if entry.generation != self.generation {
            return None;
        }
        entry.value.downcast_ref::<T>()
    }

    /// Stores the value of `id` for the current generation.
    pub fn insert<T: 'static>(&mut self, id: AnalysisId, value: T) {
        self.entries.insert(
            id,
            Entry {
                generation: self.generation,
                value: Box::new(value),
            },
        );
    }

    /// Removes the cached value of `id`.
    pub fn invalidate(&mut self, id: AnalysisId) {
        self.entries.remove(&id);
    }

    /// Removes every cached value and starts a new generation.
    pub fn invalidate_all(&mut self) {
        self.generation += 1;
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Module;

    /// The default of every fact is the top element, not a negative answer.
    #[test]
    fn the_default_fact_is_unknown() {
        let fact: Fact<u32> = Fact::default();
        assert!(fact.is_unknown());
        assert!(!fact.is_known());
        assert_eq!(fact.known(), None);
    }

    /// The stub analyses are conservative: they answer `Unknown`, not an empty summary.
    #[test]
    fn every_stub_analysis_is_unknown() {
        let m = Module::new();
        assert!(effect::analyze(&m).is_unknown());
        assert!(uses::analyze(&m).is_unknown());
        assert!(callgraph::analyze(&m).is_unknown());
        assert!(ranges::analyze(&m).is_unknown());
    }

    /// A cached fact is returned until it is invalidated on its own.
    #[test]
    fn invalidating_one_analysis_drops_only_it() {
        let mut cache = Cache::new();
        cache.insert(AnalysisId::Effect, 7u32);
        cache.insert(AnalysisId::Uses, 11u32);
        assert_eq!(cache.get::<u32>(AnalysisId::Effect), Some(&7));

        cache.invalidate(AnalysisId::Effect);
        assert_eq!(cache.get::<u32>(AnalysisId::Effect), None);
        assert_eq!(
            cache.get::<u32>(AnalysisId::Uses),
            Some(&11),
            "an unrelated analysis stays cached"
        );
    }

    /// Invalidating everything starts a new generation and drops every result. An entry that was
    /// inserted in the old generation is never returned, even if the map still held it.
    #[test]
    fn invalidating_all_drops_every_entry() {
        let mut cache = Cache::new();
        let generation = cache.generation();
        cache.insert(AnalysisId::Ranges, 3u32);
        assert_eq!(cache.get::<u32>(AnalysisId::Ranges), Some(&3));

        cache.invalidate_all();
        assert_ne!(cache.generation(), generation);
        assert_eq!(cache.get::<u32>(AnalysisId::Ranges), None);
        assert_eq!(cache.get::<u32>(AnalysisId::Effect), None);
    }

    /// A getter of the wrong type is a miss, not a panic: the caller recomputes.
    #[test]
    fn a_value_of_another_type_is_a_miss() {
        let mut cache = Cache::new();
        cache.insert(AnalysisId::CallGraph, 1u32);
        assert_eq!(cache.get::<u64>(AnalysisId::CallGraph), None);
        assert_eq!(cache.get::<u32>(AnalysisId::CallGraph), Some(&1));
    }
}
