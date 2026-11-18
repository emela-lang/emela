//! Ownership facts: which references are known to be uniquely owned (reference count one).
//!
//! # Contract
//!
//! ```text
//! Analysis ID: AnalysisId::Ownership
//! Domain / lattice: for each function, for each `let` statement, a fact per counted variable
//!                   after the statement. `Known(true)` means the variable owns the only reference
//!                   to its value; `Known(false)` means it is provably not the only owner (it is a
//!                   view, a second binding, or some use takes or duplicates the reference);
//!                   a variable without an entry is `Unknown`, the top element.
//! Transfer: forward over the control flow, derived from the reference counting pass
//!           (`heap::rc`). A destination is `Known(true)` when its value makes a new reference
//!           (`make`, `list`, `tuple`, `make_closure`) and it is not a view and no frame saves it.
//!           A view (`field`, `tuple_get`, `list_head`, `list_tail`, `env_get`, `ev_lookup`,
//!           `as_error`, `coerce` of a `ref`, the bases [`heap::rc::Bases`] records) is
//!           `Known(false)`: its value lives in its base, and the counting pass gives it a `dup`
//!           when it is live after the statement. A `dup`, a `drop`, a consuming use (a call, a
//!           container, a capture, an evidence table, a handler, `Array.set`, `defect_resume`, a
//!           yielded/broken/recurred/returned/failed value), a saved variable and the base of a
//!           second binding are `Known(false)`. A builtin or concurrency operation may store an
//!           argument, so its arguments are `Known(false)` as well; a primitive operation reads. A
//!           direct call borrows an argument at a position whose callee parameter the borrow
//!           analysis proves borrowable; the reference then stays with the caller. Everything
//!           else — a parameter, a call result, a compound statement's result — is `Unknown`
//!           unless proven otherwise. Joins intersect the `Known(true)` facts and union the
//!           `Known(false)` ones; a loop iterates to a fixpoint. A variable is uniquely owned only
//!           while no other live variable may alias it. The analysis is defined where B4 liveness
//!           is, and reads whether a view is live after its own definition (the counting pass then
//!           gives it the `dup` that shares the value) from the uses of the destination, because
//!           [`liveness::live_after`] excludes a statement's own destination by design. The
//!           `as_error` and `coerce` views share the base's own reference, not an inner one.
//! Soundness: `Known(true)` means reference count one on every path to that point, so mutating or
//!            reusing the value in place is not observable. The analysis never claims uniqueness
//!            for a value that may be shared, and it never claims it for a value it cannot follow.
//! Deopt (unknown): a consumer that gets `Unknown` does not reuse the constructor, mutate in place
//!                  or remove the copy. It must not read `Unknown` as "uniquely owned"; the deopt
//!                  rule is "no answer means no change". `Known(false)` is also a deopt, not an
//!                  error: it says only that uniqueness is disproved.
//! Consumers: Perceus reuse / FBIP (C4/PR-042), in-place array and record updates, and the
//!            `Array.set` decision. Borrow inference (C2/PR-026) reads the borrow facts this
//!            analysis uses, not the ownership set.
//! ```
//!
//! # Stage
//!
//! The facts are computed where liveness is defined, [`crate::Stage::DefectsExplicit`] and later, and
//! they use [`super::liveness`] and [`super::borrow`]. Before that stage the answer is `Unknown`
//! (no liveness: a `protect` cleanup is not modelled). At [`crate::Stage::RefCounted`] the explicit
//! `dup`/`drop` are read as facts about the reference, so the analysis agrees with the verifier's
//! balance on those stages too.
//!
//! # No consumer yet
//!
//! This module computes facts only; no pass reads them. Before Perceus reuse (C4/PR-042) may reuse
//! a constructor or an in-place update, the calling convention has to be threaded through
//! `heap::rc::value_uses`, `rc_check`, `insert_rc`, the shared `ir.rs` and both backends (C
//! `-DEMELA_LEAK_CHECK`, WASM): a borrowed argument has to be represented consistently in all of
//! them, or the verifier rejects valid IR (`/tmp/opencode/audit-c0-e0.md`, C0, "Hazards by item").
//!
//! # Status
//!
//! B6/PR-024: implemented behind the B0/PR-002 API.

