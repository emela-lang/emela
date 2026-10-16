//! Explicit defect paths: defects and the cleanups of `protect` become ordinary control.
//!
//! A defect is passed back like a failure, along its own path, without exceptions or unwinding
//! tables. The runtime holds the pending defect (one slot per fiber); a function that raises one
//! (`runtime defect(msg)`) or gets one back from a call leaves with [`Term::Unwind`], and each call
//! that may come back with a pending defect has a landing (`on_defect { … }`) that runs instead of
//! the rest of the sequence. C and WASM emit a call's landing as a test of the pending slot after
//! the call (`if (pending) { …landing… }`), and `unwind` as a return of any value.
//!
//! What the pass changes, in each function that does not suspend:
//!
//! - `defect(msg)` becomes `runtime defect(msg)` followed by `unwind`.
//! - Integer division and remainder by a divisor that is not a nonzero constant are preceded by a
//!   test that raises `division by zero` (spec 16.1); backends may then divide without checking.
//! - Every call that may raise a defect gets a landing that unwinds: direct calls to functions
//!   that may ([`Function::defects`], found by a fixpoint over the call graph), calls of function
//!   values and operations, and the builtins and concurrency operations listed in
//!   [`builtin_may_defect`] and [`fiber_may_defect`].
//! - Failure edges get landings of their own ([`OnFail::Handle`]) that break to the block or fail
//!   the function.
//! - `protect { body } cleanup { c }` becomes a block of `body` in which a copy of `c` runs on
//!   every way out of `body`: before its final `yield`, before a `break` to a block outside it,
//!   before `return` and `fail` (including those of failure landings), and before `unwind`. On the
//!   way out with a defect, the copy runs with the defect taken out of the pending slot
//!   (`runtime defect_take()`) and put back afterwards (`runtime defect_resume(d)`), so that the
//!   calls in the cleanup can raise and detect their own defects; a defect raised by the cleanup
//!   becomes a note of the first one (spec 14.4) and the unwinding goes on. A defect raised by the
//!   cleanup on another way out replaces that way out. The cleanup is copied once per way out;
//!   cleanups are small (a call of the release and `retire`).

use emela_core::{Builtin, FiberOp, TypeId};
use rustc_hash::FxHashMap;

use super::copy::{Copier, empty_like};
use crate::ir::{
    Callee, Const, FnId, Function, LabelId, Module, OnFail, Operand, PrimOp, PrimTy, RuntimeOp,
    Seq, SeqId, Stage, Stmt, StmtId, Term, Value, VarData, VarId,
};
use crate::repr::Repr;

/// Makes defects and cleanups explicit in every function that does not suspend, and sets
/// [`Function::defects`] on every function. The module must be at [`Stage::ClosuresConverted`];
/// it is at [`Stage::DefectsExplicit`] afterwards.
pub fn explicit_defects(m: &mut Module) {
    assert_eq!(
        m.stage,
        Stage::ClosuresConverted,
        "explicit defects run after closure conversion"
    );
    let flags = analyse(m);
    for (id, f) in m.functions.iter_mut() {
        f.defects = flags[ix(id)];
    }
    let ids: Vec<FnId> = m.functions.iter().map(|(id, _)| id).collect();
    for id in ids {
        let f = &m.functions[id];
        if f.suspends || f.body.is_none() {
            continue;
        }
        let new = Explicit::run(m, f);
        m.functions[id] = new;
    }
    m.stage = Stage::DefectsExplicit;
}

fn ix<T>(id: la_arena::Idx<T>) -> usize {
    u32::from(id.into_raw()) as usize
}

/// Whether the value may come back with a pending defect (from [`Stage::DefectsExplicit`], when
/// [`Function::defects`] is set).
pub(crate) fn may_defect(m: &Module, v: &Value) -> bool {
    match v {
        Value::Call {
            callee: Callee::Direct(f),
            ..
        } => m.functions[*f].defects,
        Value::Call { .. } => true,
        Value::Builtin { op, .. } => builtin_may_defect(*op),
        Value::Fiber { op, .. } => fiber_may_defect(*op),
        _ => false,
    }
}

