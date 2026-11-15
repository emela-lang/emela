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
//! `--no-opt`. There is no generic pass framework: each stage runs a fixed sequence.
//!
//! Every pass preserves the observable behaviour of the program: evaluation order, effects,
//! failures, the order and count of the releases of `with`, and drop points. It may change memory
//! use and when a program runs out of memory. A pass that cannot keep its contract gives up on
//! that function instead of changing it.
//!
//! The analyses a pass needs are shared and separately tested: [`escape`] (which values leave a
//! function) and `crate::heap::live` (liveness). Each pass documents its preconditions,
//! postconditions, what it preserves, when it deopts, and what it measures (parent plan §3.2).
//!
//! | Pass | Counter | Stage |
//! |---|---|---|
//! | [`sra`] | [`Stats::sra_tuples`] | [`run_mid`] |
//! | `rc_cleanup` (later) | — | [`run_heap`] |

pub(crate) mod escape;
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
}

impl Stats {
    /// Adds what `other` changed to these.
    pub fn merge(&mut self, other: Stats) {
        self.sra_tuples += other.sra_tuples;
    }
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
/// evidence table, captured values, and values live across a suspension point.
///
/// [`sra`] replaces non-escaping scalar tuples here; `rc_cleanup` runs later, in [`run_heap`].
///
/// This stage is the only one where SRA and closure devirtualisation are sound: running them after
/// `state_machines` would also have to rewrite `Suspend::saved`, which this stage fixes.
pub fn run_mid(m: &mut Module, optimize: bool) -> Stats {
    assert_eq!(
        m.stage,
        Stage::ClosuresConverted,
        "the closure optimisations run after closure conversion and before the state machines"
    );
    #[cfg(test)]
    observe_mid_now(m);
    if !optimize {
        return Stats::default();
    }
    let stats = sra::run(m);
    check(m);
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
/// Deopt: a function with a release that may run a finalizer or an effect is left alone.
pub fn run_heap(_m: &mut Module, _optimize: bool) -> Stats {
    Stats::default()
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
    fn merge_adds_sra_tuples() {
        let mut total = Stats::default();
        total.merge(Stats { sra_tuples: 2 });
        total.merge(Stats { sra_tuples: 3 });
        total.merge(Stats::default());
        assert_eq!(total.sra_tuples, 5);
    }
}
