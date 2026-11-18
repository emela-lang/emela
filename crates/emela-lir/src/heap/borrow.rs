//! Borrow inference: which parameters a function may receive without taking the caller's
//! reference, written as a calling-convention fact on the function.
//!
//! [`insert_borrow_signature`] runs at [`Stage::StateMachines`], after the state machines have made
//! every suspension edge explicit and before [`super::insert_rc`] inserts `dup` and `drop`. It
//! reads the B6 borrow facts ([`crate::opt::analysis::borrow`]) and writes
//! [`Function::borrowed`](crate::ir::Function::borrowed); reference counting and its balance check
//! then treat those parameters as borrowed instead of owned.
//!
//! This slice (C2a) only carries the signature through the ownership rules. The default pipeline
//! does not call this pass, so a default build's `dup`/`drop` and both backends' output are
//! unchanged; C2b wires it into [`crate::to_heap_with`] behind a flag and makes the call sites use
//! it.
//!
//! # Contract
//!
//! ```text
//! Rule ID: borrow-inference/calling-convention
//! Theory: Perceus reference counting with borrowed parameters. A parameter is borrowed when the
//!         callee only reads the value: it never takes the reference, so the caller may keep it
//!         and need not `dup` the argument. The B6 borrow facts prove exactly that.
//! Equation: callee(param) with a read-only parameter  ⇒  caller passes it without `dup`
//! Preconditions: the module is at Stage::StateMachines; the borrow analysis answers `Known`; the
//!         function has a body, is not exported, its value is never taken (no `FnRef`, no
//!         `MakeClosure` of it), and it is not the environment slot of a lifted function.
//! Postconditions: Function::borrowed is parallel to Function::params; it is empty unless some
//!         parameter is borrowed. `heap::rc::borrowed_param` answers `true` for the marked
//!         positions, `heap::rc::Bases` keeps them (unless a frame saves them), `value_uses`
//!         borrows the matching arguments of direct calls, and `rc_check` counts their initial
//!         reference as zero.
//! Preserves: evaluation order, effects, failures, defect landings, suspension points and the
//!         variables a frame saves. The pass changes no statement; it only adds the signature.
//! Deopt: `Unknown` borrow facts leave every signature empty. Exported functions and functions
//!         whose value is taken keep the owned convention, because an indirect or host caller
//!         passes owned arguments. An external function has no body and no signature.
//! Budget: one borrow analysis of the module, then one walk of every value to find the functions
//!         whose value is taken.
//! Measures: the signature is a fact, not a rewrite. [`BorrowReport`] counts the functions and
//! parameter slots it marked, and the call-site positions that reference counting now borrows
//! (the `dup`s it can remove); `Stats::{borrow_functions, borrow_params, borrow_args}` carry them.
//! ```
//!
//! # Soundness conditions
//!
//! The counting pass and the balance check rely on all of these together; a violation makes the
//! signature unsound even when the analysis itself said "borrowed":
//!
//! - **The callee never drops a borrowed parameter.** Reference counting leaves it out of the
//!   function's owned set (the counting pass), so it only gets a `dup` when something inside the
//!   callee consumes it. B6 proves that no use of the parameter consumes it.
//! - **The caller never `dup`s a borrowed argument.** [`super::rc::value_uses`] reports the matching
//!   position of a direct call as a borrow. The caller still releases its own reference when it is
//!   done with the value, as for any other value it owns.
//! - **An address-taken or exported function keeps the owned convention.** A function whose value
//!   is taken (a `FnRef`, or a `MakeClosure` record) may be called through
//!   [`Callee::Indirect`](crate::ir::Callee::Indirect), and the indirect caller passes owned
//!   arguments because the signature is not visible at that call. Exported functions are called by
//!   the host and the entry with owned arguments, and the runtime calls the error `show` functions
//!   of [`Module::error_shows`](crate::ir::Module::error_shows) with an owned error. All are forced
//!   to `false`.
//! - **A frame-saved borrowed parameter is `dup`'d and owned.** A suspension frame may keep the
//!   value after the call returned suspended, and the caller may release its reference then. When
//!   a frame saves a borrowed parameter, reference counting inserts a `dup` at the start of the
//!   function and drops the parameter like any other owned variable, and
//!   [`super::rc::Bases`] no longer treats views of it as borrowed. The environment parameter of a
//!   lifted function has always had this treatment ([`super::rc::owned_borrowed`]).
//! - **`Array.set` keeps its array argument owned.** The runtime writes the array in place when the
//!   actual count is one, so a borrowed last reference would let the callee mutate the caller's
//!   array. B6 refuses to mark the array argument of `Array.set` borrowed, so the signature never
//!   does. (`insert_borrow_signature` itself does not special-case it; it trusts the analysis,
//!   which does.)
//! - **The environment argument stays borrowed.** The first argument of a direct call of a lifted
//!   function is its closure record; the caller keeps it alive for the call. The environment slot
//!   is left out of the signature (it is handled by [`Function::env`](crate::ir::Function::env))
//!   so that a reader cannot confuse the two mechanisms.