use rustc_hash::{FxHashMap, FxHashSet};

use crate::heap::rc::{Mode, counted, is_view, value_uses, view_base};
use crate::ir::{
    Callee, FnId, Function, LabelId, Module, OnFail, Operand, SeqId, Stmt, StmtId, SwitchOn, Term,
    Value, VarId,
};
use crate::repr::Repr;

use super::{Analysis, AnalysisId, Fact, borrow, liveness};

/// The ownership facts of one variable after one statement. A missing variable is `Unknown`.
pub type StatementFacts = FxHashMap<VarId, Fact<bool>>;

/// For each function, for each `let` statement, the ownership facts after it.
///
/// A missing function, statement or variable is `Unknown`, not "not unique"; see [`unique_after`].
pub type UniqueRefs = FxHashMap<FnId, FxHashMap<StmtId, StatementFacts>>;

/// The ownership fact of `v` after the `let` `s` of `f`.
///
/// `Fact::Unknown` means the analysis has no answer (the statement is not a keyed `let`, or the
/// variable was not followed); a consumer must deopt, not read it as "not unique".
pub fn unique_after(refs: &UniqueRefs, f: FnId, s: StmtId, v: VarId) -> Fact<bool> {
    facts_after(refs, f, s)
        .and_then(|facts| facts.get(&v))
        .copied()
        .unwrap_or(Fact::Unknown)
}

/// The facts after the `let` `s` of `f`, or `None` if the statement is not keyed.
pub fn facts_after(refs: &UniqueRefs, f: FnId, s: StmtId) -> Option<&StatementFacts> {
    refs.get(&f)?.get(&s)
}

/// The ownership analysis.
pub struct Ownership;

impl Analysis for Ownership {
    type Output = Fact<UniqueRefs>;

    const ID: AnalysisId = AnalysisId::Ownership;

    fn analyze(&self, m: &Module) -> Self::Output {
        // The ownership facts are defined where liveness is: before `DefectsExplicit` a `protect`
        // cleanup can be reached through an exit the liveness walk does not model, so neither
        // analysis guesses.
        if liveness::analyze(m).is_unknown() {
            return Fact::Unknown;
        }
        let borrows = match borrow::analyze(m) {
            Fact::Known(borrows) => borrows,
            Fact::Unknown => return Fact::Unknown,
        };
        let mut refs = UniqueRefs::default();
        for (id, f) in m.functions.iter() {
            if f.body.is_none() {
                continue;
            }
            refs.insert(id, function_facts(m, id, f, &borrows));
        }
        Fact::Known(refs)
    }
}

/// The uniquely owned references of `m`, or `Unknown` (see the module contract).
pub fn analyze(m: &Module) -> Fact<UniqueRefs> {
    Ownership.analyze(m)
}

// The forward dataflow.
//
// A node is the point before a statement or before a sequence's terminator. The state maps the
// counted variables the analysis follows to a two-valued fact; a variable without an entry is
// unknown. The join is "the variable is unique only if every predecessor says so, and not unique
// if any predecessor says so".

/// A program point of one function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Node {
    /// Before statement `0` of the sequence.
    Stmt(SeqId, usize),
    /// Before the terminator of the sequence.
    Term(SeqId),
}

fn entry_of(f: &Function, id: SeqId) -> Node {
    if f.seqs[id].stmts.is_empty() {
        Node::Term(id)
    } else {
        Node::Stmt(id, 0)
    }
}

#[derive(Default)]
struct Cfg {
    /// The successors of each point, in a deterministic order.
    succ: FxHashMap<Node, Vec<Node>>,
    /// The innermost loop's variables for each sequence, for [`Term::Recur`].
    loop_vars: FxHashMap<SeqId, Vec<VarId>>,
    /// Where a [`Term::Break`] to a label continues.
    label_exit: FxHashMap<LabelId, Option<Node>>,
}

struct Builder<'a> {
    f: &'a Function,
    cfg: Cfg,
}

