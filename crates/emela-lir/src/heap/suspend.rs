//! The suspend state machines: suspending functions become resumable.
//!
//! A suspending function is compiled once, as an ordinary function whose suspension points
//! (calls that may suspend, concurrency operations that may wait, fairness yields) each get an
//! `on_suspend` edge ([`Suspend`]) with a resume point (`state`) and the variables to save. The
//! backend emits each edge as a test after the value: when it came back suspended, the function
//! allocates a frame of its own holding the state and the saved variables, pushes it onto the
//! continuation being captured and returns to its caller, which does the same. Nothing is
//! allocated on the fast path: a frame exists only once a suspension actually happens, and the
//! variables stay in locals until then (Koka's lazy capture, `emela-0.20-backend-survey.md`
//! conclusion 3).
//!
//! The captured continuation is a list of frames, innermost first. The runtime resumes it from a
//! driver loop: it pops a frame and enters its function with it, which restores the saved
//! variables, continues right after the suspended value with the input (a value, or a pending
//! failure or defect that the landings see), and returns its result to the loop, which passes it
//! to the next frame. Resuming never nests on the C stack (conclusion 4). A continuation that is
//! abandoned while suspended is unwound by resuming it with the interruption as the pending
//! defect: the defect landings run the cleanups (the pending `with` releases, spec 14.5) the same
//! way as for any defect.
//!
//! What the pass changes, in each suspending function:
//!
//! - Before each suspending call it puts a fairness yield ([`RuntimeOp::Yield`], or
//!   [`RuntimeOp::YieldPerform`] before a `suspend fn` operation, which also delivers a pending
//!   interruption; spec 11.1, 11.4). A yield is a suspension point with a copy of the call's
//!   defect landing.
//! - Each value that may suspend gets its `on_suspend` edge. The saved variables are those live
//!   after the value on any way on (its landings included) and the references the value borrows:
//!   after reference counting, the resumed function then owns exactly what the uninterrupted one
//!   owns right after the value, so the code after it is the same for both.
//!
//! Non-suspending functions are left as they are. A suspending call in one of them is a call
//! that must complete in place (spec 8.7); the backend unwinds what comes back suspended as
//! interrupted (spec 14.5).

use crate::heap::copy::{clone_seq, compact};
use crate::heap::live::live_on_lets;
use crate::heap::rc::{Mode, value_uses};
use crate::ir::{
    Callee, FnId, Function, Module, OnFail, RuntimeOp, SeqId, Stage, Stmt, StmtId, Suspend, Value,
    VarId,
};

/// Makes every suspending function a state machine. The module must be at
/// [`Stage::DefectsExplicit`]; it is at [`Stage::StateMachines`] afterwards.
pub fn state_machines(m: &mut Module) {
    assert_eq!(
        m.stage,
        Stage::DefectsExplicit,
        "the state machines are made after the defect paths are explicit"
    );
    let ids: Vec<FnId> = m.functions.iter().map(|(id, _)| id).collect();
    for id in ids {
        let f = &m.functions[id];
        if !f.suspends || f.body.is_none() {
            continue;
        }
        let mut f = f.clone();
        let body = f.body.expect("a function with a body");
        insert_yields(m, &mut f, body);
        mark(m, &mut f);
        m.functions[id] = compact(&f);
    }
    m.stage = Stage::StateMachines;
}

/// Whether the value may come back suspended: a suspending call, a concurrency operation that
/// may wait, or a fairness yield.
pub(crate) fn may_suspend(v: &Value) -> bool {
    match v {
        Value::Call { suspends, .. } => *suspends,
        Value::Fiber { op, .. } => op.suspends(),
        Value::Runtime { op, .. } => matches!(op, RuntimeOp::Yield | RuntimeOp::YieldPerform),
        _ => false,
    }
}

/// The sequences directly inside the statement, in order.
fn children(stmt: &Stmt) -> Vec<SeqId> {
    match stmt {
        Stmt::Let {
            value, on_defect, ..
        } => {
            let mut out = Vec::new();
            if let Value::Call {
                on_fail: Some(OnFail::Handle { body, .. }),
                ..
            }
            | Value::Fiber {
                on_fail: Some(OnFail::Handle { body, .. }),
                ..
            } = value
            {
                out.push(*body);
            }
            out.extend(*on_defect);
            out
        }
        Stmt::If { then, else_, .. } => vec![*then, *else_],
        Stmt::Switch { cases, default, .. } => cases
            .iter()
            .map(|c| c.body)
            .chain(default.iter().copied())
            .collect(),
        Stmt::Loop { body, .. } | Stmt::Block { body, .. } => vec![*body],
        Stmt::Protect { body, cleanup, .. } => vec![*body, *cleanup],
        Stmt::Dup(_) | Stmt::Drop(_) => Vec::new(),
    }
}

/// Puts a fairness yield before every suspending call in the sequence (nested ones included).
fn insert_yields(m: &Module, f: &mut Function, seq: SeqId) {
    let stmts = f.seqs[seq].stmts.clone();
    let mut out = Vec::with_capacity(stmts.len());
    for s in stmts {
        for c in children(&f.stmts[s]) {
            insert_yields(m, f, c);
        }
        if let Stmt::Let {
            value:
                Value::Call {
                    callee,
                    suspends: true,
                    ..
                },
            on_defect,
            ..
        } = &f.stmts[s]
        {
            let perform = matches!(
                callee,
                Callee::Op { effect, op, .. } if m.core.effects[*effect].ops[*op].suspend
            );
            let landing = *on_defect;
            let on_defect = landing.map(|l| clone_seq(f, l));
            let op = if perform {
                RuntimeOp::YieldPerform
            } else {
                RuntimeOp::Yield
            };
            let y = f.stmts.alloc(Stmt::Let {
                dst: None,
                value: Value::Runtime {
                    op,
                    args: Vec::new(),
                },
                on_defect,
                on_suspend: None,
            });
            out.push(y);
        }
        out.push(s);
    }
    f.seqs[seq].stmts = out;
}

/// The statements of the sequence and the ones nested in it, in order.
fn preorder(f: &Function, seq: SeqId, out: &mut Vec<StmtId>) {
    for &s in &f.seqs[seq].stmts {
        out.push(s);
        for c in children(&f.stmts[s]) {
            preorder(f, c, out);
        }
    }
}

/// The variables a suspension point saves (see [`Suspend::saved`]), in variable order.
pub(crate) fn saved_vars(
    m: &Module,
    f: &Function,
    value: &Value,
    live_on: &crate::heap::live::Set,
) -> Vec<VarId> {
    let mut saved = live_on.clone();
    value_uses(m, f, value, &mut |v, mode| {
        if mode == Mode::Borrow {
            saved.insert(v);
        }
    });
    let mut saved: Vec<VarId> = saved.into_iter().collect();
    saved.sort_by_key(|v| v.into_raw());
    saved
}

/// Gives every suspension point its edge, numbering the states in statement order.
fn mark(m: &Module, f: &mut Function) {
    let live = live_on_lets(f);
    let mut order = Vec::new();
    preorder(f, f.body.expect("a function with a body"), &mut order);
    let mut state = 0;
    for s in order {
        let Stmt::Let { value, .. } = &f.stmts[s] else {
            continue;
        };
        if !may_suspend(value) {
            continue;
        }
        let saved = saved_vars(m, f, value, &live[&s]);
        if let Stmt::Let { on_suspend, .. } = &mut f.stmts[s] {
            *on_suspend = Some(Suspend { state, saved });
        }
        state += 1;
    }
}
