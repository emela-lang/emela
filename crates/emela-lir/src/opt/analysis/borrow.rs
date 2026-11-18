//! Borrow facts: which parameters a function may receive without taking ownership.
//!
//! # Contract
//!
//! ```text
//! Analysis ID: AnalysisId::Borrow
//! Domain / lattice: for each function, a Boolean per parameter: `true` when the parameter may be
//!                   borrowed. The lattice is pointwise implication; `Unknown` is the top element.
//! Transfer: a parameter may be borrowed when every use of it reads the value without taking the
//!           reference and without making a second one. A read is [`Value::Field`],
//!           [`Value::TupleGet`], [`Value::ListHead`], [`Value::ListTail`], [`Value::EnvGet`],
//!           [`Value::EvLookup`], [`Value::AsError`], [`Value::Coerce`], [`Value::Tag`],
//!           [`Value::ListIsEmpty`], [`Value::IsError`], a [`Stmt::Switch`] scrutinee, a
//!           [`Value::Prim`], the index of `Array.set`, and the runtime operations that only read
//!           (`concat`, `show_string`, `debug_print`, `retire`). A position of a direct call is a
//!           read when the callee's parameter may be borrowed; the transfer crosses direct calls
//!           that way. Every other use may keep a reference, so a parameter with it is not
//!           borrowable: every other builtin argument (a builtin may store what it is given), the
//!           array and the value of `Array.set` (the array may be written in place; the value is
//!           stored in it), concurrency arguments, a defect message, `dup` and `drop`.
//! Soundness: `Known(true)` means the callee neither takes the caller's reference nor makes a
//!           second one, so the caller may keep its reference and the value stays uniquely owned
//!           while the call runs. The analysis never marks a parameter borrowed when it may be
//!           stored, returned, failed with, turned into a defect, captured, put into a container,
//!           saved by a suspension frame, duplicated, dropped, passed to an indirect or operation
//!           call, passed to an external function, passed to a builtin other than as the index of
//!           `Array.set`, used as the array or the value of `Array.set`, or passed to a direct
//!           callee at a position the callee does not borrow. A caller that reads a `Known(true)`
//!           position may therefore leave out the `dup` without letting the callee keep the
//!           caller's reference or a copy of it.
//! Deopt (unknown): a consumer that gets `Unknown` leaves the `dup` and the ownership transfer as
//!                  they are. It must not read `Unknown` as "borrowed".
//! Consumers: borrow inference (C2/PR-026), rc cleanup (C1/PR-025) and Perceus reuse (C4/PR-042).
//! ```
//!
//! # Direction of the fixpoint
//!
//! The analysis starts from "no parameter is borrowable" (the bottom of the pointwise lattice) and
//! adds facts until nothing changes: the least fixed point. The transfer is monotone in the
//! callee's facts — a fact is only read to decide that an argument is borrowed, never that it is
//! consumed — so every round can only turn a `false` into a `true`. Starting from "every parameter
//! is borrowable" and removing with proof would need a proof that no path reaches a sink, which
//! the transfer does not produce. Every intermediate result of the ascending iteration is sound,
//! and the least fixed point is the smallest sound set: a cycle whose members only pass the value
//! to each other stays `false`, because no member proves it is safe first.
//!
//! # Stage and the calling convention
//!
//! The facts describe the calling convention *before* reference counting. At [`crate::Stage::RefCounted`]
//! the explicit [`Stmt::Drop`] of a parameter is an ownership transfer, so a read-only parameter
//! that the pass drops at the end is not borrowable at that stage. Borrow inference (C2/PR-026)
//! reads these facts before `insert_rc` and threads a borrowed argument through
//! `heap::rc::value_uses`, `rc_check`, `insert_rc`, the shared `ir.rs` and both backends (C
//! `-DEMELA_LEAK_CHECK`, WASM) before a caller may omit a `dup`; the audit
//! (`/tmp/opencode/audit-c0-e0.md`, C0 "Hazards by item") names the same five places. This module
//! computes the facts only; no pass reads them yet.
//!
//! # Status
//!
//! B6/PR-024: implemented behind the B0/PR-002 API.

use rustc_hash::FxHashMap;

use emela_core::Builtin;

use crate::ir::{
    Callee, FnId, Function, Module, OnFail, Operand, RuntimeOp, SeqId, Stmt, StmtId, Term, Value,
    VarId,
};

use super::{Analysis, AnalysisId, Fact};

/// For each function, one flag per parameter: whether it may be borrowed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BorrowFacts {
    /// Parallel to `Function::params`.
    pub borrowed_params: FxHashMap<FnId, Vec<bool>>,
}

impl BorrowFacts {
    /// Whether parameter `index` of `f` may be borrowed.
    ///
    /// A function or position the analysis did not key answers [`Fact::Unknown`]; a consumer must
    /// deopt on that, not read it as "not borrowable".
    pub fn borrowed(&self, f: FnId, index: usize) -> Fact<bool> {
        match self.borrowed_params.get(&f).and_then(|p| p.get(index)) {
            Some(&b) => Fact::Known(b),
            None => Fact::Unknown,
        }
    }
}