/// Whether the builtin may raise a defect.
pub fn builtin_may_defect(op: Builtin) -> bool {
    match op {
        // A negative shift amount (spec 7.5).
        Builtin::IntShiftLeft
        | Builtin::IntShiftRight
        | Builtin::IntShiftRightUnsigned
        | Builtin::Int64ShiftLeft
        | Builtin::Int64ShiftRight
        | Builtin::Int64ShiftRightUnsigned
        // NaN, infinities and values out of the range of Int (spec 16.1).
        | Builtin::FloatFloor
        | Builtin::FloatCeil
        | Builtin::FloatRound
        | Builtin::FloatTruncate
        | Builtin::Panic
        | Builtin::Todo => true,
        Builtin::StringLength
        | Builtin::StringByteSize
        | Builtin::StringAppend
        | Builtin::StringContains
        | Builtin::StringStartsWith
        | Builtin::StringEndsWith
        | Builtin::StringSplit
        | Builtin::StringChars
        | Builtin::StringJoin
        | Builtin::StringTrim
        | Builtin::StringCodePoints
        | Builtin::StringFromCodePoint
        | Builtin::IntToString
        | Builtin::IntParse
        | Builtin::IntCheckedAdd
        | Builtin::IntBitAnd
        | Builtin::IntBitOr
        | Builtin::IntBitXor
        | Builtin::IntBitNot
        | Builtin::Int64FromInt
        | Builtin::Int64ToString
        | Builtin::Int64ToInt
        | Builtin::Int64BitAnd
        | Builtin::Int64BitOr
        | Builtin::Int64BitXor
        | Builtin::Int64BitNot
        | Builtin::FloatToString
        | Builtin::FloatFromInt
        | Builtin::FloatParse
        | Builtin::Dbg
        | Builtin::CellNew
        | Builtin::CellGet
        | Builtin::CellSet => false,
    }
}

/// Whether the concurrency operation may raise a defect in the current fiber.
pub fn fiber_may_defect(op: FiberOp) -> bool {
    match op {
        // A child's defect, an interrupted child, a raised cause, or a scope's body.
        FiberOp::Scope { .. }
        | FiberOp::Join
        | FiberOp::Await { .. }
        | FiberOp::Interrupt
        | FiberOp::DeferredAwait
        | FiberOp::CauseRaise => true,
        // `attempt` turns the function's defects into a `Failure`.
        FiberOp::Attempt { .. }
        | FiberOp::Fork
        | FiberOp::Deferred
        | FiberOp::Complete
        | FiberOp::Poll
        | FiberOp::CauseKind { .. }
        | FiberOp::CauseToString => false,
    }
}

/// Whether an integer division or remainder needs a test for a zero divisor.
fn needs_zero_test(op: PrimOp, ty: PrimTy, args: &[Operand]) -> bool {
    matches!(op, PrimOp::Div | PrimOp::Rem)
        && matches!(ty, PrimTy::I32 | PrimTy::I64)
        && !matches!(
            args.get(1),
            Some(Operand::Const(Const::Int(n))) if *n != 0
        )
        && !matches!(
            args.get(1),
            Some(Operand::Const(Const::Int64(n))) if *n != 0
        )
}

/// Applies `visit` to every statement and terminator of the body, nested ones included.
pub(crate) fn visit(f: &Function, seq: SeqId, visit: &mut dyn FnMut(Option<&Stmt>, &Term)) {
    let s = &f.seqs[seq];
    for &id in &s.stmts {
        let stmt = &f.stmts[id];
        visit(Some(stmt), &Term::Unreachable);
        let mut inner = |q: SeqId| self::visit(f, q, visit);
        match stmt {
            Stmt::Let {
                value, on_defect, ..
            } => {
                match value {
                    Value::Closure(c) => inner(c.body),
                    Value::Call {
                        on_fail: Some(OnFail::Handle { body, .. }),
                        ..
                    }
                    | Value::Fiber {
                        on_fail: Some(OnFail::Handle { body, .. }),
                        ..
                    } => inner(*body),
                    _ => {}
                }
                if let Some(l) = on_defect {
                    inner(*l);
                }
            }
            Stmt::If { then, else_, .. } => {
                inner(*then);
                inner(*else_);
            }
            Stmt::Switch { cases, default, .. } => {
                for c in cases {
                    inner(c.body);
                }
                if let Some(d) = default {
                    inner(*d);
                }
            }
            Stmt::Loop { body, .. } | Stmt::Block { body, .. } => inner(*body),
            Stmt::Protect { body, cleanup, .. } => {
                inner(*body);
                inner(*cleanup);
            }
            Stmt::Dup(_) | Stmt::Drop(_) => {}
        }
    }
    visit(None, &s.term);
}

