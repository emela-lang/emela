//! Optimisation of the low IR (LIR), after the Core IR and before code generation.
//!
//! Three stages take part, because they see different things:
//!
//! - [`run_flow`] works on the Flow IR, after `from_core` and before closure conversion, where
//!   closures are still nested and values are still variables.
//! - [`run_mid`] works right after closure conversion, where a closure is a lifted function and a
//!   [`Value::MakeClosure`](crate::ir::Value::MakeClosure) record, and before the state machines
//!   make suspension points explicit. SRA and closure devirtualisation run here, because after
//!   `state_machines` a rewrite would have to change the saved variables of a frame
//!   ([`Suspend::saved`](crate::ir::Suspend::saved)) as well.
//! - [`run_heap`] works on the Heap IR, after reference counting has inserted `dup` and `drop`,
//!   where every allocation and every reference is explicit. `rc_cleanup` runs here.
//!
//! All are called by [`crate::to_heap_with`], so a build switches all of them off together with
//! `--no-opt`. There is no generic pass framework: each stage runs a fixed sequence. The stage
//! wrappers with options ([`run_mid_with`], [`run_heap_with`]) run the same sequence with
//! individual LIR passes turned off.
//!
//! Every pass preserves the observable behaviour of the program: evaluation order, effects,
//! failures, the order and count of the releases of `with`, and drop points. It may change memory
//! use and when a program runs out of memory. A pass that cannot keep its contract gives up on
//! that function instead of changing it.
//!
//! The analyses a pass needs are shared and separately tested: [`analysis`] is the API for the
//! escape, liveness, borrow and ownership facts. [`analysis::escape`] implements the intraprocedural
//! escape sets and the interprocedural parameter summary; [`escape`] is the crate-internal façade
//! SRA calls, and `crate::heap::live` is the current implementation of liveness. Each analysis
//! documents its domain, transfer, soundness and the deopt a consumer must take when it answers
//! `Unknown`. Each pass documents its preconditions, postconditions, what it preserves, when it
//! deopts, and what it measures (parent plan §3.4).
//!
//! | Pass | Counter | Stage | Status |
//! |---|---|---|---|
//! | [`analysis`] (`escape`, `liveness`, `borrow`, `ownership`) | — | every stage | `escape` and its parameter summary implemented (B3/PR-022); the others answer `Unknown` until B4/PR-023 and B6/PR-024 |
//! | borrow inference | [`Stats::borrow_functions`], [`Stats::borrow_params`], [`Stats::borrow_args`] | between `state_machines` and `insert_rc` | [`LirRules::borrow`], on by default (C2/PR-026). Not a rewrite: it writes the calling convention reference counting follows |
//! | [`sra`] | [`Stats::sra_tuples`], [`Stats::sra_records`] | [`run_mid`] | scalar tuples; scalar records behind [`LirRules::sra_records`], off by default (C5/PR-043) |
//! | [`devirt`] | [`Stats::devirt`] | [`run_mid`] | closure records only called in their function (E5/PR-037), on by default |
//! | [`closure_sra`] | [`Stats::sra_closures`] | [`run_mid`] | scalar captures of a closure record that E5 made direct, behind [`LirRules::sra_closures`], on by default (C5/PR-043) |
//! | [`rc`] cleanup | [`Stats::rc_pairs`], [`Stats::rc_borrow_pairs`] | [`run_heap`] | `dup`/`drop` pairs, with the local-base borrow elision on by default ([`LirRules::rc_borrow`], C1/PR-025) |
//! | drop specialisation | [`Stats::drop_specialized`], [`Stats::drop_deopts`] | [`run_heap`] | a type-directed block drop for reference-free values behind [`LirRules::drop_spec`], opt-in (C3/PR-027) until its measured size cost pays for a runtime gain |
//! | reuse / FBIP | [`Stats::reuse`], [`Stats::reuse_deopts`] | [`run_heap`] | a uniquely owned constructor rewritten in place, its old reference fields released first, behind [`LirRules::reuse`], off by default (C4/PR-042) |
//! | [`dce`] | [`Stats::dce`] | [`run_heap`] | dead pure `let`s with scalar destinations, on by default (C7/PR-028) |
//! | [`switch`] | [`Stats::switch_cases_merged`], [`Stats::switch_direct`] | [`run_heap`] | canonical switch cases, shared bodies and direct bodies behind [`LirRules::switch`], on by default (E4) |
//! | [`frame_prune`] | — | — | read-only analysis (C7/PR-028): the corpus has no stale saved slot, so no pass prunes a frame |
//! | reachability pruning | — | after [`run_heap`], in the backends | remove functions that [`crate::analysis::reach`] neither calls nor references, behind [`LirRules::prune`]; off by default (E3) |
//!
//! A row marked "not implemented" has no code and no flag yet; its pull request adds both. The
//! analyses are not passes and never change the module.
//!
//! Frame pruning is deliberately not part of [`dce`]: removing an entry of
//! [`Suspend::saved`](crate::ir::Suspend::saved) changes what a frame owns and whether a lifted
//! function's environment is borrowed, so it needs an ownership proof of its own. [`frame_prune`]
//! asks whether such an entry exists at all, and the answer on the local corpus is no: the saved
//! set is fixed before reference counting, `rc` never removes a pair whose variable a frame
//! saves, and [`dce`] removes no statement on the corpus, so no saved entry loses its last use
//! after the value. No flag turns a frame-pruning pass on because there is none; the analysis
//! documents the obstacle and `tests/frame_prune.rs` asserts the zero.
//!
//! # Feature flags
//!
//! [`LirRules`] has one flag per LIR pass that exists today ([`sra`] with its record half, its
//! [`devirt`] extension and its closure half ([`LirRules::sra_closures`], [`closure_sra`]), the
//! `rc` cleanup with its local-base borrow elision ([`LirRules::rc_borrow`]), [`dce`], drop
//! specialisation ([`LirRules::drop_spec`], [`drop_spec`]), borrow inference
//! ([`LirRules::borrow`], run by
//! [`to_heap_with_options`](crate::to_heap_with_options) between `state_machines` and `insert_rc`),
//! reuse ([`LirRules::reuse`], [`reuse`]), and switch normalisation ([`LirRules::switch`],
//! [`switch`]), and [`Options`] holds them. [`run_mid_with`] and [`run_heap_with`] take the
//! options; [`run_mid`] and [`run_heap`] use [`Options::default`], which runs the M3 memory passes
//! that pay for themselves — SRA, the `rc` cleanup with the local-base borrow elision, DCE and
//! borrow inference — and the switch normalisation, which only joins identical bodies or removes
//! a dispatch whose arms are all one body.
//! Drop specialisation stays opt-in: with it on, the default build's native binary grew by up to
//! 7.5% and its C source by up to 3.7% in the M3 measurement, with no measured runtime gain, and
//! the plan requires a reason and approval for a size increase over 2%. Reuse is off by default
//! too: it is the first, conservative slice of C4, and its output must stay what it was until a
//! later change turns it on. The flags are a programmatic API only: no command-line flag is added.
//! Record SRA, closure devirtualisation and closure SRA were off by default until M5/M6. That is
//! now a default change, not a rule change: closure devirtualisation and closure SRA run by default
//! because the `effects` benchmark measured 0.892–0.906x on WASM and 0.849–0.852x on C with both
//! on, the WASM output of the other 16 benchmark cases was byte-identical, and the passes are
//! covered end to end by `tests/closure_sra.rs`. Frame pruning has no pass and no flag:
//! [`frame_prune`] reports no stale saved slot on the corpus.
//!
//! [`LirRules::sra_records`] is off by default so that a default build's output, the WASM byte pins
//! and the differential tests stay what they were. It only takes part when [`LirRules::sra`] is on
//! too. [`LirRules::sra_closures`] only takes part when [`LirRules::devirt`] made the calls direct:
//! it removes the record E5 left as the environment of its own calls.

