//! Suspension frame pruning (C7/PR-028): which saved slots a deferred suspension still needs.
//!
//! This module is a read-only analysis. [`scan`] reports, over a module at
//! [`Stage::RefCounted`](crate::ir::Stage::RefCounted), how many entries of the suspension frames
//! (`Suspend::saved`) no longer need to be saved, and why the others are still needed. **No pass
//! prunes frames**, because the corpus has no candidate: see "Why there are no candidates" below.
//!
//! # The question
//!
//! [`state_machines`](crate::heap::state_machines) fixes each edge's saved set from liveness at
//! that point: every variable live after the value on any way on, plus the references the value
//! borrows (`heap::suspend::saved_vars`). Reference counting (`insert_rc`) then runs, and the `rc`
//! cleanup and [`dce`](crate::opt::dce) rewrite statements after it. An entry is *stale* when it is
//! neither live after the value nor borrowed by it. The verifier's `saved_errors` requires only the
//! entries that liveness and the value's borrows need, so a stale entry may be removed from the
//! frame.
//!
//! # What pruning a stale entry would take
//!
//! - A **scalar** (`Repr` other than `Ref`) owns nothing and has no finalizer. Removing it from the
//!   frame would change only the frame's size, which is memory (`[dyn.memory]` leaves it
//!   implementation-defined).
//! - An **owned reference** must keep its reference balanced. Removing it from the frame without a
//!   release would leak it, and `rc_check` would reject the module, so the pass would have to insert
//!   `Stmt::Drop(v)` immediately before the suspension. That is allowed only when the release runs
//!   no finalizer ([`crate::opt::drop_spec::has_finalizer`]) and no kept variable reads out of `v`
//!   through a view (a view borrows its base's value, so releasing the base first would leave a
//!   dangling view).
//! - A **borrowed reference** of the value is one the callee may still hold; it must stay saved.
//!
//! Ownership needs no extra analysis for this decision: each counted variable in `edge.saved`
//! owns exactly one reference at the edge. Reference counting moves the owned set into the frame
//! (`heap::rc` asserts `saved ∩ refs == owned`), and `rc_check` accepts the edge only when taking
//! each saved reference leaves nothing owned; a counted saved variable that owned none would
//! underflow, and one that owned two would leave one behind. Membership in `saved` is therefore
//! the ownership proof.
//!
//! # Why there are no candidates
//!
//! The obstacle is the pipeline, not the rule. The saved set is computed **before** reference
//! counting, and nothing after it can make a saved entry stale:
//!
//! - Reference counting only adds `dup` and `drop` statements, and a `dup` or `drop` is a use, so
//!   it grows liveness, never shrinks it.
//! - `rc` cannot remove a `dup`/`drop` pair whose variable a frame saves: a frame
//!   save is a consuming use, so the region the pair needs never closes. It cannot take the last use
//!   of a saved reference away either.
//! - [`dce`](crate::opt::dce) is the only remaining pass. It removes a pure `let` with a scalar
//!   destination, and it never removes a reference-valued `let`; if that `let` was the last use of
//!   a saved **scalar** after the suspension, the slot becomes stale. Everything else is kept.
//!
//! On the local corpus that last shape does not occur. With the default LIR passes,
//! `crates/emela-codegen-c/tests/programs/*.emel` (68 programs) has 803 suspension edges and 1,344
//! saved slots, and `examples/*` (10 projects) has 526 edges and 1,042 slots; **no slot is stale**
//! (2026-11-19). `dce` removes **zero** statements over both, so it has no chance to make one
//! stale: lowering and the Core reductions do not leave a dead pure scalar `let` that reads a
//! variable saved at an earlier suspension. `rc_cleanup` removes 402 and 762 pairs, but never one
//! whose variable a frame saves.
//!
//! The hand-built LIR fixtures set their saved sets explicitly; the tests in
//! `tests/frame_prune.rs` scan the shapes that would be stale (a dead saved scalar, a dead saved
//! reference, and the module `dce` turns into the first one) and the two reasons a stale reference
//! stays saved (a finalizer, and a kept view of it). The corpus test
//! `stale_frame_slots_on_the_c_corpus` asserts the zero, so a future change that makes a slot stale
//! fails the suite and reopens the pass.
//!
//! # Contract
//!
//! ```text
//! Analysis ID: C7 suspension frame staleness (read-only; no pass consumes it yet)
//! Domain / lattice: for each function, a report over its suspension edges; a saved entry is
//!                   `needed` (live after the value or borrowed by it) or `stale`, and a stale
//!                   reference is classified as prunable, finalizer-bearing or read by a kept view.
//!                   The lattice is flat: an entry is one of these classes or `Unknown`.
//! Transfer: liveness is the backward walk of `heap::live` (its own contract); "borrowed by the
//!           value" is `heap::rc::value_uses` with `Mode::Borrow`; the view chains are
//!           `heap::rc::Bases`. A stale scalar needs nothing else; a stale reference is prunable
//!           when its type has no finalizer and no kept variable's view chain ends at it.
//! Soundness: a reported `stale` entry is not read after the value on any way on and is not
//!            borrowed by it, so the verifier's `saved_errors` does not require it. A reported
//!            `stale_ref` owns exactly one reference at the edge (the saved-set invariant of
//!            `heap::rc` and `rc_check`), its release runs no finalizer, and no kept variable reads
//!            out of it, so a `drop` immediately before the suspension balances and is unobservable.
//! Deopt (unknown): a statement that liveness does not key (no answer) is skipped and its function
//!                  counted in `unknown_functions`; the scan never reads "no answer" as "not live".
//! Consumers: none yet. A pruning pass would be the consumer, and would run after `dce` in
//!            `opt::run_heap_with` (the order in which the scan is tested).
//! ```
//!
//! # Status
//!
//! C7/PR-028: the DCE half is implemented ([`dce`](crate::opt::dce)). The frame-pruning half has
//! no candidate on the corpus, so it stops at this analysis; the next memory work is PR-042 (C4
//! reuse).