/// Which functions may come back with a pending defect: external and suspending ones, those that
/// raise one or divide by a variable, and those that make a call that may.
fn analyse(m: &Module) -> Vec<bool> {
    let n = m.functions.len();
    let mut flags = vec![false; n];
    let mut callees: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (id, f) in m.functions.iter() {
        let i = ix(id);
        let Some(body) = f.body else {
            flags[i] = true;
            continue;
        };
        let mut raises = f.suspends;
        visit(f, body, &mut |stmt, term| match stmt {
            Some(Stmt::Let { value, .. }) => match value {
                Value::Call {
                    callee: Callee::Direct(g),
                    ..
                } => callees[i].push(ix(*g)),
                Value::Prim { op, ty, args } => raises |= needs_zero_test(*op, *ty, args),
                v => raises |= may_defect_without_flags(v),
            },
            Some(_) => {}
            None => raises |= matches!(term, Term::Defect(_) | Term::Unwind),
        });
        flags[i] = raises;
    }
    let mut changed = true;
    while changed {
        changed = false;
        for i in 0..n {
            if !flags[i] && callees[i].iter().any(|&g| flags[g]) {
                flags[i] = true;
                changed = true;
            }
        }
    }
    flags
}

/// [`may_defect`] for values other than direct calls.
fn may_defect_without_flags(v: &Value) -> bool {
    match v {
        Value::Call {
            callee: Callee::Direct(_),
            ..
        } => false,
        Value::Call { .. } => true,
        Value::Builtin { op, .. } => builtin_may_defect(*op),
        Value::Fiber { op, .. } => fiber_may_defect(*op),
        _ => false,
    }
}

/// What encloses the code being rebuilt, innermost last.
#[derive(Clone, Copy)]
enum Encl {
    /// The body of a `protect` with this cleanup (of the source function).
    Protect(SeqId),
    /// A cleanup running on the way out with a defect, which was taken into this variable.
    Stash(VarId),
}

/// Rebuilds one function.
struct Explicit<'a> {
    m: &'a Module,
    src: &'a Function,
    c: Copier<'a>,
    out: Function,
    stack: Vec<Encl>,
    /// The depth of `stack` at each block of the source function.
    depth: FxHashMap<LabelId, usize>,
}

impl<'a> Explicit<'a> {
    fn run(m: &'a Module, src: &'a Function) -> Function {
        let mut x = Explicit {
            m,
            src,
            c: Copier::new(src),
            out: empty_like(src),
            stack: Vec::new(),
            depth: FxHashMap::default(),
        };
        let params = src
            .params
            .iter()
            .map(|&p| x.c.fresh(&mut x.out, p))
            .collect();
        x.out.params = params;
        let body = src.body.expect("a function with a body");
        let body = x.seq(body, false);
        x.out.body = Some(body);
        x.out
    }

    fn let_(&mut self, dst: Option<VarId>, value: Value) -> StmtId {
        self.out.stmts.alloc(Stmt::Let {
            dst,
            value,
            on_defect: None,
        })
    }

    fn runtime(&mut self, stmts: &mut Vec<StmtId>, op: RuntimeOp, args: Vec<Operand>) {
        let s = self.let_(None, Value::Runtime { op, args });
        stmts.push(s);
    }

    /// A sequence that runs the code `exit` puts before `term`.
    fn exit_seq(&mut self, from: usize, unwind: bool, term: Term) -> SeqId {
        let mut stmts = Vec::new();
        self.exit(from, unwind, &mut stmts);
        self.out.seqs.alloc(Seq { stmts, term })
    }

    /// Rebuilds the sequence. `protect_top`: it is the body of a `protect`, so its `yield` leaves
    /// the innermost protect.
    fn seq(&mut self, id: SeqId, protect_top: bool) -> SeqId {
        let src = self.src;
        let seq = &src.seqs[id];
        let mut stmts = Vec::new();
        for &s in &seq.stmts {
            self.stmt(s, &mut stmts);
        }
        let term = self.term(&seq.term, protect_top, &mut stmts);
        self.out.seqs.alloc(Seq { stmts, term })
    }