pub mod analysis;
pub mod closure_sra;
pub mod dce;
pub mod devirt;
pub mod drop_spec;
pub(crate) mod escape;
pub mod frame_prune;
mod rc;
pub mod reuse;
mod sra;
pub mod switch;

#[cfg(test)]
use std::cell::RefCell;

use crate::ir::{Module, Stage};

/// What each LIR pass changed in one run, for tests and the differential harness.
///
/// The fields are added with the passes. An empty `Stats` says that no pass changed anything,
/// which is what a test of a pass must not see on the corpus it runs on.
#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    /// Non-escaping tuples replaced by their fields (SRA).
    pub sra_tuples: u32,
    /// Non-escaping records replaced by their fields (SRA, C5/PR-043).
    pub sra_records: u32,
    /// Record candidates SRA left alone (a deopt), one per `make` bound by a `let`.
    pub sra_deopts: u32,
    /// Closure records replaced by their captures (closure SRA, C5/PR-043), one per record.
    pub sra_closures: u32,
    /// Closure records left alone (a deopt), one per `make_closure` bound by a `let`.
    pub sra_closure_deopts: u32,
    /// `dup`/`drop` pairs removed by [`rc`], one per duplicated reference.
    pub rc_pairs: u32,
    /// Indirect calls made direct by [`devirt`], one per call.
    pub devirt: u32,
    /// Values [`devirt`] left alone: a `make_closure` candidate whose uses, escape fact or call
    /// sites did not fit, and a function whose escape set was `Unknown`.
    pub devirt_deopts: u32,
    /// The pairs of `rc_pairs` that needed [`LirRules::rc_borrow`]: a view of a uniquely owned
    /// local base that stays alive for the region.
    pub rc_borrow_pairs: u32,
    /// Pairs [`rc`] left alone because liveness or ownership answered `Unknown`, one per pair.
    pub rc_borrow_deopts: u32,
    /// Dead pure `let`s removed by [`dce`], one per statement.
    pub dce: u32,
    /// Functions [`dce`] left alone because liveness answered `Unknown`, one per function.
    pub dce_deopts: u32,
    /// Case bodies [`switch`] joined into one case with several keys (E4), one per case removed.
    pub switch_cases_merged: u32,
    /// Switches [`switch`] replaced by a direct block because every arm ran the same body or the
    /// single case covered every constructor (E4), one per switch.
    pub switch_direct: u32,
    /// Functions borrow inference gave a non-empty borrowed-parameter signature
    /// ([`LirRules::borrow`], C2/PR-026).
    ///
    /// This and the two counters below are facts about the calling convention, not a rewrite: the
    /// signature itself changes no statement. The observable effect is the `dup`s the caller no
    /// longer emits, counted by [`borrow_args`](Self::borrow_args).
    pub borrow_functions: u32,
    /// Borrowed parameter slots written into the signatures of those functions.
    pub borrow_params: u32,
    /// Direct-call argument positions that reference counting treats as borrowed because of a
    /// signature: a counted argument at a marked parameter position (the environment argument of a
    /// lifted call is not one). This is the number of call-site `dup`s the flag can remove when the
    /// argument stays live; the pass counts positions, not the `dup`s `insert_rc` actually omits.
    pub borrow_args: u32,
    /// `drop`s [`drop_spec`] gave the type-directed block release, one per statement
    /// (C3/PR-027).
    pub drop_specialized: u32,
    /// `drop`s [`drop_spec`] left generic, one per statement: a value whose payload may hold a
    /// reference or whose type has a finalizer.
    pub drop_deopts: u32,
    /// Constructors [`reuse`] forced a uniquely owned block to be written again instead of
    /// allocating a new one (C4/PR-042), one per `Make` statement.
    pub reuse: u32,
    /// Candidate pairs [`reuse`] left as a drop and an allocation, one per adjacent `drop` and
    /// `Make` of the same constructor whose conditions did not hold.
    pub reuse_deopts: u32,
}