/// The borrow analysis.
pub struct Borrow;

impl Analysis for Borrow {
    type Output = Fact<BorrowFacts>;

    const ID: AnalysisId = AnalysisId::Borrow;

    fn analyze(&self, m: &Module) -> Self::Output {
        borrow_facts(m)
    }
}

/// The borrow facts of `m`, or `Unknown` (see the module contract).
pub fn analyze(m: &Module) -> Fact<BorrowFacts> {
    Borrow.analyze(m)
}

/// The borrow facts of the module, or `Unknown` when a function is neither defined nor external
/// (the call graph cannot be resolved) — the same malformed module `verify` reports as
/// `MissingBody`.
fn borrow_facts(m: &Module) -> Fact<BorrowFacts> {
    if m.functions
        .iter()
        .any(|(_, f)| f.body.is_none() && f.external.is_none())
    {
        return Fact::Unknown;
    }
    let functions: Vec<FnId> = m.functions.iter().map(|(id, _)| id).collect();
    // The bottom of the lattice: nothing is borrowable, and a host function has no body to prove
    // anything about.
    let mut facts = BorrowFacts::default();
    for &id in &functions {
        facts
            .borrowed_params
            .insert(id, vec![false; m.functions[id].params.len()]);
    }
    let bound = facts.borrowed_params.values().map(Vec::len).sum::<usize>() + 1;
    let mut rounds = 0;
    loop {
        let mut changed = false;
        for &id in &functions {
            let f = &m.functions[id];
            if f.external.is_some() {
                continue;
            }
            let next = borrowable(m, f, &facts);
            let current = &facts.borrowed_params[&id];
            debug_assert!(
                current.iter().zip(&next).all(|(old, new)| new >= old),
                "the borrow analysis is not monotone in `{}`",
                f.name
            );
            if *current != next {
                facts.borrowed_params.insert(id, next);
                changed = true;
            }
        }
        if !changed {
            break;
        }
        rounds += 1;
        debug_assert!(rounds <= bound, "the borrow analysis does not converge");
    }
    Fact::Known(facts)
}

/// One round of the fixpoint: which parameters of `f` are borrowable given the current facts of
/// the functions it calls.
fn borrowable(m: &Module, f: &Function, facts: &BorrowFacts) -> Vec<bool> {
    let mut walk = Walk {
        m,
        f,
        facts,
        blocked: Default::default(),
    };
    if let Some(body) = f.body {
        walk.seq(body);
    }
    f.params.iter().map(|p| !walk.blocked.contains(p)).collect()
}

/// The parameters of `f` that some use may take ownership of (directly or through a callee).
struct Walk<'a> {
    m: &'a Module,
    f: &'a Function,
    facts: &'a BorrowFacts,
    blocked: rustc_hash::FxHashSet<VarId>,
}

