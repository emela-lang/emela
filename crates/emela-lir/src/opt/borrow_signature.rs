//! Borrow inference: which parameters a function may receive without taking the caller's
//! reference, written as a calling-convention fact on the function.
//!
//! [`insert_borrow_signature`] runs at [`Stage::StateMachines`], after the state machines have made
//! every suspension edge explicit and before [`crate::heap::insert_rc`] inserts `dup` and `drop`. It
//! reads the B6 borrow facts ([`crate::opt::analysis::borrow`]) and writes
//! [`Function::borrowed`](crate::ir::Function::borrowed); reference counting and its balance check
//! then treat those parameters as borrowed instead of owned.
//!
//! This slice (C2a) only carries the signature through the ownership rules. C2b wires it into
//! [`crate::to_heap_with`] behind [`LirRules::borrow`](crate::opt::LirRules::borrow), which is on
//! by default since M3.
//!
//! # Contract
//!
//! ```text
//! Rule ID: borrow-inference/calling-convention
//! Theory: Perceus reference counting with borrowed parameters. A parameter is borrowed when the
//!         callee only reads the value: it never takes the reference, so the caller may keep it
//!         and need not `dup` the argument. The B6 borrow facts prove that this is safe, but they
//!         are only a necessary condition: a borrow also has to pay for itself. A borrowed
//!         parameter saves the callee one drop and makes every caller keep its reference across
//!         the call and drop it afterwards. It removes work only when the callee reads a counted
//!         view out of the parameter (directly, or through other views) and does not consume that
//!         view, because the owned convention would give the view its own `dup`/`drop` pair. When
//!         every such view is consumed (stored, returned, or passed at an owned position), the
//!         borrow only moves a drop and is cleared.
//! Equation: keep a borrowed position  ⇒  ∃ counted view whose base chain reaches the parameter
//!         and which no use consumes under the tentative signatures.
//! Preconditions: the module is at Stage::StateMachines; the borrow analysis answers `Known`; the
//!         function has a body, is not exported, its value is never taken (no `FnRef`, no
//!         `MakeClosure` of it), and it is not the environment slot of a lifted function.
//! Postconditions: Function::borrowed is parallel to Function::params and is a subset of the B6
//!         facts after the suspension cascade; it is empty unless some parameter is borrowed.
//!         `heap::rc::borrowed_param` answers `true` for the marked positions, `heap::rc::Bases`
//!         keeps them (unless a frame saves them), `value_uses` borrows the matching arguments of
//!         direct calls, and `rc_check` counts their initial reference as zero. The rule runs to
//!         a fixpoint: clearing a position can make a caller's view consumed at that call site and
//!         so clear the caller's position too. It terminates because every round clears at least
//!         one `true` slot and no slot is ever set again.
//! Preserves: evaluation order, effects, failures, defect landings, suspension points and the
//!         variables a frame saves. The pass changes no statement; it only writes the signature.
//!         The narrowing is sound by construction: it only clears borrowed positions, so reference
//!         counting sees a subset of the signatures the B6 facts justify, and the owned convention
//!         that replaces a cleared position is balanced by `rc_check`.
//! Deopt: `Unknown` borrow facts leave every signature empty. Exported functions and functions
//!         whose value is taken keep the owned convention, because an indirect or host caller
//!         passes owned arguments. A parameter position passed a counted argument at a call that
//!         may suspend keeps the owned convention, because the caller's suspension frame was
//!         built before this pass and does not save an argument the call leaves owned. A position
//!         whose views are all consumed keeps the owned convention, because the borrow removes no
//!         counting work. An external function has no body and no signature.
//! Budget: one borrow analysis of the module, one walk of every value to find the functions whose
//!         value is taken, and the suspension cascade plus the C2 fixpoint. Each fixpoint round
//!         walks the statements of every function with a borrowed position and clears at least one
//!         slot, so there are at most as many rounds as borrowed slots.
//! Measures: the signature is a fact, not a rewrite. [`BorrowReport`] counts the functions and
//!         parameter slots of the final narrowed signature, and the call-site positions that
//!         reference counting now borrows (the `dup`s it can remove);
//!         `Stats::{borrow_functions, borrow_params, borrow_args}` carry them.
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
//! - **The caller never `dup`s a borrowed argument.** [`crate::heap::rc::value_uses`] reports the matching
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
//!   [`crate::heap::rc::Bases`] no longer treats views of it as borrowed. The environment parameter of a
//!   lifted function has always had this treatment ([`crate::heap::rc::owned_borrowed`]).
//! - **A counted argument of a suspending call is not borrowed.** The caller's frame is made before
//!   borrow inference and saves what the owned convention leaves owned. A borrowed argument stays
//!   owned in the caller, so the frame would have to save it; it does not. Such a position keeps
//!   the owned convention, and the call consumes the argument as before.
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
//! this pass between the state machines and [`crate::heap::insert_rc`]. The resulting program is
//! balanced:
//!
//! - the caller does not `dup` a borrowed argument; it still owns and drops its own reference when
//!   it no longer needs the value;
//! - the callee does not drop a borrowed parameter; if something inside the callee consumes it
//!   (a view is consumed, or a forced-owned callee takes it), the counting pass `dup`s first;
//! - the verifier's balance check ([`crate::heap::rc_check`]) counts a borrowed parameter as starting
//!   with zero references, accepts a reference-less read of it, and still checks each use;
//! - the guards above keep the convention sound: an address-taken or exported function, a runtime
//!   `show` function, and a frame-saved parameter are owned; `Array.set` keeps its array owned.
//!
//! The flag is on by default since M3. The counters ([`BorrowReport`]) are facts about the calling
//! convention, not a rewrite; the observable effect is the call-site `dup`s that reference
//! counting no longer emits.