impl Stats {
    /// Adds what `other` changed to these.
    pub fn merge(&mut self, other: Stats) {
        self.sra_tuples += other.sra_tuples;
        self.sra_records += other.sra_records;
        self.sra_deopts += other.sra_deopts;
        self.sra_closures += other.sra_closures;
        self.sra_closure_deopts += other.sra_closure_deopts;
        self.rc_pairs += other.rc_pairs;
        self.devirt += other.devirt;
        self.devirt_deopts += other.devirt_deopts;
        self.rc_borrow_pairs += other.rc_borrow_pairs;
        self.rc_borrow_deopts += other.rc_borrow_deopts;
        self.dce += other.dce;
        self.dce_deopts += other.dce_deopts;
        self.switch_cases_merged += other.switch_cases_merged;
        self.switch_direct += other.switch_direct;
        self.borrow_functions += other.borrow_functions;
        self.borrow_params += other.borrow_params;
        self.borrow_args += other.borrow_args;
        self.drop_specialized += other.drop_specialized;
        self.drop_deopts += other.drop_deopts;
        self.reuse += other.reuse;
        self.reuse_deopts += other.reuse_deopts;
    }
}

/// The LIR passes [`run_mid_with`] and [`run_heap_with`] may run, one flag per pass.
///
/// The stages themselves stay in place: a pass flag only decides whether its pass runs, not where
/// in the stage order it sits. The analyses are not passes and have no flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LirRules {
    /// SRA at [`run_mid`]: non-escaping scalar tuples replaced by their fields.
    pub sra: bool,
    /// SRA of records at [`run_mid`]: a non-escaping record whose fields are scalars is replaced
    /// by its fields (C5/PR-043). Off by default, so a default build's output is unchanged. It has
    /// no effect when [`sra`](Self::sra) is off. Closure records are the other half of C5:
    /// [`sra_closures`](Self::sra_closures) replaces them after the closure devirtualisation of
    /// E5/PR-037.
    pub sra_records: bool,
    /// Closure devirtualisation at [`run_mid`]: a call through a closure record that is created and
    /// only called in its function becomes a direct call. On by default (E5/PR-037): it makes the
    /// `effects` benchmark 0.89x on WASM and 0.85x on C, and leaves the WASM output of every other
    /// benchmark case byte-identical.
    pub devirt: bool,
    /// Closure scalar replacement at [`run_mid`] (C5/PR-043, closure half): a closure record that
    /// only feeds its own direct calls is removed and its scalar captures are passed directly to a
    /// specialisation of the lifted function ([`closure_sra`]). On by default with
    /// [`devirt`](Self::devirt), whose direct calls it needs to find candidates. A reference
    /// capture or a suspending function is not covered (see the pass contract).
    pub sra_closures: bool,
    /// `rc` cleanup at [`run_heap`]: `dup`/`drop` pairs removed.
    pub rc_cleanup: bool,
    /// The `rc` cleanup's local-base borrow elision at [`run_heap`]: also remove the pair of a view
    /// of a uniquely owned local base that stays alive for the whole region. On by default (C1);
    /// it needs the liveness (B4) and ownership (B6) facts and deopts when either is `Unknown`.
    pub rc_borrow: bool,
    /// Dead code elimination at [`run_heap`]: pure `let`s with a dead scalar destination removed.
    /// On by default (C7/PR-028). Frame pruning is not part of this flag: [`frame_prune`] is the
    /// read-only analysis of the saved slots, and the corpus has no candidate for a pruning pass.
    pub dce: bool,
    /// Drop specialisation at [`run_heap`] (C3/PR-027): a `drop` of a value whose payload holds no
    /// reference and whose type has no finalizer is released as a block, without walking fields
    /// ([`drop_spec`]). It is not a rewrite of the statements: it writes
    /// [`Function::drop_kinds`](crate::ir::Function::drop_kinds), which the backends read.
    /// Off by default: turning it on grew the default build's native binary by up to 7.5% and its
    /// C source by up to 3.7% in the M3 measurement, with no measured runtime gain, so the flag
    /// stays opt-in until its size cost is understood or a gain is measured. A drop that may hold
    /// a reference or has a finalizer stays generic.
    pub drop_spec: bool,
    /// Borrow inference between `state_machines` and `insert_rc` (C2/PR-026): write each
    /// function's borrowed-parameter signature from the borrow facts, so reference counting does
    /// not `dup` an argument the callee only reads and does not `drop` that parameter. On by
    /// default. This is not a rewrite of the Heap IR: it changes the calling convention the
    /// counting pass follows.
    pub borrow: bool,
    /// Perceus reuse at [`run_heap`] (C4/PR-042): a `drop` of a uniquely owned object of the same
    /// constructor as the `Make` right after it is removed, and the `Make` writes its fields into
    /// the old block instead of allocating, releasing the references the old fields held ([`reuse`]).
    /// Off by default: the default build's output stays what it was.
    pub reuse: bool,
    /// Switch normalisation at [`run_heap`] (E4): cases with the same body are joined into one
    /// case with several sorted keys, duplicate keys are dropped, the cases are ordered by key,
    /// and a switch every arm of which runs the same body becomes that body, without dispatch
    /// ([`switch`]). On by default: it only joins structurally identical bodies or removes a
    /// dispatch whose arms are all that one body, so it preserves behaviour and makes the emitted
    /// C and WASM smaller. This is the canonical form both backends read.
    pub switch: bool,
    /// Function pruning in the backends (E3/PR-039): after the Heap IR, remove every function
    /// that reachability neither calls nor references, and remap the [`FnId`](crate::ir::FnId)s of
    /// the module and the entry roots (`crate::analysis::reach::prune_functions`). Off by default:
    /// it changes what the C source contains and, because a function index disappears, the WASM
    /// bytes, so the default build's output and the byte pins stay what they were. A handler
    /// operation that a reachable closure record names stays (referenced, not called), and the
    /// backend emits a stub body for it as before.
    pub prune: bool,
}

