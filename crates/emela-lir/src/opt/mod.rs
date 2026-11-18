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
//! | [`sra`] | [`Stats::sra_tuples`], [`Stats::sra_records`] | [`run_mid`] | scalar tuples; scalar records behind [`LirRules::sra_records`] (C5/PR-043). The closure half of C5 is deferred to E5/PR-037 |
//! | [`devirt`] | [`Stats::devirt`] | [`run_mid`] | closure records only called in their function (E5/PR-037) |
//! | [`rc`] cleanup | [`Stats::rc_pairs`] | [`run_heap`] | `dup`/`drop` pairs; C1/PR-025 generalises it |
//! | drop specialisation | — | [`run_heap`] | not implemented (C3/PR-027) |
//! | reuse / FBIP | — | [`run_heap`] | not implemented (C4/PR-042) |
//! | DCE / suspension frame pruning | — | [`run_heap`] | not implemented (C7/PR-028) |
//!
//! A row marked "not implemented" has no code and no flag yet; its pull request adds both. The
//! analyses are not passes and never change the module.
//!
//! # Feature flags
//!
//! [`LirRules`] has one flag per LIR pass that exists today ([`sra`], [`devirt`] and the `rc`
//! cleanup), plus one that extends SRA to records, and [`Options`] holds them.
//! [`run_mid_with`] and [`run_heap_with`] take the options; [`run_mid`] and [`run_heap`] keep
//! their old signatures and use [`Options::default`], which leaves the stage's output as it was
//! before closure devirtualisation existed and before records were decomposed. The flags are a
//! programmatic API only: no command-line flag is added in this skeleton. Closure devirtualisation
//! is off by default; the later LIR passes (drop specialisation, reuse, DCE, frame pruning) get
//! their own flags when they land.
//!
//! [`LirRules::sra_records`] is off by default so that a default build's output, the WASM byte pins
//! and the differential tests stay what they were. It only takes part when [`LirRules::sra`] is on
//! too.

pub mod analysis;
pub mod devirt;
pub(crate) mod escape;
mod rc;
mod sra;

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
    /// `dup`/`drop` pairs removed by [`rc`], one per duplicated reference.
    pub rc_pairs: u32,
    /// Indirect calls made direct by [`devirt`], one per call.
    pub devirt: u32,
    /// Values [`devirt`] left alone: a `make_closure` candidate whose uses, escape fact or call
    /// sites did not fit, and a function whose escape set was `Unknown`.
    pub devirt_deopts: u32,
}

impl Stats {
    /// Adds what `other` changed to these.
    pub fn merge(&mut self, other: Stats) {
        self.sra_tuples += other.sra_tuples;
        self.sra_records += other.sra_records;
        self.sra_deopts += other.sra_deopts;
        self.rc_pairs += other.rc_pairs;
        self.devirt += other.devirt;
        self.devirt_deopts += other.devirt_deopts;
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
    /// no effect when [`sra`](Self::sra) is off. Closure records are not covered: that half of C5
    /// needs the closure devirtualisation of E5/PR-037.
    pub sra_records: bool,
    /// Closure devirtualisation at [`run_mid`]: a call through a closure record that is created and
    /// only called in its function becomes a direct call. Off by default: the stage's output stays
    /// what it was before E5/PR-037 until a later change turns it on.
    pub devirt: bool,
    /// `rc` cleanup at [`run_heap`]: `dup`/`drop` pairs removed.
    pub rc_cleanup: bool,
}

impl LirRules {
    /// Every flag on.
    pub const fn all() -> Self {
        Self {
            sra: true,
            sra_records: true,
            devirt: true,
            rc_cleanup: true,
        }
    }

    /// Every flag off: every stage returns without changing the module.
    pub const fn none() -> Self {
        Self {
            sra: false,
            sra_records: false,
            devirt: false,
            rc_cleanup: false,
        }
    }
}

impl Default for LirRules {
    /// The passes that are on by default: SRA and the `rc` cleanup. Record SRA and closure
    /// devirtualisation are off until a later change turns them on, so a default build's output is
    /// unchanged.
    fn default() -> Self {
        Self {
            sra: true,
            sra_records: false,
            devirt: false,
            rc_cleanup: true,
        }
    }
}

/// The options of [`run_mid_with`] and [`run_heap_with`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Which LIR passes to run. [`LirRules::default`] by default: SRA and the `rc` cleanup, with
    /// record SRA and closure devirtualisation off.
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
/// leaves a closure record alone when a use other than a call through it remains, and [`sra`]
/// leaves a tuple alone when a field, a read or a use does not fit its rule (each lists its own).
///
/// [`sra`] replaces non-escaping scalar tuples here, and scalar records when
/// [`LirRules::sra_records`] is set; [`devirt`] makes the call of a closure record that is created
/// and only called in the same function direct; `rc_cleanup` runs later, in [`run_heap`].
///
/// This stage is the only one where SRA and closure devirtualisation are sound: running them after
/// `state_machines` would also have to rewrite `Suspend::saved`, which this stage fixes.
///
/// This is [`run_mid_with`] with [`Options::default`] (SRA on; the Heap IR passes and closure
/// devirtualisation are not here).
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
/// keeping the value alive (a view whose base is not the borrowed environment of the call), when
/// the variable is consumed between the two, or when the `drop` is not reached on every path.
///
/// This is [`run_heap_with`] with [`Options::default`] (every pass on).
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
        stats.merge(rc::run(m));
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
            rc_pairs: 1,
            devirt: 4,
            devirt_deopts: 2,
        });
        total.merge(Stats {
            sra_tuples: 3,
            sra_records: 4,
            sra_deopts: 2,
            rc_pairs: 4,
            devirt: 5,
            devirt_deopts: 1,
        });
        total.merge(Stats::default());
        assert_eq!(total.sra_tuples, 5);
        assert_eq!(total.sra_records, 5);
        assert_eq!(total.sra_deopts, 3);
        assert_eq!(total.rc_pairs, 5);
        assert_eq!(total.devirt, 9);
        assert_eq!(total.devirt_deopts, 3);
    }

    /// `LirRules::all` turns every flag on; `none` turns every flag off. Record SRA and closure
    /// devirtualisation are off in `Default`, so a default build keeps the output as it was before
    /// C5/PR-043 and E5/PR-037.
    #[test]
    fn the_lir_rule_flags_have_conservative_defaults() {
        let all = LirRules::all();
        assert!(all.sra && all.sra_records && all.rc_cleanup && all.devirt);
        let default = LirRules::default();
        assert!(default.sra && default.rc_cleanup);
        assert!(!default.sra_records && !default.devirt);
        let none = LirRules::none();
        assert!(!none.sra && !none.sra_records && !none.rc_cleanup && !none.devirt);
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
        assert_eq!(stats.rc_pairs, 0);
        assert_eq!(stats.devirt, 0);
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
    }
}