//! # End-to-end contract (C2b)
//!
//! With [`LirRules::borrow`](crate::opt::LirRules::borrow) on, [`crate::to_heap_with_options`] runs
//! this pass between the state machines and [`super::insert_rc`]. The resulting program is
//! balanced:
//!
//! - the caller does not `dup` a borrowed argument; it still owns and drops its own reference when
//!   it no longer needs the value;
//! - the callee does not drop a borrowed parameter; if something inside the callee consumes it
//!   (a view is consumed, or a forced-owned callee takes it), the counting pass `dup`s first;
//! - the verifier's balance check ([`super::rc_check`]) counts a borrowed parameter as starting
//!   with zero references, accepts a reference-less read of it, and still checks each use;
//! - the guards above keep the convention sound: an address-taken or exported function, a runtime
//!   `show` function, and a frame-saved parameter are owned; `Array.set` keeps its array owned.
//!
//! The flag is off by default, so a default build never runs the pass and the dump, byte pins and
//! backends' output stay what they were. The counters ([`BorrowReport`]) are facts about the
//! calling convention, not a rewrite; the observable effect is the call-site `dup`s that
//! reference counting no longer emits.

use rustc_hash::FxHashSet;

use crate::ir::{Callee, FnId, Module, Operand, Stage, Stmt, Value};
use crate::opt::analysis::borrow;

/// What [`insert_borrow_signature`] wrote: how many functions and parameter slots got a borrowed
/// signature, and how many direct-call positions reference counting now treats as borrowed.
///
/// `args` counts counted variables passed at marked positions (excluding the environment argument
/// of a lifted call), so it is the number of call-site `dup`s the flag can remove when the argument
/// stays live. It is a fact about the calling convention, not a rewrite of the Heap IR.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BorrowReport {
    /// Functions that got a non-empty signature (at least one `true`).
    pub functions: u32,
    /// Borrowed parameter slots written across those functions.
    pub params: u32,
    /// Direct-call argument positions that are borrowed because of a signature.
    pub args: u32,
}

impl BorrowReport {
    /// The report as the counters of [`crate::opt::Stats`].
    pub(crate) fn into_stats(self) -> crate::opt::Stats {
        crate::opt::Stats {
            borrow_functions: self.functions,
            borrow_params: self.params,
            borrow_args: self.args,
            ..crate::opt::Stats::default()
        }
    }
}