impl LirRules {
    /// Every flag on.
    pub const fn all() -> Self {
        Self {
            sra: true,
            sra_records: true,
            devirt: true,
            sra_closures: true,
            rc_cleanup: true,
            rc_borrow: true,
            dce: true,
            drop_spec: true,
            borrow: true,
            reuse: true,
            switch: true,
            prune: true,
        }
    }

    /// Every flag off: every stage returns without changing the module.
    pub const fn none() -> Self {
        Self {
            sra: false,
            sra_records: false,
            devirt: false,
            sra_closures: false,
            rc_cleanup: false,
            rc_borrow: false,
            dce: false,
            drop_spec: false,
            borrow: false,
            reuse: false,
            switch: false,
            prune: false,
        }
    }
}

impl Default for LirRules {
    /// The passes that are on by default: SRA, the `rc` cleanup with the local-base borrow
    /// elision, DCE, borrow inference (the M3 memory passes that pay for themselves), the switch
    /// normalisation (E4), which only joins structurally identical bodies or removes a dispatch
    /// whose arms all run one body, and closure devirtualisation (E5) with closure SRA (C5). The
    /// last two are a default change, not a rule change: the `effects` benchmark runs 0.892–0.906x
    /// on WASM and 0.849–0.852x on C with both on, the WASM output of the other 16 benchmark cases
    /// is byte-identical, and the passes are covered by `tests/closure_sra.rs` end to end. Drop
    /// specialisation stays opt-in because its default-build size cost has no measured runtime
    /// gain; reuse stays opt-in so that the default build's output is unchanged; record SRA stays
    /// off because it rewrites nothing on the corpus; frame pruning has no pass, because
    /// [`frame_prune::scan`] finds no stale saved slot on the corpus. Reachability function
    /// pruning is off: it changes the C source and the WASM function indices, so the default
    /// output and the byte pins stay what they were.
    fn default() -> Self {
        Self {
            sra: true,
            sra_records: false,
            devirt: true,
            sra_closures: true,
            rc_cleanup: true,
            rc_borrow: true,
            dce: true,
            drop_spec: false,
            borrow: true,
            reuse: false,
            switch: true,
            prune: false,
        }
    }
}