    /// Appends the cleanups run when leaving the enclosing statements from index `from` of the
    /// stack on, innermost first.
    fn exit(&mut self, from: usize, unwind: bool, stmts: &mut Vec<StmtId>) {
        let mut i = self.stack.len();
        while i > from {
            i -= 1;
            match self.stack[i] {
                Encl::Protect(cleanup) => {
                    let saved = self.stack.split_off(i);
                    if unwind {
                        let d = self.out.vars.alloc(VarData {
                            hint: "defect".into(),
                            repr: Repr::Ref,
                            ty: TypeId::ANY,
                        });
                        let take = self.let_(
                            Some(d),
                            Value::Runtime {
                                op: RuntimeOp::DefectTake,
                                args: Vec::new(),
                            },
                        );
                        stmts.push(take);
                        self.stack.push(Encl::Stash(d));
                        let block = self.cleanup(cleanup);
                        stmts.push(block);
                        self.stack.pop();
                        self.runtime(stmts, RuntimeOp::DefectResume, vec![d.into()]);
                    } else {
                        let block = self.cleanup(cleanup);
                        stmts.push(block);
                    }
                    self.stack.extend(saved);
                }
                Encl::Stash(d) => {
                    debug_assert!(unwind, "only unwinding leaves a cleanup");
                    self.runtime(stmts, RuntimeOp::DefectResume, vec![d.into()]);
                }
            }
        }
    }

    /// A copy of the cleanup, as a block without value.
    fn cleanup(&mut self, cleanup: SeqId) -> StmtId {
        let label = self.out.labels.alloc(crate::ir::LabelData {
            hint: "cleanup".into(),
        });
        let body = self.seq(cleanup, false);
        self.out.stmts.alloc(Stmt::Block {
            dst: None,
            label,
            body,
        })
    }

    fn term(&mut self, t: &Term, protect_top: bool, stmts: &mut Vec<StmtId>) -> Term {
        let out = &mut self.out;
        match t {
            Term::Yield(v) => {
                let v = v.as_ref().map(|v| self.c.operand(out, v));
                if protect_top {
                    self.exit(self.stack.len() - 1, false, stmts);
                }
                Term::Yield(v)
            }
            Term::Return(v) => {
                let v = v.as_ref().map(|v| self.c.operand(out, v));
                self.exit(0, false, stmts);
                Term::Return(v)
            }
            Term::Fail(v) => {
                let v = self.c.operand(out, v);
                self.exit(0, false, stmts);
                Term::Fail(v)
            }
            Term::Break { label, value } => {
                let value = value.as_ref().map(|v| self.c.operand(out, v));
                let to = self.depth[label];
                let label = self.c.label(out, *label);
                self.exit(to, false, stmts);
                Term::Break { label, value }
            }
            Term::Recur(args) => Term::Recur(self.c.operands(out, args)),
            Term::Defect(msg) => {
                let msg = self.c.operand(out, msg);
                self.runtime(stmts, RuntimeOp::Defect, vec![msg]);
                self.exit(0, true, stmts);
                Term::Unwind
            }
            Term::Unwind => {
                self.exit(0, true, stmts);
                Term::Unwind
            }
            Term::Unreachable => Term::Unreachable,
        }
    }

    fn fresh_dst(&mut self, dst: Option<VarId>) -> Option<VarId> {
        dst.map(|d| self.c.fresh(&mut self.out, d))
    }