/// Writes each function's borrowed-parameter signature from the B6 borrow facts.
///
/// The module must be at [`Stage::StateMachines`]. The signature is
/// [`Function::borrowed`](crate::ir::Function::borrowed), parallel to
/// [`Function::params`](crate::ir::Function::params): `true` at a position means the caller keeps
/// its reference for the call and the callee must not drop it (see the module documentation for the
/// soundness conditions).
///
/// A function whose signature has no `true` gets an empty vector, which means the owned calling
/// convention. The pass is idempotent: a second run recomputes the same facts from the analysis
/// (which does not read the signature) and writes the same vectors.
///
/// Returns what it wrote ([`BorrowReport`]); the report is not a rewrite count.
pub fn insert_borrow_signature(m: &mut Module) -> BorrowReport {
    assert_eq!(
        m.stage,
        Stage::StateMachines,
        "borrow inference runs after the state machines are made and before reference counting"
    );
    // `Unknown` is the top of the analysis lattice and is not a negative fact: leave every function
    // with the owned convention.
    let Some(facts) = borrow::analyze(m).known() else {
        return BorrowReport::default();
    };
    let taken = functions_taken_as_values(m);
    let host_called: FxHashSet<FnId> = m.error_shows.iter().map(|&(_, f)| f).collect();
    let ids: Vec<FnId> = m.functions.iter().map(|(id, _)| id).collect();
    let mut report = BorrowReport::default();
    for id in ids {
        let Some(from_analysis) = facts.borrowed_params.get(&id) else {
            continue;
        };
        let borrowed = {
            let f = &m.functions[id];
            // An external function has no body to prove anything about, and a bodyless function
            // cannot have been a `Known` answer anyway.
            if f.body.is_none() {
                continue;
            }
            let mut borrowed = from_analysis.clone();
            borrowed.resize(f.params.len(), false);
            // The environment is borrowed according to `f.env`; keep it out of the signature so
            // that there is one mechanism per slot.
            if f.env.is_some()
                && let Some(first) = borrowed.first_mut()
            {
                *first = false;
            }
            // The host, the entry and indirect callers pass owned arguments.
            if f.exported || taken.contains(&id) || host_called.contains(&id) {
                borrowed.fill(false);
            }
            borrowed
        };
        if borrowed.contains(&true) {
            report.functions += 1;
            report.params += borrowed.iter().filter(|&&b| b).count() as u32;
            m.functions[id].borrowed = borrowed;
        } else {
            m.functions[id].borrowed.clear();
        }
    }
    report.args = borrowed_args(m);
    report
}

/// The counted direct-call argument positions that a signature marks borrowed.
///
/// The environment argument of a lifted call is borrowed by the calling convention, not by a
/// signature, so it is not counted. A position whose argument is not a counted variable would
/// have no `dup` to remove and is not counted either.
fn borrowed_args(m: &Module) -> u32 {
    let mut n = 0;
    for (_, f) in m.functions.iter() {
        for (_, stmt) in f.stmts.iter() {
            let Stmt::Let {
                value:
                    Value::Call {
                        callee: Callee::Direct(g),
                        args,
                        ..
                    },
                ..
            } = stmt
            else {
                continue;
            };
            let callee = &m.functions[*g];
            if callee.external.is_some() {
                continue;
            }
            let start = usize::from(callee.env.is_some());
            for (i, o) in args.iter().enumerate().skip(start) {
                if callee.borrowed.get(i) != Some(&true) {
                    continue;
                }
                if let Operand::Var(v) = o
                    && super::rc::counted(f, *v)
                {
                    n += 1;
                }
            }
        }
    }
    n
}

/// The functions the module uses as values: a `FnRef`, or the target of a `MakeClosure` record.
///
/// Such a function may be called through [`Callee::Indirect`](crate::ir::Callee::Indirect), which
/// cannot see the signature, so it must keep the owned convention. An unreachable value left in an
/// arena only adds a conservative `false`.
fn functions_taken_as_values(m: &Module) -> FxHashSet<FnId> {
    let mut taken = FxHashSet::default();
    for (_, f) in m.functions.iter() {
        for (_, stmt) in f.stmts.iter() {
            let Stmt::Let { value, .. } = stmt else {
                continue;
            };
            match value {
                Value::FnRef(g) => {
                    taken.insert(*g);
                }
                Value::MakeClosure { function, .. } => {
                    taken.insert(*function);
                }
                _ => {}
            }
        }
    }
    taken
}