impl Builder<'_> {
    fn next(&self, id: SeqId, i: usize) -> Node {
        if i + 1 < self.f.seqs[id].stmts.len() {
            Node::Stmt(id, i + 1)
        } else {
            Node::Term(id)
        }
    }

    /// Builds the points of `id`, whose normal yield continues at `after` and whose `recur`
    /// targets `loop_` (the innermost loop's body entry and variables).
    fn build(&mut self, id: SeqId, after: Option<Node>, loop_: Option<(Node, Vec<VarId>)>) {
        if let Some((_, vars)) = &loop_ {
            self.cfg.loop_vars.insert(id, vars.clone());
        }
        let len = self.f.seqs[id].stmts.len();
        for i in 0..len {
            let sid = self.f.seqs[id].stmts[i];
            let node = Node::Stmt(id, i);
            let next = self.next(id, i);
            let mut out = Vec::new();
            match self.f.stmts[sid].clone() {
                Stmt::Let {
                    value, on_defect, ..
                } => {
                    out.push(next);
                    if let Some(landing) = on_defect {
                        self.build(landing, None, loop_.clone());
                        out.push(entry_of(self.f, landing));
                    }
                    let fail = match value {
                        Value::Call {
                            on_fail: Some(OnFail::Handle { body, .. }),
                            ..
                        }
                        | Value::Fiber {
                            on_fail: Some(OnFail::Handle { body, .. }),
                            ..
                        } => Some(body),
                        _ => None,
                    };
                    if let Some(body) = fail {
                        self.build(body, None, loop_.clone());
                        out.push(entry_of(self.f, body));
                    }
                }
                Stmt::If { then, else_, .. } => {
                    self.build(then, Some(next), loop_.clone());
                    self.build(else_, Some(next), loop_.clone());
                    out.push(entry_of(self.f, then));
                    out.push(entry_of(self.f, else_));
                }
                Stmt::Switch { cases, default, .. } => {
                    for case in &cases {
                        self.build(case.body, Some(next), loop_.clone());
                        out.push(entry_of(self.f, case.body));
                    }
                    if let Some(default) = default {
                        self.build(default, Some(next), loop_.clone());
                        out.push(entry_of(self.f, default));
                    }
                }
                Stmt::Loop { vars, body, .. } => {
                    let head = entry_of(self.f, body);
                    self.build(body, Some(next), Some((head, vars)));
                    out.push(head);
                }
                Stmt::Block { label, body, .. } => {
                    self.cfg.label_exit.insert(label, Some(next));
                    self.build(body, Some(next), loop_.clone());
                    out.push(entry_of(self.f, body));
                }
                Stmt::Protect { body, cleanup, .. } => {
                    // Not present from `DefectsExplicit` on; built for completeness.
                    let cleanup_entry = entry_of(self.f, cleanup);
                    self.build(body, Some(cleanup_entry), loop_.clone());
                    self.build(cleanup, after, loop_.clone());
                    out.push(entry_of(self.f, body));
                }
                Stmt::Dup(_) | Stmt::Drop(_) => out.push(next),
            }
            self.cfg.succ.insert(node, out);
        }

        let term = &self.f.seqs[id].term;
        let mut out = Vec::new();
        match term {
            Term::Yield(_) => {
                if let Some(after) = after {
                    out.push(after);
                }
            }
            Term::Break { label, .. } => {
                if let Some(Some(exit)) = self.cfg.label_exit.get(label) {
                    out.push(*exit);
                }
            }
            Term::Recur(_) => {
                if let Some((head, _)) = loop_ {
                    out.push(head);
                }
            }
            Term::Return(_)
            | Term::Fail(_)
            | Term::Defect(..)
            | Term::Unwind
            | Term::Unreachable => {}
        }
        self.cfg.succ.insert(Node::Term(id), out);
    }
}