impl Walk<'_> {
    /// The variables among the operands, as values some use may keep.
    fn keep(&mut self, os: &[Operand]) {
        for o in os {
            if let Operand::Var(v) = o {
                self.blocked.insert(*v);
            }
        }
    }

    fn seq(&mut self, id: SeqId) {
        let seq = &self.f.seqs[id];
        for &s in &seq.stmts {
            self.stmt(s);
        }
        self.term(&seq.term);
    }

    fn stmt(&mut self, id: StmtId) {
        match &self.f.stmts[id] {
            Stmt::Let {
                value,
                on_defect,
                on_suspend,
                ..
            } => {
                self.value(value);
                if let Some(suspend) = on_suspend {
                    // The frame keeps the saved variables after the function returned suspended.
                    for &v in &suspend.saved {
                        self.blocked.insert(v);
                    }
                }
                if let Some(landing) = on_defect {
                    self.seq(*landing);
                }
            }
            Stmt::If { then, else_, .. } => {
                // A condition is a scalar read; the verifier rejects a reference in that position.
                self.seq(*then);
                self.seq(*else_);
            }
            Stmt::Switch { cases, default, .. } => {
                // The scrutinee is read in place.
                for case in cases {
                    self.seq(case.body);
                }
                if let Some(default) = default {
                    self.seq(*default);
                }
            }
            Stmt::Loop { inits, body, .. } => {
                // A loop variable is rebound by `recur`; the initial values move into it.
                self.keep(inits);
                self.seq(*body);
            }
            Stmt::Block { body, .. } => self.seq(*body),
            Stmt::Protect { body, cleanup, .. } => {
                self.seq(*body);
                self.seq(*cleanup);
            }
            // A `dup` makes a second reference, which may outlive the call, and a `drop` gives up
            // a reference the callee would have to own.
            Stmt::Dup(v) | Stmt::Drop(v) => {
                self.blocked.insert(*v);
            }
        }
    }

    fn term(&mut self, t: &Term) {
        match t {
            // A yielded, broken or recurring value moves to a compound statement's destination or
            // a loop variable; the rules do not follow it there, so it is not provably a read.
            Term::Yield(Some(v)) | Term::Break { value: Some(v), .. } => {
                self.keep(std::slice::from_ref(v))
            }
            Term::Yield(None) | Term::Break { value: None, .. } => {}
            Term::Recur(args) => self.keep(args),
            Term::Return(Some(v)) => self.keep(std::slice::from_ref(v)),
            Term::Return(None) => {}
            Term::Fail(v) | Term::Defect(v, _) => self.keep(std::slice::from_ref(v)),
            Term::Unwind | Term::Unreachable => {}
        }
    }

    fn value(&mut self, v: &Value) {
        match v {
            Value::Const(_) | Value::FnRef(_) => {}
            // A primitive operation reads its operands (the strings it compares included).
            Value::Prim { .. } => {}
            Value::Call {
                callee,
                args,
                on_fail,
                ..
            } => {
                self.call(callee, args);
                // A failure landing is part of the function.
                if let Some(OnFail::Handle { body, .. }) = on_fail {
                    self.seq(*body);
                }
            }
            Value::Builtin { op, args } => {
                if *op == Builtin::ArraySet {
                    // The runtime writes the array in place when its actual count is one, so the
                    // array argument would be mutated through the caller's reference. The value is
                    // stored in the array; the index is the only argument that is read.
                    for (i, o) in args.iter().enumerate() {
                        if i == 1 {
                            continue;
                        }
                        if let Operand::Var(v) = o {
                            self.blocked.insert(*v);
                        }
                    }
                } else {
                    // A builtin may store what it is given (the runtime dups what it keeps); the
                    // analysis does not know the shape of each one.
                    self.keep(args);
                }
            }
            // Containers, closures, evidence tables and handlers keep what is given to them.
            Value::Make { args, .. } | Value::Tuple(args) => self.keep(args),
            Value::List { elems, tail } => {
                self.keep(elems);
                if let Some(tail) = tail {
                    self.keep(std::slice::from_ref(tail));
                }
            }
            Value::Closure(c) => {
                for &v in &c.captures {
                    self.blocked.insert(v);
                }
            }
            Value::MakeClosure { captures, .. } => {
                for &v in captures {
                    self.blocked.insert(v);
                }
            }
            Value::EvExtend { base, bindings } => {
                if let Some(base) = base {
                    self.blocked.insert(*base);
                }
                for (_, v) in bindings {
                    self.blocked.insert(*v);
                }
            }
            Value::Handler { ops, .. } => {
                for &v in ops {
                    self.blocked.insert(v);
                }
            }
            // A concurrency operation stores its arguments in runtime objects.
            Value::Fiber { args, .. } => self.keep(args),
            Value::Runtime { op, args } => {
                match op {
                    // It consumes the pending defect it is given, and a defect message is kept as
                    // the pending defect.
                    RuntimeOp::DefectResume | RuntimeOp::Defect | RuntimeOp::DefectAt => {
                        self.keep(args)
                    }
                    // These read their arguments and keep no reference.
                    RuntimeOp::ShowString
                    | RuntimeOp::Concat
                    | RuntimeOp::DebugPrint
                    | RuntimeOp::Retire
                    | RuntimeOp::DefectTake
                    | RuntimeOp::Hold
                    | RuntimeOp::Unhold
                    | RuntimeOp::Yield
                    | RuntimeOp::YieldPerform => {}
                }
            }
            // Reads: the value stays in its variable.
            Value::Field { .. }
            | Value::Tag(_)
            | Value::TupleGet { .. }
            | Value::ListIsEmpty(_)
            | Value::ListHead(_)
            | Value::ListTail(_)
            | Value::EnvGet { .. }
            | Value::EvLookup { .. }
            | Value::IsError { .. }
            | Value::AsError { .. }
            | Value::Coerce(_) => {}
        }
    }

    /// Charges a call to its arguments: a direct call to a function the module defines borrows
    /// the positions its callee borrows; every other call may keep every argument.
    fn call(&mut self, callee: &Callee, args: &[Operand]) {
        match callee {
            Callee::Direct(g) => {
                if self.m.functions[*g].external.is_some() {
                    // The host calling convention is not described by these facts.
                    self.keep(args);
                    return;
                }
                let params = self.facts.borrowed_params.get(g);
                for (i, o) in args.iter().enumerate() {
                    let borrowed = params.is_some_and(|p| p.get(i).copied().unwrap_or(false));
                    if !borrowed && let Operand::Var(v) = o {
                        self.blocked.insert(*v);
                    }
                }
            }
            // The callee is a value the module does not resolve (an indirect or operation call):
            // nothing is known about what it keeps.
            Callee::Indirect(_) | Callee::Op { .. } => self.keep(args),
        }
    }
}
