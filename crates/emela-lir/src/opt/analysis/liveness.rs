//! Liveness: which variables are live after a statement or at a suspension point.
//!
//! # Contract
//!
//! ```text
//! Analysis ID: AnalysisId::Liveness
//! Domain / lattice: for each function, a map from each `let` statement to the set of variables
//!                   live after it, with the statement's own destination excluded (it is the value
//!                   the statement just made, not an earlier value kept live across it). Sets are
//!                   ordered by inclusion at a join; `Unknown` is the top element.
//! Transfer: backward. A use makes a variable live, a definition kills it, and an `if`, `switch`,
//!           `loop` back edge, block exit, failure landing, defect landing or suspension edge
//!           joins the sets of its successors. Loops iterate to a fixpoint over the outer
//!           variables they carry around the back edge.
//! Soundness: a variable in a `Known` set may still have its value from before the statement read
//!            on a way on: on the normal path, in a failure or defect landing, or in the
//!            continuation after a suspension, before the variable is defined again. The
//!            statement's destination is excluded even if it is read later, because the statement
//!            gives it a new value; its old value is not needed after the definition. The analysis
//!            never leaves out a variable whose old value may be read. It is a may-analysis: a
//!            variable in a set is live on some path, not necessarily on every one.
//! Deopt (unknown): a consumer that gets `Unknown` does not remove the statement, the frame slot or
//!                  the reference it wanted to remove. It must not read `Unknown` as "not live".
//!                  `None` from [`live_after`] is also "no answer", not an empty set.
//! Consumers: LIR DCE and frame pruning (C7/PR-028), rc cleanup (C1/PR-025) and SRA (C5/PR-043).
//! ```
//!
//! # Stages
//!
//! The sets are computed at [`Stage::DefectsExplicit`] and later (`StateMachines`,
//! `RefCounted`), by `crate::heap::live::live_on_lets`, the walk the state machines and the
//! verifier already use.
//!
//! At [`Stage::Flow`] and [`Stage::ClosuresConverted`] the analysis answers `Unknown`. Those
//! stages still have [`Stmt::Protect`](crate::ir::Stmt::Protect) and
//! [`Term::Defect`](crate::ir::Term::Defect): a defect, failure or `break` inside a
//! `protect` leaves through its cleanup, and the walk reads a terminator's live-in from its
//! operands alone, so it would omit a variable that only the cleanup reads on that exit. Closures
//! are also still nested there, and their bodies are not part of the enclosing function's sets.
//! Returning `Unknown` is the conservative answer; it does not guess.
//!
//! # Queries
//!
//! [`live_after`] looks up a `let` statement. Only `let` statements are keyed: they are the
//! definitions a DCE pass removes and the points rc cleanup and frame pruning ask about. A
//! statement that is not keyed gives `None`, which is "no answer".
//!
//! There is no `last_use` query. A single "last statement" cannot be sound on structured control:
//! a variable may be live after several incomparable statements (one per arm of an `if` or
//! `switch`), a loop statement can run any number of times, and a use in a condition or terminator
//! is not a `let` statement at all. A consumer that needs to know whether one `let` statement is a
//! last use of a variable asks [`live_after`] for that statement: if the set omits the variable,
//! no way on reads the variable again. Uses in conditions and terminators are not statements, so
//! this query does not answer for them.
//!
//! There is no `live_across_suspension` query either. The frame of a suspension point is exactly
//! [`Suspend::saved`](crate::ir::Suspend::saved), which the state machines compute from this
//! analysis plus the references the value borrows; C7 reads it there. A saved variable is an
//! operand of the suspending value or is live after it, so it is in the sets of the statements
//! before the suspension point (and, when it is live after the value, in the set [`live_after`]
//! gives for the suspension statement too).
//!
//! # Status
//!
//! B4/PR-023: implemented on the Heap IR behind the B0/PR-002 API. `crate::heap::live` keeps
//! serving the state machines and the verifier; this module exposes its result to the passes.

use rustc_hash::{FxHashMap, FxHashSet};

use crate::heap::live::live_on_lets;
use crate::ir::{FnId, Module, Stage, StmtId, VarId};

use super::{Analysis, AnalysisId, Fact};

/// The variables live after each `let` statement of each function.
pub type LiveSets = FxHashMap<FnId, FxHashMap<StmtId, FxHashSet<VarId>>>;

/// The liveness analysis.
pub struct Liveness;

impl Analysis for Liveness {
    type Output = Fact<LiveSets>;

    const ID: AnalysisId = AnalysisId::Liveness;

    fn analyze(&self, m: &Module) -> Self::Output {
        // Before defects are explicit, a `protect` cleanup can run on a terminator that the walk
        // sees only as its operands (see the module contract). Do not guess there.
        if m.stage < Stage::DefectsExplicit {
            return Fact::Unknown;
        }
        let mut sets = LiveSets::default();
        for (id, f) in m.functions.iter() {
            sets.insert(id, live_on_lets(f));
        }
        Fact::Known(sets)
    }
}

/// The live sets of `m`, or `Unknown` (see the module contract).
pub fn analyze(m: &Module) -> Fact<LiveSets> {
    Liveness.analyze(m)
}

/// The variables live after `s` in `f`, or `None` if the analysis did not key `s`.
///
/// `s`'s own destination is not in the set (see the module contract).
///
/// Only `let` statements are keyed. A consumer must treat `None` as "no answer" and deopt; it must
/// not read it as an empty set.
pub fn live_after(sets: &LiveSets, f: FnId, s: StmtId) -> Option<&FxHashSet<VarId>> {
    sets.get(&f)?.get(&s)
}