/// The options of [`run_mid_with`] and [`run_heap_with`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Which LIR passes to run. [`LirRules::default`] by default: SRA, the `rc` cleanup with the
    /// borrow elision, DCE, borrow inference, switch normalisation, and closure devirtualisation
    /// with closure SRA, with record SRA, drop specialisation, reuse and reachability pruning off.
    pub rules: LirRules,
}

/// Runs the Flow IR optimisation passes on `m` if `optimize`, and returns what they changed.
///
/// Preconditions: `m` is at [`Stage::Flow`], before closure conversion.
///
/// Postconditions: `m` is still at [`Stage::Flow`].
///
/// Preserves: evaluation order, effects, failure edges, drop points and the evidence tables.
///
/// Deopt: a pass gives up on a function whose values it cannot place in the Flow IR.
pub fn run_flow(_m: &mut Module, _optimize: bool) -> Stats {
    Stats::default()
}

/// Runs the optimisation passes that need closure conversion but not the state machines.
///
/// Preconditions: `m` is at [`Stage::ClosuresConverted`]: a closure is a lifted function and a
/// `MakeClosure` record, and there is no `Closure` or `FnRef` value.
///
/// Postconditions: `m` is still at [`Stage::ClosuresConverted`]. No `Suspend` has been made, so a
/// pass may rewrite a value without changing a frame.
///
/// Preserves: evaluation order, effects, failure edges, drop points and the evidence tables.
///
/// Deopt: an aggregate or closure record whose value escapes
/// ([`escape::escaping`]) is left alone. So are values with a finalizer (`Any`, a handler, an
/// evidence table, a cell, a runtime object), values stored in a container, a handler or an
/// evidence table, captured values, and values live across a suspension point. [`devirt`] also
/// leaves a closure record alone when a use other than a call through it remains; [`closure_sra`]
/// leaves a record alone when a capture is a reference, when its function fails or suspends, or
/// when its function's environment is used beyond the capture reads; and [`sra`] leaves a tuple
/// alone when a field, a read or a use does not fit its rule (each lists its own).
///
/// [`sra`] replaces non-escaping scalar tuples here, and scalar records when
/// [`LirRules::sra_records`] is set; [`devirt`] makes the call of a closure record that is created
/// and only called in the same function direct, and [`closure_sra`] then replaces such a record by
/// its scalar captures when [`LirRules::sra_closures`] is set; `rc_cleanup` and [`dce`] run later,
/// in [`run_heap`].
///
/// This stage is the only one where SRA, closure devirtualisation and closure scalar replacement
/// are sound: running them after `state_machines` would also have to rewrite `Suspend::saved`,
/// which this stage fixes.
///
/// This is [`run_mid_with`] with [`Options::default`] (SRA, closure devirtualisation and closure
/// SRA on; the Heap IR passes are not here).
pub fn run_mid(m: &mut Module, optimize: bool) -> Stats {
    if !optimize {
        assert_eq!(
            m.stage,
            Stage::ClosuresConverted,
            "the closure optimisations run after closure conversion and before the state machines"
        );
        #[cfg(test)]
        observe_mid_now(m);
        return Stats::default();
    }
    run_mid_with(m, Options::default())
}

/// Runs the closure-level passes selected by `options`.
///
/// The preconditions, postconditions, preserved behaviour and deopt are those of [`run_mid`]; a
/// pass whose flag is off does not run and leaves its counter zero.
pub fn run_mid_with(m: &mut Module, options: Options) -> Stats {
    assert_eq!(
        m.stage,
        Stage::ClosuresConverted,
        "the closure optimisations run after closure conversion and before the state machines"
    );
    #[cfg(test)]
    observe_mid_now(m);
    let mut stats = Stats::default();
    if options.rules.sra {
        stats.merge(sra::run(m, options.rules.sra_records));
        check(m);
    }
    if options.rules.devirt {
        stats.merge(devirt::run(m));
        check(m);
    }
    // Closure SRA runs after E5, which leaves the record as the first argument of a direct call.
    // Without E5 every candidate has an indirect callee and is left alone.
    if options.rules.sra_closures {
        stats.merge(closure_sra::run(m));
        check(m);
    }
    stats
}

