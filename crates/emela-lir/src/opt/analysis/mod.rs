//! Analyses of the LIR, shared by the passes that need them.
//!
//! The analyses answer questions about the Flow, Closure or Heap IR without changing it. The
//! escape analysis lives in [`escape`], and `crate::opt::escape` is the crate-internal façade SRA
//! calls, so there is one implementation and not two.
//!
//! # Contract
//!
//! ```text
//! Analysis ID: see [`AnalysisId`]
//! Domain / lattice: each analysis documents its own; every fact has a top element `Unknown`
//! Transfer: each analysis documents how one value, statement or function is summarised
//! Soundness: a `Known` fact is true. `Unknown` means the analysis could not prove anything; it
//!            never means "does not escape", "not live" or "not owned".
//! Deopt (unknown): a consumer that gets `Unknown` leaves the function or value alone and counts
//!                  the decision as a deopt. It must not read `Unknown` as a negative fact.
//! Consumers: the passes named in the module documentation of each analysis
//! ```
//!
//! # Status
//!
//! B0/PR-002 fixes the API. The escape analysis (`escape`) and its parameter summary are
//! implemented (B3/PR-022), liveness (B4/PR-023) from `Stage::DefectsExplicit` on, and the borrow
//! and ownership facts (B6/PR-024). Borrow facts need no liveness and answer at every stage;
//! ownership facts use liveness and the borrow facts, so they are `Unknown` before
//! `Stage::DefectsExplicit`. The existing `crate::opt::escape::escaping` is the same
//! implementation behind the façade SRA calls.

use std::any::Any;

use rustc_hash::FxHashMap;

use crate::ir::Module;

pub mod borrow;
pub mod escape;
pub mod liveness;
pub mod ownership;

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
pub trait Analysis {
    /// The facts the analysis computes.
    type Output;

    /// The identity of the analysis in a [`Cache`] and in deopt reports.
    const ID: AnalysisId;

    /// Computes the analysis of `m`, or returns `Unknown`.
    fn analyze(&self, m: &Module) -> Self::Output;
}

/// The analyses of the LIR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnalysisId {
    /// Which values leave a function (`escape`).
    Escape,
    /// Which parameters a function may keep (`escape::parameter_summary`).
    ParameterEscape,
    /// Which variables are live after a statement (`liveness`).
    Liveness,
    /// Which parameters a function may borrow (`borrow`).
    Borrow,
    /// Which references are uniquely owned (`ownership`).
    Ownership,
}

struct Entry {
    /// The generation the value was computed in. A value from an older generation is stale even
    /// if it is still in the map.
    generation: u64,
    value: Box<dyn Any>,
}

/// A cache of analysis results, keyed by [`AnalysisId`].
///
/// [`Cache::invalidate`] drops one analysis and [`Cache::invalidate_all`] drops every result and
/// starts a new generation, so no pass reads a fact that described a module before a rewrite.
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

    fn module() -> Module {
        Module::new(emela_core::Module::new())
    }

    /// The default of every fact is the top element, not a negative answer.
    #[test]
    fn the_default_fact_is_unknown() {
        let fact: Fact<u32> = Fact::default();
        assert!(fact.is_unknown());
        assert!(!fact.is_known());
        assert_eq!(fact.known(), None);
    }

    /// Each analysis answers where it is defined; liveness and ownership are conservative before
    /// the Heap IR stage, and an empty module still has borrow facts.
    #[test]
    fn the_analyses_answer_where_they_are_defined() {
        let m = module();
        assert!(escape::analyze(&m).is_known());
        assert!(escape::parameter_summary(&m).is_known());
        assert!(
            liveness::analyze(&m).is_unknown(),
            "liveness is defined from DefectsExplicit on"
        );
        assert!(borrow::analyze(&m).is_known());
        assert!(
            ownership::analyze(&m).is_unknown(),
            "ownership needs liveness"
        );
    }

    /// Borrow and ownership answer on a well-formed Heap IR module and are conservative on a
    /// module whose call graph does not resolve.
    #[test]
    fn borrow_and_ownership_are_unknown_only_on_a_malformed_module() {
        use crate::{Const, FnBuilder, Stage, Stmt, Term, Value};

        let core = emela_core::Module::new();
        let mut b = FnBuilder::new(&core, "f");
        b.set_ret(emela_core::TypeId::INT);
        let x = b.var("x", emela_core::TypeId::INT);
        b.stmt(Stmt::Let {
            dst: Some(x),
            value: Value::Const(Const::Int(1)),
            on_defect: None,
            on_suspend: None,
        });
        b.term(Term::Return(Some(x.into())));
        let f = b.finish();

        let mut well_formed = Module::new(core);
        well_formed.add(f);
        well_formed.stage = Stage::RefCounted;
        assert!(borrow::analyze(&well_formed).is_known());
        assert!(ownership::analyze(&well_formed).is_known());

        let mut malformed = Module::new(emela_core::Module::new());
        malformed.declare("declared_but_never_defined");
        malformed.stage = Stage::RefCounted;
        assert!(borrow::analyze(&malformed).is_unknown());
        assert!(ownership::analyze(&malformed).is_unknown());
    }

    /// A cached fact is returned until it is invalidated on its own.
    #[test]
    fn invalidating_one_analysis_drops_only_it() {
        let mut cache = Cache::new();
        cache.insert(AnalysisId::Escape, 7u32);
        cache.insert(AnalysisId::Liveness, 11u32);
        assert_eq!(cache.get::<u32>(AnalysisId::Escape), Some(&7));

        cache.invalidate(AnalysisId::Escape);
        assert_eq!(cache.get::<u32>(AnalysisId::Escape), None);
        assert_eq!(
            cache.get::<u32>(AnalysisId::Liveness),
            Some(&11),
            "an unrelated analysis stays cached"
        );
    }

    /// Invalidating everything starts a new generation and drops every result.
    #[test]
    fn invalidating_all_drops_every_entry() {
        let mut cache = Cache::new();
        let generation = cache.generation();
        cache.insert(AnalysisId::Ownership, 3u32);
        assert_eq!(cache.get::<u32>(AnalysisId::Ownership), Some(&3));

        cache.invalidate_all();
        assert_ne!(cache.generation(), generation);
        assert_eq!(cache.get::<u32>(AnalysisId::Ownership), None);
        assert_eq!(cache.get::<u32>(AnalysisId::Escape), None);
    }

    /// A getter of the wrong type is a miss, not a panic.
    #[test]
    fn a_value_of_another_type_is_a_miss() {
        let mut cache = Cache::new();
        cache.insert(AnalysisId::Borrow, 1u32);
        assert_eq!(cache.get::<u64>(AnalysisId::Borrow), None);
        assert_eq!(cache.get::<u32>(AnalysisId::Borrow), Some(&1));
    }
}