/// The per-function facts, from the function's entry to a fixpoint over the control-flow graph.
fn function_facts(
    m: &Module,
    f_id: FnId,
    f: &Function,
    borrows: &borrow::BorrowFacts,
) -> FxHashMap<StmtId, StatementFacts> {
    let Some(body) = f.body else {
        return FxHashMap::default();
    };
    let used = uses_of(f);
    let mut builder = Builder {
        f,
        cfg: Cfg::default(),
    };
    builder.build(body, None, None);
    let cfg = builder.cfg;

    let mut entry = FxHashMap::default();
    for (index, &p) in f.params.iter().enumerate() {
        if !counted(f, p) {
            continue;
        }
        // The environment parameter of a lifted function is borrowed by the caller, unless a frame
        // may keep it (`owned_env`); reference counting then dups it at the start, so it is not the
        // only owner either way. Any other parameter is kept by the caller when the borrow
        // analysis proves the callee may borrow it; an owned parameter may or may not have been
        // duplicated at the call site, so its uniqueness is not known.
        let env = index == 0 && f.env.is_some();
        let borrowed = borrows.borrowed(f_id, index) == Fact::Known(true);
        if env || borrowed {
            entry.insert(p, Fact::Known(false));
        }
    }

    let start = entry_of(f, body);
    let mut states: FxHashMap<Node, StatementFacts> = FxHashMap::default();
    let mut facts: FxHashMap<StmtId, StatementFacts> = FxHashMap::default();
    states.insert(start, entry);
    let mut work = std::collections::VecDeque::from([start]);
    let bound = (f.vars.len() + 1) * (f.stmts.len() + 1) * 3 + 1;
    let mut steps = 0;
    while let Some(node) = work.pop_front() {
        steps += 1;
        debug_assert!(steps <= bound, "the ownership analysis does not converge");
        let state = states[&node].clone();
        let out = match node {
            Node::Stmt(id, i) => {
                let sid = f.seqs[id].stmts[i];
                let out = transfer(m, f, borrows, &used, sid, state);
                if let Stmt::Let { .. } = f.stmts[sid] {
                    let record = match facts.get(&sid) {
                        Some(record) => join(record, &out),
                        None => out.clone(),
                    };
                    facts.insert(sid, record);
                }
                out
            }
            Node::Term(id) => term_transfer(f, &cfg, id, state),
        };
        for &succ in cfg.succ.get(&node).into_iter().flatten() {
            let covered = match states.get(&succ) {
                Some(old) => join(old, &out),
                None => out.clone(),
            };
            if states.get(&succ) != Some(&covered) {
                states.insert(succ, covered);
                work.push_back(succ);
            }
        }
    }
    facts
}

/// The join of two states: a variable is unique only if both agree, and not unique if either says
/// so. An absent entry is unknown, so it cannot support a `Known(true)`.
fn join(a: &StatementFacts, b: &StatementFacts) -> StatementFacts {
    let mut out = FxHashMap::default();
    for (&v, fa) in a {
        match b.get(&v) {
            Some(fb) => {
                let fact = match (fa, fb) {
                    (Fact::Known(true), Fact::Known(true)) => Fact::Known(true),
                    _ => Fact::Known(false),
                };
                out.insert(v, fact);
            }
            // true joined with unknown stays unknown; false survives on its own.
            None => {
                if *fa == Fact::Known(false) {
                    out.insert(v, Fact::Known(false));
                }
            }
        }
    }
    for (&v, fb) in b {
        if !a.contains_key(&v) && *fb == Fact::Known(false) {
            out.insert(v, Fact::Known(false));
        }
    }
    out
}

fn not_unique(state: &mut StatementFacts, v: VarId) {
    state.insert(v, Fact::Known(false));
}