    fn stmt(&mut self, id: StmtId, stmts: &mut Vec<StmtId>) {
        let src = self.src;
        let stmt = match &src.stmts[id] {
            Stmt::Let { dst, value, .. } => {
                if let Value::Prim { op, ty, args } = value
                    && needs_zero_test(*op, *ty, args)
                {
                    self.zero_test(*ty, &args[1], stmts);
                }
                let mut plain = value.clone();
                let on_fail = match &mut plain {
                    Value::Call { on_fail, .. } | Value::Fiber { on_fail, .. } => on_fail.take(),
                    _ => None,
                };
                let mut value = self.c.value(&mut self.out, &plain);
                if let Some(edge) = on_fail {
                    let handle = self.landing(edge);
                    if let Value::Call { on_fail, .. } | Value::Fiber { on_fail, .. } = &mut value {
                        *on_fail = Some(handle);
                    }
                }
                let on_defect =
                    may_defect(self.m, &value).then(|| self.exit_seq(0, true, Term::Unwind));
                Stmt::Let {
                    dst: self.fresh_dst(*dst),
                    value,
                    on_defect,
                }
            }
            Stmt::If {
                dst,
                cond,
                then,
                else_,
            } => {
                let cond = self.c.operand(&mut self.out, cond);
                let then = self.seq(*then, false);
                let else_ = self.seq(*else_, false);
                Stmt::If {
                    dst: self.fresh_dst(*dst),
                    cond,
                    then,
                    else_,
                }
            }
            Stmt::Switch {
                dst,
                on,
                cases,
                default,
            } => {
                let on = match on {
                    crate::ir::SwitchOn::Tag(v) => {
                        crate::ir::SwitchOn::Tag(self.c.var(&mut self.out, *v))
                    }
                    crate::ir::SwitchOn::Lit(v) => {
                        crate::ir::SwitchOn::Lit(self.c.var(&mut self.out, *v))
                    }
                };
                let cases = cases
                    .iter()
                    .map(|c| crate::ir::Case {
                        key: c.key.clone(),
                        body: self.seq(c.body, false),
                    })
                    .collect();
                let default = default.map(|d| self.seq(d, false));
                Stmt::Switch {
                    dst: self.fresh_dst(*dst),
                    on,
                    cases,
                    default,
                }
            }
            Stmt::Loop {
                dst,
                vars,
                inits,
                body,
            } => {
                let inits = self.c.operands(&mut self.out, inits);
                let vars = vars
                    .iter()
                    .map(|&v| self.c.fresh(&mut self.out, v))
                    .collect();
                let body = self.seq(*body, false);
                Stmt::Loop {
                    dst: self.fresh_dst(*dst),
                    vars,
                    inits,
                    body,
                }
            }
            Stmt::Block { dst, label, body } => {
                self.depth.insert(*label, self.stack.len());
                let new = self.out.labels.alloc(src.labels[*label].clone());
                self.c.labels.insert(*label, new);
                let body = self.seq(*body, false);
                Stmt::Block {
                    dst: self.fresh_dst(*dst),
                    label: new,
                    body,
                }
            }
            Stmt::Dup(v) => Stmt::Dup(self.c.var(&mut self.out, *v)),
            Stmt::Drop(v) => Stmt::Drop(self.c.var(&mut self.out, *v)),
            Stmt::Protect { dst, body, cleanup } => {
                self.stack.push(Encl::Protect(*cleanup));
                let body = self.seq(*body, true);
                self.stack.pop();
                let label = self.out.labels.alloc(crate::ir::LabelData {
                    hint: "protect".into(),
                });
                Stmt::Block {
                    dst: self.fresh_dst(*dst),
                    label,
                    body,
                }
            }
        };
        let s = self.out.stmts.alloc(stmt);
        stmts.push(s);
    }

    /// The landing of a failure edge.
    fn landing(&mut self, edge: OnFail) -> OnFail {
        let error = self.out.vars.alloc(VarData {
            hint: "e".into(),
            repr: Repr::Ref,
            ty: TypeId::ANY,
        });
        let body = match edge {
            OnFail::Break(l) => {
                let to = self.depth[&l];
                let label = self.c.label(&mut self.out, l);
                self.exit_seq(
                    to,
                    false,
                    Term::Break {
                        label,
                        value: Some(error.into()),
                    },
                )
            }
            OnFail::Propagate => self.exit_seq(0, false, Term::Fail(error.into())),
            OnFail::Handle { .. } => unreachable!("failure landings come from this pass"),
        };
        OnFail::Handle { error, body }
    }

    /// `if divisor == 0 { runtime defect("division by zero"); unwind }`.
    fn zero_test(&mut self, ty: PrimTy, divisor: &Operand, stmts: &mut Vec<StmtId>) {
        let divisor = self.c.operand(&mut self.out, divisor);
        let zero = match ty {
            PrimTy::I64 => Const::Int64(0),
            _ => Const::Int(0),
        };
        let z = self.out.vars.alloc(VarData {
            hint: "zero".into(),
            repr: Repr::I32,
            ty: TypeId::BOOL,
        });
        let test = self.let_(
            Some(z),
            Value::Prim {
                op: PrimOp::Eq,
                ty,
                args: vec![divisor, zero.into()],
            },
        );
        stmts.push(test);
        let mut raise = Vec::new();
        self.runtime(
            &mut raise,
            RuntimeOp::Defect,
            vec![Const::String("division by zero".into()).into()],
        );
        self.exit(0, true, &mut raise);
        let then = self.out.seqs.alloc(Seq {
            stmts: raise,
            term: Term::Unwind,
        });
        let else_ = self.out.seqs.alloc(Seq {
            stmts: Vec::new(),
            term: Term::Yield(None),
        });
        let s = self.out.stmts.alloc(Stmt::If {
            dst: None,
            cond: z.into(),
            then,
            else_,
        });
        stmts.push(s);
    }
}