use crate::heap::live::live_on_lets;
use crate::heap::rc::{Bases, Mode, value_uses};
use crate::ir::{Module, Stmt, VarId};
use crate::repr::Repr;

/// What [`scan`] found in the suspension frames of a module.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    /// Suspension edges whose saved set the scan looked at.
    pub edges: u32,
    /// Functions with a suspension edge that liveness does not key, so their edges were skipped.
    pub unknown_functions: u32,
    /// Entries in the saved sets of the edges looked at.
    pub slots: u32,
    /// Saved entries that are neither live after the value nor borrowed by it (stale).
    pub stale: u32,
    /// Stale entries of a scalar representation, removable from the frame as they are.
    pub stale_scalars: u32,
    /// Stale entries of a reference representation that own one reference, have no finalizer and
    /// are not read out by a kept variable, so a `drop` before the value would release them.
    pub stale_refs: u32,
    /// Stale reference entries kept because their type has a finalizer.
    pub kept_finalizers: u32,
    /// Stale reference entries kept because a kept variable reads out of them through a view.
    pub kept_views: u32,
}

impl Report {
    /// Whether a pruning pass would change anything.
    pub fn has_candidates(&self) -> bool {
        self.stale_scalars > 0 || self.stale_refs > 0
    }
}

/// Counts the stale entries of every suspension frame (read-only).
///
/// The module must be at [`Stage::RefCounted`](crate::ir::Stage::RefCounted) for the scan to mean
/// what the pass would see; it changes nothing at any stage. A suspension edge exists only from
/// [`Stage::StateMachines`](crate::ir::Stage::StateMachines) on, and a statement that liveness does
/// not key is reported through [`Report::unknown_functions`], never read as "not live".
pub fn scan(m: &Module) -> Report {
    let mut report = Report::default();
    for (_, f) in m.functions.iter() {
        if f.body.is_none() || !f.suspends {
            continue;
        }
        let live = live_on_lets(f);
        let bases = Bases::new(f);
        let mut unknown = false;
        for (_, seq) in f.seqs.iter() {
            for &s in &seq.stmts {
                let Stmt::Let {
                    value,
                    on_suspend: Some(edge),
                    ..
                } = &f.stmts[s]
                else {
                    continue;
                };
                // `None` is "no answer", not "not live": leave the edge alone.
                let Some(after) = live.get(&s) else {
                    unknown = true;
                    continue;
                };
                report.edges += 1;
                report.slots += edge.saved.len() as u32;
                // What the verifier's `saved_errors` recomputes and still requires.
                let mut keep = after.clone();
                value_uses(m, f, value, &mut |v, mode| {
                    if mode == Mode::Borrow {
                        keep.insert(v);
                    }
                });
                for &v in &edge.saved {
                    if keep.contains(&v) {
                        continue;
                    }
                    report.stale += 1;
                    if f.vars[v].repr != Repr::Ref {
                        report.stale_scalars += 1;
                        continue;
                    }
                    if crate::opt::drop_spec::has_finalizer(&m.core, f.vars[v].ty) {
                        report.kept_finalizers += 1;
                        continue;
                    }
                    if kept_reads_out_of(&bases, f.vars.len(), &keep, v) {
                        report.kept_views += 1;
                        continue;
                    }
                    report.stale_refs += 1;
                }
            }
        }
        if unknown {
            report.unknown_functions += 1;
        }
    }
    report
}

/// Whether a kept variable reads its value out of `base` through a view chain.
fn kept_reads_out_of(
    bases: &Bases,
    variables: usize,
    keep: &crate::heap::live::Set,
    base: VarId,
) -> bool {
    keep.iter().any(|&v| {
        let mut cur = v;
        // A view's base is defined before it, so a chain is no longer than the variables of the
        // function; the bound only guards a cycle the constructor cannot make.
        for _ in 0..=variables {
            match bases.view_base(cur) {
                Some(b) if b == base => return true,
                Some(b) => cur = b,
                None => return false,
            }
        }
        false
    })
}