/// The state after `sid`.
fn transfer(
    m: &Module,
    f: &Function,
    borrows: &borrow::BorrowFacts,
    used: &FxHashSet<VarId>,
    sid: StmtId,
    mut state: StatementFacts,
) -> StatementFacts {
    match &f.stmts[sid] {
        Stmt::Let {
            dst,
            value,
            on_suspend,
            ..
        } => {
            apply_uses(m, f, borrows, value, &mut state);

            if let Some(d) = *dst
                && counted(f, d)
            {
                let saved = on_suspend.as_ref().is_some_and(|s| s.saved.contains(&d));
                let fact = if saved {
                    Fact::Known(false)
                } else if is_fresh(value) && !is_view(f, value) {
                    Fact::Known(true)
                } else if is_view(f, value) || matches!(value, Value::Const(_)) {
                    Fact::Known(false)
                } else {
                    // A parameter, a call or builtin result, a compound value: not proven.
                    state.remove(&d);
                    Fact::Unknown
                };
                if let Fact::Known(fact) = fact {
                    state.insert(d, Fact::Known(fact));
                }
            }

            // A view that is used after its definition gets a `dup` from the counting pass, so the
            // value it reads has a second reference. A view of an inner reference (`field`,
            // `list_head`, …) shares that inner value with its base; a view of the same reference
            // (`coerce`, `as_error`) shares the base's own reference, so the base is not unique.
            // A view that is never used again is dropped without a `dup` and shares nothing. B4
            // decides the live variables; this reads the use of the destination from the same
            // walk, because the liveness sets exclude a statement's own destination.
            if is_view(f, value)
                && let Some(base) = view_base(f, value)
                && counted(f, base)
                && same_value_view(f, value)
                && dst.is_some_and(|d| used.contains(&d))
            {
                not_unique(&mut state, base);
            }

            // A suspension edge moves the saved variables into a heap frame.
            if let Some(suspend) = on_suspend {
                for &v in &suspend.saved {
                    if counted(f, v) {
                        not_unique(&mut state, v);
                    }
                }
            }
        }
        Stmt::If { dst, .. } | Stmt::Switch { dst, .. } | Stmt::Block { dst, .. } => {
            if let Some(d) = dst {
                state.remove(d);
            }
        }
        Stmt::Loop {
            dst, vars, inits, ..
        } => {
            if let Some(d) = dst {
                state.remove(d);
            }
            // The initial values move into the loop variables, which are rebound at the head and
            // by every `recur`.
            for o in inits {
                if let Operand::Var(v) = o
                    && counted(f, *v)
                {
                    not_unique(&mut state, *v);
                }
            }
            for &v in vars {
                state.remove(&v);
            }
        }
        Stmt::Protect { dst, .. } => {
            if let Some(d) = dst {
                state.remove(d);
            }
        }
        // A second reference exists after a `dup`, and the value leaves on a `drop`.
        Stmt::Dup(v) | Stmt::Drop(v) => not_unique(&mut state, *v),
    }
    state
}

/// The state after the terminator of `id`.
fn term_transfer(f: &Function, cfg: &Cfg, id: SeqId, mut state: StatementFacts) -> StatementFacts {
    match &f.seqs[id].term {
        Term::Yield(Some(v)) | Term::Break { value: Some(v), .. } => {
            if let Operand::Var(v) = v
                && counted(f, *v)
            {
                not_unique(&mut state, *v);
            }
        }
        Term::Recur(args) => {
            for v in cfg.loop_vars.get(&id).into_iter().flatten() {
                state.remove(v);
            }
            for o in args {
                if let Operand::Var(v) = o
                    && counted(f, *v)
                {
                    not_unique(&mut state, *v);
                }
            }
        }
        Term::Return(Some(v)) => {
            if let Operand::Var(v) = v
                && counted(f, *v)
            {
                not_unique(&mut state, *v);
            }
        }
        Term::Fail(v) | Term::Defect(v, _) => {
            if let Operand::Var(v) = v
                && counted(f, *v)
            {
                not_unique(&mut state, *v);
            }
        }
        Term::Yield(None)
        | Term::Return(None)
        | Term::Break { value: None, .. }
        | Term::Unwind
        | Term::Unreachable => {}
    }
    state
}