/// Runs the Heap IR optimisation passes on `m` if `optimize`, and returns what they changed.
///
/// Preconditions: `m` is at [`Stage::RefCounted`]: `insert_rc` has run and `dup` and `drop` are
/// explicit.
///
/// Postconditions: `m` is still at [`Stage::RefCounted`].
///
/// Preserves: evaluation order, effects, failure edges, the order and count of the releases of
/// `with`, and drop points. A pass may remove a `dup` and its matching `drop` together, and make
/// the last use of a reference a move, but never change when a value is released.
///
/// Deopt: [`rc`] leaves a `dup`/`drop` pair alone when the reference it takes may be the only thing
/// keeping the value alive (a view whose base is not the borrowed environment of the call, or, with
/// [`LirRules::rc_borrow`], a local base whose uniqueness or liveness is not known), when the
/// variable is consumed between the two, or when the `drop` is not reached on every path. [`dce`]
/// leaves a `let` alone when its value is not known to be pure, when its destination is a
/// reference, when it has a defect landing or a suspension edge, or when liveness answers
/// `Unknown`. [`switch`] leaves cases with different bodies, landings or suspension edges
/// unmerged. [`drop_spec`] leaves a `drop` generic when the value's payload may hold a reference
/// or the type has a finalizer. [`reuse`] leaves a `drop` and its `Make` alone unless the value is
/// read only by that drop, both are references of the same type, and the constructor has no
/// error, host handle or finalizer; a reference field is released before it is overwritten (see
/// [`reuse`]).
///
/// This is [`run_heap_with`] with [`Options::default`] (the `rc` cleanup with the local-base
/// borrow elision, DCE and switch normalisation on; drop specialisation and reuse are opt-in).
pub fn run_heap(m: &mut Module, optimize: bool) -> Stats {
    if !optimize {
        assert_eq!(
            m.stage,
            Stage::RefCounted,
            "the Heap IR optimisations run after reference counting"
        );
        return Stats::default();
    }
    run_heap_with(m, Options::default())
}

/// Runs the Heap IR passes selected by `options`.
///
/// The preconditions, postconditions, preserved behaviour and deopt are those of [`run_heap`]; a
/// pass whose flag is off does not run and leaves its counter zero.
pub fn run_heap_with(m: &mut Module, options: Options) -> Stats {
    assert_eq!(
        m.stage,
        Stage::RefCounted,
        "the Heap IR optimisations run after reference counting"
    );
    let mut stats = Stats::default();
    if options.rules.rc_cleanup {
        stats.merge(rc::run(m, options.rules.rc_borrow));
        check(m);
    }
    if options.rules.dce {
        stats.merge(dce::run(m));
        check(m);
    }
    // Switch normalisation runs after DCE, which is the last point at which two arms can become
    // the same body, and before the passes keyed by statement IDs: it compacts a function it
    // changed, and drop specialisation and reuse must see the final numbering (E4, [`switch`]).
    if options.rules.switch {
        stats.merge(switch::run(m));
        check(m);
    }
    // Drop specialisation runs last: the `rc` cleanup and DCE have cut statements and renumbered
    // the ones that remain, so this is the final set of `Drop`s and the statement IDs the backends
    // see (C3/PR-027, [`drop_spec`]).
    if options.rules.drop_spec {
        stats.merge(drop_spec::run(m));
        check(m);
    }
    // Reuse runs after drop specialisation (C4/PR-042, [`reuse`]): it removes a `drop` and must
    // not leave the classification of a drop that no longer exists, and the counter of the pass
    // that saw the pair first is already written. Nothing after it renumbers statements, so the
    // map it writes is the final statement numbering.
    if options.rules.reuse {
        stats.merge(reuse::run(m));
        check(m);
    }
    stats
}

#[cfg(test)]
type MidObserver = Box<dyn FnOnce(&Module)>;

#[cfg(test)]
thread_local! {
    /// A test-only observer called at the point [`run_mid`] runs. It lets a test check where the
    /// stage sits in [`crate::to_heap_with`] without changing the build's output.
    static MID_OBSERVER: RefCell<Option<MidObserver>> = const { RefCell::new(None) };
}

/// Registers the observer of the next [`run_mid`] on this thread (tests only).
#[cfg(test)]
pub(crate) fn observe_mid(observer: impl FnOnce(&Module) + 'static) {
    MID_OBSERVER.with(|slot| *slot.borrow_mut() = Some(Box::new(observer)));
}