use rustc_hash::{FxHashMap, FxHashSet};

use crate::ir::{Callee, FnId, Function, Module, Operand, Stage, Stmt, Value, VarId};
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
/// convention. The facts are narrowed after the suspension cascade: a position whose counted views
/// are all consumed saves no reference counting work and is cleared, to a fixpoint (C2). The pass
/// is idempotent: a second run recomputes the same facts from the analysis (which does not read the
/// signature) and writes the same vectors.
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
    let mut signatures: FxHashMap<FnId, Vec<bool>> = FxHashMap::default();
    for &id in &ids {
        let f = &m.functions[id];
        let Some(from_analysis) = facts.borrowed_params.get(&id) else {
            continue;
        };
        // An external function has no body to prove anything about, and a bodyless function cannot
        // have been a `Known` answer anyway.
        if f.body.is_none() {
            continue;
        }
        let mut borrowed = from_analysis.clone();
        borrowed.resize(f.params.len(), false);
        // The environment is borrowed according to `f.env`; keep it out of the signature so that
        // there is one mechanism per slot.
        if f.env.is_some()
            && let Some(first) = borrowed.first_mut()
        {
            *first = false;
        }
        // The host, the entry and indirect callers pass owned arguments.
        if f.exported || taken.contains(&id) || host_called.contains(&id) {
            borrowed.fill(false);
        }
        signatures.insert(id, borrowed);
    }
    // A borrowed argument is not consumed, so it stays owned in the caller across the call. A
    // suspension frame was built before this pass, under the owned convention, and does not save
    // it. Clear a borrowed position wherever a counted argument the caller owns is passed to a
    // call that may suspend; the call site then consumes it as before. Clearing a caller's own
    // signature can make more of its arguments owned, so repeat until nothing changes.
    loop {
        let mut changed = false;
        for &caller_id in &ids {
            let caller = &m.functions[caller_id];
            if caller.body.is_none() {
                continue;
            }
            for (_, stmt) in caller.stmts.iter() {
                let Stmt::Let {
                    value:
                        Value::Call {
                            callee: Callee::Direct(g),
                            args,
                            suspends: true,
                            ..
                        },
                    ..
                } = stmt
                else {
                    continue;
                };
                let callee = &m.functions[*g];
                let start = usize::from(callee.env.is_some());
                for (i, arg) in args.iter().enumerate().skip(start) {
                    let Operand::Var(v) = arg else {
                        continue;
                    };
                    if !crate::heap::rc::counted(caller, *v)
                        || !caller_owns(&signatures, caller_id, caller, *v)
                    {
                        continue;
                    }
                    if let Some(slot) = signatures.get_mut(g).and_then(|s| s.get_mut(i))
                        && *slot
                    {
                        *slot = false;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    clear_borrows_that_save_nothing(m, &ids, &mut signatures);
    let mut report = BorrowReport::default();
    for id in ids {
        let Some(borrowed) = signatures.get(&id) else {
            continue;
        };
        if borrowed.contains(&true) {
            report.functions += 1;
            report.params += borrowed.iter().filter(|&&b| b).count() as u32;
            m.functions[id].borrowed = borrowed.clone();
        } else {
            m.functions[id].borrowed.clear();
        }
    }
    report.args = borrowed_args(m);
    report
}

/// Clears the borrowed positions that do not remove reference counting work inside their function
/// (C2).
///
/// A borrowed parameter saves the callee one drop, but every caller then keeps its reference alive
/// across the call and drops it afterwards. The borrow therefore pays only when the callee reads a
/// counted view out of the parameter (directly, or through other views) and does not consume that
/// view: under the owned convention the view would need its own `dup`/`drop` pair, and borrowing
/// the base is what removes it. When every such view is consumed (stored, returned, passed at an
/// owned position), the borrow moves a drop and removes nothing, so the position goes back to the
/// owned convention.
///
/// Only `true` slots are cleared, starting from the facts and the suspension cascade. Clearing a
/// position can make a caller's view consumed at that call site, which can clear the caller's own
/// position, so the rule repeats until no position changes. The number of positions is finite and
/// every round clears at least one, so the loop terminates; the rounds are bounded by the total
/// number of borrowed slots.
fn clear_borrows_that_save_nothing(
    m: &mut Module,
    ids: &[FnId],
    signatures: &mut FxHashMap<FnId, Vec<bool>>,
) {
    // `value_uses` reads the tentative signatures to decide which call arguments are consumed.
    write_signatures(m, ids, signatures);
    loop {
        let mut to_clear: Vec<(FnId, Vec<usize>)> = Vec::new();
        for &id in ids {
            let f = &m.functions[id];
            if f.body.is_none() {
                continue;
            }
            let Some(sig) = signatures.get(&id) else {
                continue;
            };
            if !sig.contains(&true) {
                continue;
            }
            let (consumed, bases) = consumed_and_views(m, f);
            let clear: Vec<usize> = f
                .params
                .iter()
                .enumerate()
                .filter(|&(i, _)| sig.get(i) == Some(&true))
                .filter(|&(_, &p)| {
                    !bases
                        .keys()
                        .any(|&view| !consumed.contains(&view) && reaches_base(&bases, view, p))
                })
                .map(|(i, _)| i)
                .collect();
            if !clear.is_empty() {
                to_clear.push((id, clear));
            }
        }
        if to_clear.is_empty() {
            break;
        }
        for (id, clear) in to_clear {
            if let Some(sig) = signatures.get_mut(&id) {
                for i in clear {
                    sig[i] = false;
                }
            }
        }
        write_signatures(m, ids, signatures);
    }
}

/// The counted views of `f` and, for each, the variable it reads its value out of.
fn view_bases(f: &Function) -> FxHashMap<VarId, VarId> {
    let mut bases = FxHashMap::default();
    for (_, stmt) in f.stmts.iter() {
        let Stmt::Let {
            dst: Some(d),
            value,
            ..
        } = stmt
        else {
            continue;
        };
        if crate::heap::rc::counted(f, *d)
            && let Some(base) = crate::heap::rc::view_base(f, value)
        {
            bases.insert(*d, base);
        }
    }
    bases
}

/// The counted variables a use of `f` consumes, and its counted views ([`view_bases`]).
fn consumed_and_views(m: &Module, f: &Function) -> (FxHashSet<VarId>, FxHashMap<VarId, VarId>) {
    let mut consumed = FxHashSet::default();
    for (_, stmt) in f.stmts.iter() {
        if let Stmt::Let { value, .. } = stmt {
            crate::heap::rc::value_uses(m, f, value, &mut |v, mode| {
                if mode == crate::heap::rc::Mode::Consume {
                    consumed.insert(v);
                }
            });
        }
    }
    for (_, seq) in f.seqs.iter() {
        consumed.extend(crate::heap::rc::term_consumes(f, &seq.term));
    }
    (consumed, view_bases(f))
}

/// Whether the base chain of `view` reaches `param` through the counted views in `bases`.
///
/// A view is defined before its base is used, so a chain is no longer than the number of views;
/// the bound also stops a malformed cycle.
fn reaches_base(bases: &FxHashMap<VarId, VarId>, mut view: VarId, param: VarId) -> bool {
    for _ in 0..=bases.len() {
        if view == param {
            return true;
        }
        match bases.get(&view) {
            Some(&next) => view = next,
            None => return false,
        }
    }
    false
}

/// Writes the tentative signatures into the module, so that [`crate::heap::rc::value_uses`] reads the
/// call-site modes of the current round.
fn write_signatures(m: &mut Module, ids: &[FnId], signatures: &FxHashMap<FnId, Vec<bool>>) {
    for &id in ids {
        match signatures.get(&id) {
            Some(sig) if sig.contains(&true) => m.functions[id].borrowed = sig.clone(),
            _ => m.functions[id].borrowed.clear(),
        }
    }
}

/// Whether `v` is owned by `caller` at the call that uses it: a local, a frame-owned parameter, or
/// a parameter whose tentative signature is owned. A parameter that is still borrowed (and not
/// saved by a frame) is kept alive by the caller of `caller`, not by this frame.
fn caller_owns(
    signatures: &FxHashMap<FnId, Vec<bool>>,
    caller_id: FnId,
    caller: &Function,
    v: VarId,
) -> bool {
    if caller.env.is_some() && caller.params.first() == Some(&v) {
        return false;
    }
    let Some(i) = caller.params.iter().position(|&p| p == v) else {
        return true;
    };
    let borrowed = signatures
        .get(&caller_id)
        .and_then(|s| s.get(i))
        .copied()
        .unwrap_or(false);
    !borrowed || crate::heap::rc::saved_by_a_frame(caller, v)
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
                    && crate::heap::rc::counted(f, *v)
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