/// Applies the reference counting use modes of `value` to the state. A call is charged by its
/// borrowing convention: a direct callee's borrowable parameter keeps the argument owned by the
/// caller, every other argument may be kept or duplicated.
fn apply_uses(
    m: &Module,
    f: &Function,
    borrows: &borrow::BorrowFacts,
    value: &Value,
    state: &mut StatementFacts,
) {
    match value {
        Value::Call { callee, args, .. } => match callee {
            Callee::Direct(g) => {
                let external = m.functions[*g].external.is_some();
                for (i, o) in args.iter().enumerate() {
                    let Operand::Var(v) = o else { continue };
                    if !counted(f, *v) {
                        continue;
                    }
                    let borrowed = !external && borrows.borrowed(*g, i) == Fact::Known(true);
                    if !borrowed {
                        not_unique(state, *v);
                    }
                }
            }
            // The callee value of an indirect or operation call is borrowed; the arguments are
            // not.
            Callee::Indirect(_) | Callee::Op { .. } => {
                for o in args {
                    if let Operand::Var(v) = o
                        && counted(f, *v)
                    {
                        not_unique(state, *v);
                    }
                }
            }
        },
        // The runtime dups what it keeps, but a builtin or concurrency operation may store an
        // argument in an object this analysis does not follow. Only a primitive operation is a
        // pure read.
        Value::Builtin { args, .. } | Value::Fiber { args, .. } => {
            for o in args {
                if let Operand::Var(v) = o
                    && counted(f, *v)
                {
                    not_unique(state, *v);
                }
            }
        }
        _ => {
            let mut uses = Vec::new();
            value_uses(m, f, value, &mut |v, mode| uses.push((v, mode)));
            for (v, mode) in uses {
                if mode == Mode::Consume {
                    not_unique(state, v);
                }
            }
        }
    }
}

/// A value that makes a new reference when it is bound to its destination.
fn is_fresh(value: &Value) -> bool {
    matches!(
        value,
        Value::Make { .. } | Value::Tuple(_) | Value::List { .. } | Value::MakeClosure { .. }
    )
}

/// A view that reads the same reference as its base (`coerce` of a `ref`, `as_error`), rather than
/// an inner value held by the base.
fn same_value_view(f: &Function, value: &Value) -> bool {
    match value {
        Value::Coerce(x) => f.vars[*x].repr == Repr::Ref,
        Value::AsError { .. } => true,
        _ => false,
    }
}

/// Every variable some statement or terminator reads, including the unreachable ones. A variable
/// is used only after its definition, so this is the liveness fact the view rule needs for a
/// statement's own destination, which [`liveness::live_after`] excludes by design.
fn uses_of(f: &Function) -> FxHashSet<VarId> {
    let mut used = FxHashSet::default();
    for (_, stmt) in f.stmts.iter() {
        match stmt {
            Stmt::Let {
                value, on_suspend, ..
            } => {
                value.for_each_var(|v| {
                    used.insert(v);
                });
                if let Some(suspend) = on_suspend {
                    used.extend(suspend.saved.iter().copied());
                }
            }
            Stmt::If { cond, .. } => {
                if let Operand::Var(v) = cond {
                    used.insert(*v);
                }
            }
            Stmt::Switch { on, .. } => {
                let (SwitchOn::Tag(v) | SwitchOn::Lit(v)) = on;
                used.insert(*v);
            }
            Stmt::Loop { inits, .. } => {
                for o in inits {
                    if let Operand::Var(v) = o {
                        used.insert(*v);
                    }
                }
            }
            Stmt::Block { .. } | Stmt::Protect { .. } => {}
            Stmt::Dup(v) | Stmt::Drop(v) => {
                used.insert(*v);
            }
        }
    }
    for (_, seq) in f.seqs.iter() {
        let operands = |os: &[Operand], used: &mut FxHashSet<VarId>| {
            for o in os {
                if let Operand::Var(v) = o {
                    used.insert(*v);
                }
            }
        };
        match &seq.term {
            Term::Yield(v) | Term::Return(v) | Term::Break { value: v, .. } => {
                operands(v.as_slice(), &mut used);
            }
            Term::Fail(v) | Term::Defect(v, _) => {
                if let Operand::Var(v) = v {
                    used.insert(*v);
                }
            }
            Term::Recur(args) => operands(args, &mut used),
            Term::Unwind | Term::Unreachable => {}
        }
    }
    used
}