#[cfg(test)]
fn observe_mid_now(m: &Module) {
    if let Some(observer) = MID_OBSERVER.with(|slot| slot.borrow_mut().take()) {
        observer(m);
    }
}

/// Checks that a pass left the module well typed. A failure is a compiler bug: the pass broke its
/// postcondition, not the program.
fn check(m: &Module) {
    #[cfg(debug_assertions)]
    if let Err(errors) = crate::verify(m) {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!(
            "a LIR optimisation pass produced ill-typed IR (a compiler bug):\n{}",
            lines.join("\n")
        );
    }
    #[cfg(not(debug_assertions))]
    let _ = m;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::ir::{Const, Stmt, Term, Value};
    use crate::{FnBuilder, to_heap_with};
    use emela_core::{FnType, TypeId};

    /// A Flow IR module with one nested closure, so that closure conversion has something to lift.
    fn module_with_a_closure() -> Module {
        let mut core = emela_core::Module::new();
        let thunk = core.func(FnType {
            params: vec![],
            ret: TypeId::INT,
            fails: false,
            suspends: false,
            ev: false,
            uses: false,
            sets: None,
        });
        let mut b = FnBuilder::new(&core, "lift");
        let n = b.assign("n", TypeId::INT, Value::Const(Const::Int(1)));
        let f = b.var("f", thunk);
        b.closure(f, vec![], vec![n], false, |b| {
            b.term(Term::Return(Some(n.into())));
        });
        b.term(Term::Return(None));
        let function = b.finish();
        let mut m = Module::new(core);
        m.add(function);
        m
    }

    /// The closure-level passes run after closure conversion (there are lifted records) and before
    /// the state machines (there is no `Suspend` yet), whatever the optimisation switch says.
    #[test]
    fn mid_runs_between_closure_conversion_and_the_state_machines() {
        for optimize in [false, true] {
            let mut m = module_with_a_closure();
            let called = Arc::new(AtomicBool::new(false));
            let seen = Arc::clone(&called);
            observe_mid(move |m| {
                assert_eq!(m.stage, Stage::ClosuresConverted);
                let mut records = 0;
                for (_, f) in m.functions.iter() {
                    for (_, stmt) in f.stmts.iter() {
                        let Stmt::Let {
                            value, on_suspend, ..
                        } = stmt
                        else {
                            continue;
                        };
                        assert!(
                            on_suspend.is_none(),
                            "a suspension edge exists before `state_machines`"
                        );
                        match value {
                            Value::Closure(_) | Value::FnRef(_) => {
                                panic!("a nested closure is still there after closure conversion")
                            }
                            Value::MakeClosure { .. } => records += 1,
                            _ => {}
                        }
                    }
                }
                assert!(records > 0, "no lifted closure record");
                seen.store(true, Ordering::SeqCst);
            });
            to_heap_with(&mut m, optimize);
            assert!(called.load(Ordering::SeqCst), "run_mid did not run");
        }
    }

    /// The stage stays put: the passes rewrite values in place, they do not move the module on.
    #[test]
    fn mid_keeps_the_stage() {
        let mut m = module_with_a_closure();
        crate::convert_closures(&mut m);
        assert_eq!(m.stage, Stage::ClosuresConverted);
        run_mid(&mut m, true);
        assert_eq!(m.stage, Stage::ClosuresConverted);
    }

    /// The precondition is checked: a Flow IR module (before closure conversion) is a bug.
    #[test]
    #[should_panic(expected = "the closure optimisations run after closure conversion")]
    fn mid_rejects_a_flow_module() {
        let mut m = module_with_a_closure();
        run_mid(&mut m, true);
    }

    /// A function without tuples is left as closure conversion made it.
    #[test]
    fn mid_leaves_a_function_without_tuples_alone() {
        let mut m = module_with_a_closure();
        crate::convert_closures(&mut m);
        let before = crate::dump(&m);
        run_mid(&mut m, true);
        assert_eq!(crate::dump(&m), before);
    }

    /// The total is the sum of what each stage returned.
    #[test]
    fn merge_adds_the_counters() {
        let mut total = Stats::default();
        total.merge(Stats {
            sra_tuples: 2,
            sra_records: 1,
            sra_deopts: 1,
            sra_closures: 1,
            sra_closure_deopts: 2,
            rc_pairs: 1,
            devirt: 4,
            devirt_deopts: 2,
            rc_borrow_pairs: 1,
            rc_borrow_deopts: 1,
            dce: 3,
            dce_deopts: 1,
            borrow_functions: 1,
            borrow_params: 2,
            borrow_args: 3,
            drop_specialized: 2,
            drop_deopts: 1,
            reuse: 1,
            reuse_deopts: 2,
            switch_cases_merged: 1,
            switch_direct: 1,
        });
        total.merge(Stats {
            sra_tuples: 3,
            sra_records: 4,
            sra_deopts: 2,
            sra_closures: 3,
            sra_closure_deopts: 1,
            rc_pairs: 4,
            devirt: 5,
            devirt_deopts: 1,
            rc_borrow_pairs: 2,
            rc_borrow_deopts: 2,
            dce: 5,
            dce_deopts: 2,
            borrow_functions: 2,
            borrow_params: 3,
            borrow_args: 4,
            drop_specialized: 3,
            drop_deopts: 2,
            reuse: 4,
            reuse_deopts: 1,
            switch_cases_merged: 1,
            switch_direct: 2,
        });
        total.merge(Stats::default());
        assert_eq!(total.sra_tuples, 5);
        assert_eq!(total.sra_records, 5);
        assert_eq!(total.sra_deopts, 3);
        assert_eq!(total.sra_closures, 4);
        assert_eq!(total.sra_closure_deopts, 3);
        assert_eq!(total.rc_pairs, 5);
        assert_eq!(total.devirt, 9);
        assert_eq!(total.devirt_deopts, 3);
        assert_eq!(total.rc_borrow_pairs, 3);
        assert_eq!(total.rc_borrow_deopts, 3);
        assert_eq!(total.borrow_functions, 3);
        assert_eq!(total.borrow_params, 5);
        assert_eq!(total.borrow_args, 7);
        assert_eq!(total.drop_specialized, 5);
        assert_eq!(total.drop_deopts, 3);
        assert_eq!(total.dce, 8);
        assert_eq!(total.dce_deopts, 3);
        assert_eq!(total.reuse, 5);
        assert_eq!(total.reuse_deopts, 3);
        assert_eq!(total.switch_cases_merged, 2);
        assert_eq!(total.switch_direct, 3);
    }

    /// `LirRules::all` turns every flag on; `none` turns every flag off. The default turns on the
    /// M3 memory passes that pay for themselves (the `rc` cleanup with the borrow elision, DCE and
    /// borrow inference), the E4 switch normalisation, and E5 closure devirtualisation with C5
    /// closure SRA, keeps drop specialisation opt-in for its size cost and record SRA off because
    /// it rewrites nothing on the corpus.
    #[test]
    fn the_lir_rule_flags_have_the_m3_defaults() {
        let all = LirRules::all();
        assert!(
            all.sra
                && all.sra_records
                && all.rc_cleanup
                && all.rc_borrow
                && all.devirt
                && all.dce
                && all.sra_closures
                && all.drop_spec
                && all.borrow
                && all.reuse
                && all.switch
        );
        let default = LirRules::default();
        assert!(default.sra && default.rc_cleanup);
        assert!(default.rc_borrow && default.dce && default.borrow && default.switch);
        assert!(default.devirt && default.sra_closures);
        assert!(!default.sra_records && !default.drop_spec && !default.reuse);
        let none = LirRules::none();
        assert!(
            !none.sra
                && !none.sra_records
                && !none.rc_cleanup
                && !none.rc_borrow
                && !none.devirt
                && !none.sra_closures
                && !none.drop_spec
                && !none.borrow
                && !none.reuse
                && !none.switch
        );
    }

    /// A stage whose pass is off does not run: the module and the counters are untouched.
    #[test]
    fn a_disabled_pass_leaves_the_stage_alone() {
        let mut m = module_with_a_closure();
        crate::convert_closures(&mut m);
        let before = crate::dump(&m);
        let stats = run_mid_with(
            &mut m,
            Options {
                rules: LirRules::none(),
            },
        );
        assert_eq!(stats.sra_tuples, 0);
        assert_eq!(stats.sra_records, 0);
        assert_eq!(stats.sra_deopts, 0);
        assert_eq!(stats.sra_closures, 0);
        assert_eq!(stats.sra_closure_deopts, 0);
        assert_eq!(stats.rc_pairs, 0);
        assert_eq!(stats.devirt, 0);
        assert_eq!(stats.dce, 0);
        assert_eq!(stats.switch_cases_merged, 0);
        assert_eq!(stats.switch_direct, 0);
        assert_eq!(crate::dump(&m), before);
    }

    /// The Heap IR wrapper keeps the stage precondition even when every pass is off.
    #[test]
    fn the_heap_wrapper_keeps_its_stage_check() {
        let mut m = module_with_a_closure();
        crate::convert_closures(&mut m);
        m.stage = Stage::RefCounted;
        let stats = run_heap_with(
            &mut m,
            Options {
                rules: LirRules::none(),
            },
        );
        assert_eq!(stats.sra_tuples, 0);
        assert_eq!(stats.sra_records, 0);
        assert_eq!(stats.sra_deopts, 0);
        assert_eq!(stats.rc_pairs, 0);
        assert_eq!(stats.dce, 0);
    }
}
