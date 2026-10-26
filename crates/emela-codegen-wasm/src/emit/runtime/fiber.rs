//! Fibers, scopes, Deferred values and the run's scheduler (spec chapter 11, 14.5).
//!
//! This is the WASM counterpart of the C runtime's fiber machinery
//! (`crates/emela-codegen-c/src/emela_runtime.c`), so that all targets switch fibers in the same
//! order:
//!
//! - A run (main or one test) is a scheduler of fibers on one thread. The root of a run is a
//!   fiber of its own, so that an empty run can interrupt it like any other. A fiber is a counted
//!   object holding its scope, how it ended, its value or cause, the wait it is parked on, the
//!   fibers waiting for it, and its pending interruptions.
//! - A suspension starts at a request (a `wait` object): a fairness yield, a `suspend fn`
//!   operation that is not resumed in place, or a fiber operation that waits. The request makes
//!   the frames bubble up; the driver attaches the captured continuation to it. Waking a wait
//!   continues it as a queued job, in the order the wakeups happened.
//! - An interruption is delivered as the pending defect (an `interrupted` object), so the defect
//!   landings of the Heap IR unwind it and run the releases. A continuation abandoned while
//!   suspended is resumed with one.
//! - A scope closes when its body returns a value or a child fails: the remaining children are
//!   interrupted and the scope waits for them (`closer`), then the first failure (or the body's
//!   result) ends the scope.
//!
//! Every runtime record is an ordinary counted object, so `alloc` / `drop` free them. Only the
//! links between records (target lists, the run's lists, the queue) are raw, so no reference
//! cycle keeps a record alive. The scheduler runs on one thread and resumes one continuation at a
//! time; the order of the queue is the order of the wakeups, so a run is repeatable.

use super::*;

use super::attempt::{CAUSE_DIE, CAUSE_FAIL};
use super::sched::{F_NEXT, P_CALLER, P_IMPL, P_USED, P_WHERE};

/// The index of `Interrupt` in `CauseKind`.
const CAUSE_INTERRUPT: i32 = 2;
/// The `perform` record's result representation (see `runtime::sched`).
const P_REPR: u32 = HDR + 80;

// ---- the fiber record ----

const F_SCOPE: u32 = HDR; // Ref: the scope it was forked into (0 for the root)
const F_FLAGS: u32 = HDR + 8;
const F_HOLDS: u32 = HDR + 16;
const F_SCOPES: u32 = HDR + 24;
const F_BUDGET: u32 = HDR + 32;
const F_CAUSE: u32 = HDR + 40; // Ref: a Cause, or 0 for a value
const F_RESMODE: u32 = HDR + 48;
const F_RESREF: u32 = HDR + 56;
const F_RESWORD: u32 = HDR + 64; // i64
const F_RESVAL: u32 = HDR + 72; // Ref: the value when it is a reference
const F_WAITER: u32 = HDR + 80; // raw: the wait it is parked on
const F_WHEAD: u32 = HDR + 88;
const F_WTAIL: u32 = HDR + 96;
const F_PHEAD: u32 = HDR + 104; // pending interruptions (list nodes)
const F_PTAIL: u32 = HDR + 112;
const F_UHEAD: u32 = HDR + 120; // scopes whose body unwinds by an interruption
const F_UTAIL: u32 = HDR + 128;
const F_NEXTF: u32 = HDR + 136; // raw: the run's list of fibers
const F_PREVF: u32 = HDR + 144;
const F_ITEM: u32 = HDR + 152; // raw: the scope's list of children
const F_ITEMPREV: u32 = HDR + 160;
const F_REPR: u32 = HDR + 168;
const F_SLOTS: i32 = 22;

/// `F_FLAGS`: the fiber ended, unwinds as interrupted, or is in the run's list.
const FF_ENDED: i32 = 1;
const FF_UNWINDING: i32 = 2;
const FF_LISTED: i32 = 4;
/// The representation index of `Repr::Ref` (see `emit::suspend::repr_index`).
const R_REF: i32 = 4;
/// The budget of suspending calls before a fairness yield (spec 11.1; the C runtime's
/// `EM_BUDGET`).
const BUDGET: i32 = 1000;

// ---- the scope record ----

const S_FIBER: u32 = HDR; // raw: the fiber its body runs on
const S_DEPTH: u32 = HDR + 8;
const S_FLAGS: u32 = HDR + 16;
const S_FIRST: u32 = HDR + 24; // Ref: the first failure (a Cause)
const S_CHEAD: u32 = HDR + 32; // raw: children that have not ended
const S_CTAIL: u32 = HDR + 40;
const S_CLOSER: u32 = HDR + 48; // raw: the wait for the last child
const S_SLOTS: i32 = 7;

const SF_CLOSED: i32 = 1;
const SF_BODY_DONE: i32 = 2;
const SF_ENDED: i32 = 4;

// ---- the Deferred record ----

const D_SCOPE: u32 = HDR; // Ref
const D_FULL: u32 = HDR + 8;
const D_ISREF: u32 = HDR + 16;
const D_WORD: u32 = HDR + 24; // i64
const D_VAL: u32 = HDR + 32; // Ref
const D_WHEAD: u32 = HDR + 40;
const D_WTAIL: u32 = HDR + 48;
const D_SLOTS: i32 = 7;

// ---- the wait record ----

pub(super) const W_CHAIN: u32 = HDR; // Ref: the continuation, once suspended
pub(super) const W_FIBER: u32 = HDR + 8; // Ref
pub(super) const W_KIND: u32 = HDR + 16;
pub(super) const W_VERB: u32 = HDR + 24;
pub(super) const W_REPR: u32 = HDR + 32;
pub(super) const W_TARGET: u32 = HDR + 40; // raw: the fiber, deferred or scope
pub(super) const W_PREV: u32 = HDR + 48; // raw: the run's list
pub(super) const W_NEXT: u32 = HDR + 56;
pub(super) const W_LIVE: u32 = HDR + 64;
pub(super) const W_ONDROP: u32 = HDR + 72; // Ref: the operation call whose implementation it waits for
pub(super) const W_END: u32 = HDR + 80;
pub(super) const W_FLAGS: u32 = HDR + 88;
pub(super) const W_AT: u32 = HDR + 96; // i64: the TestClock time a sleeper waits for
pub(super) const W_SEQ: u32 = HDR + 104;
pub(super) const W_HPREV: u32 = HDR + 112;
pub(super) const W_HNEXT: u32 = HDR + 120;
/// raw: the target's (a fiber's or a Deferred's) list of waiters. The run's list uses
/// `W_PREV`/`W_NEXT`, so a wait in both does not overwrite either list.
pub(super) const W_TPREV: u32 = HDR + 128;
pub(super) const W_TNEXT: u32 = HDR + 136;
/// Ref: the operation call whose strand this continuation is (`W_END == WE_IMPL`).
pub(super) const W_IMPL: u32 = HDR + 144;
const W_SLOTS: i32 = 19;

/// What a wait waits for.
pub(super) const WK_FIBER: i32 = 0; // join, await and interrupt of a child
pub(super) const WK_DEFERRED: i32 = 1;
pub(super) const WK_PAUSE: i32 = 2; // a fairness yield
pub(super) const WK_CLOSER: i32 = 3; // a scope waiting for its children
pub(super) const WK_CALLER: i32 = 4; // a caller waiting for an operation implementation
pub(super) const WK_HOST: i32 = 5; // a host timer
pub(super) const WK_TEST: i32 = 6; // a TestClock sleeper
/// The verb of a wait registered on a fiber.
const WV_JOIN: i32 = 0;
const WV_AWAIT: i32 = 1;
const WV_INTERRUPT: i32 = 2;

/// `W_FLAGS`: the wait is in its target's list, in the run's list, or is a scope closing (not
/// cut short when the run is unwound, spec 14.5).
const WF_TARGET: i32 = 1;
const WF_ROOT: i32 = 2;
const WF_CLOSING: i32 = 4;
const WF_HOST: i32 = 8;
const WF_TEST: i32 = 16;

/// `W_END`: whose continuation it is.
pub(super) const WE_MAIN: i32 = 0;
pub(super) const WE_IMPL: i32 = 1;
pub(super) const WE_ORPHAN: i32 = 2;
/// The caller chain of a suspending call inside a non-suspending function (spec 8.7).
pub(super) const WE_SYNC: i32 = 3;

// ---- interruption nodes ----
//
// A pending interruption is the scope whose body it unwinds, or a node with a 0 target for the
// whole fiber. The lists keep the order the interruptions arrived; only the outermost scope is
// delivered next (`deliver`).

const N_NEXT: u32 = HDR;
const N_TARGET: u32 = HDR + 8; // Ref
const N_SLOTS: i32 = 2;

// ---- the queue's job records ----

const J_KIND: u32 = HDR;
const J_END: u32 = HDR + 8;
const J_FIBER: u32 = HDR + 16; // Ref
const J_CHAIN: u32 = HDR + 24; // Ref: the continuation, or the closure of a begin
const J_PERF: u32 = HDR + 32; // Ref: the operation call of an implementation continuation
const J_MODE: u32 = HDR + 40;
const J_WORD: u32 = HDR + 48; // i64
const J_NEXT: u32 = HDR + 56;
/// The representation of a value-mode word, so an interruption that replaces it can release a
/// reference (see `drain`).
const J_REPR: u32 = HDR + 60;
const J_SLOTS: i32 = 8;

pub(super) const JK_RESUME: i32 = 0;
pub(super) const JK_BEGIN: i32 = 1;
pub(super) const JK_INTERRUPT: i32 = 2;
/// A fairness yield: `J_CHAIN` is the pause wait to resume from (the C runtime's `EM_JOB_GO`).
pub(super) const JK_GO: i32 = 3;

/// The statics and descriptors the fiber runtime uses.
pub(crate) struct Plan {
    pub(crate) cur_fiber: u32,
    pub(crate) cur_main: u32,
    /// 1 while a continuation runs (`em_cur` in the C runtime); the event loop resumes a
    /// continuation by queueing it, not inside the wake.
    pub(crate) stepping: u32,
    pub(crate) susp_wait: u32,
    pub(crate) fhead: u32,
    pub(crate) ftail: u32,
    pub(crate) whead: u32,
    pub(crate) wtail: u32,
    pub(crate) qhead: u32,
    pub(crate) qtail: u32,
    /// Jobs that wait for a turn of the host event loop, after the queue empties (the C runtime's
    /// `later`; every 16th fairness yield goes here).
    pub(crate) lhead: u32,
    pub(crate) ltail: u32,
    pub(crate) pauses: u32,
    /// The run's result: 0 for a value, 1 for a failure, 2 for a defect. A strand that finishes
    /// after the run ended must not take it out of `G_DEFECT`/`G_FAILURE`.
    pub(crate) run_mode: u32,
    pub(crate) run_word: u32,
    pub(crate) run_over: u32,
    pub(crate) live_waits: u32,
    pub(crate) orphan: u32,
    pub(crate) fiber_desc: u32,
    pub(crate) scope_desc: u32,
    pub(crate) deferred_desc: u32,
    pub(crate) waiter_desc: u32,
    pub(crate) node_desc: u32,
    pub(crate) job_desc: u32,
    pub(crate) interrupted_desc: u32,
    pub(crate) scope_body_table: u32,
    pub(crate) scope_close_table: u32,
    pub(crate) call_table: u32,
    pub(crate) call1_table: u32,
    pub(crate) notes: u32,
    pub(crate) host_head: u32,
    pub(crate) host_tail: u32,
    pub(crate) test_head: u32,
    pub(crate) test_tail: u32,
    pub(crate) test_seq: u32,
    pub(crate) wait_set: u32,
    pub(crate) cause_desc: u32,
    pub(crate) failure_desc: u32,
    pub(crate) success_val: u32,
    pub(crate) success_ref: u32,
    pub(crate) note_desc: u32,
    pub(crate) scope_body_desc: u32,
    pub(crate) scope_close_desc: u32,
    pub(crate) msg_join: u32,
    pub(crate) msg_scope_used: u32,
    pub(crate) msg_scope_fork: u32,
    pub(crate) msg_deferred_complete: u32,
    pub(crate) msg_deferred_await: u32,
    pub(crate) msg_deferred_poll: u32,
    pub(crate) def_join: u32,
    pub(crate) def_await: u32,
    pub(crate) def_interrupt: u32,
    pub(crate) failed_after: u32,
    pub(crate) note_other: u32,
}

pub(super) fn plan<'a>(b: &'a Builder<'_>) -> &'a Plan {
    b.fiber_plan.as_ref().expect("the fiber runtime")
}

fn cell(b: &Builder<'_>, f: impl Fn(&Plan) -> u32) -> u32 {
    f(plan(b))
}

/// Whether the program forks, scopes or shares a Deferred: the operations that need the
/// scheduler. `Fiber.attempt` and the operations of `Std.Cause` run on the current fiber and need
/// none of this.
pub(crate) fn used(b: &Builder<'_>) -> bool {
    b.m.functions.iter().any(|(_, f)| {
        f.stmts.iter().any(|(_, s)| {
            matches!(
                s,
                Stmt::Let {
                    value: Value::Fiber { op, .. },
                    ..
                } if !matches!(
                    op,
                    FiberOp::Attempt { .. }
                        | FiberOp::CauseKind { .. }
                        | FiberOp::CauseToString
                        | FiberOp::CauseRaise { .. }
                )
            )
        })
    })
}

pub(super) const FUNCS: &[Sig] = &[
    ("fb_root_begin", &[], &[]),
    ("fb_root_run", &[], &[]),
    ("fb_finish_run", &[], &[]),
    ("fb_drain", &[], &[]),
    (
        "fb_begin",
        &[ValType::I32, ValType::I32, ValType::I32, ValType::I32],
        &[],
    ),
    (
        "fb_advance",
        &[
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I64,
            ValType::I32,
        ],
        &[],
    ),
    (
        "fb_suspended",
        &[ValType::I32, ValType::I32, ValType::I32, ValType::I32],
        &[],
    ),
    (
        "fb_strand_end",
        &[
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I64,
            ValType::I32,
        ],
        &[],
    ),
    (
        "fb_finish",
        &[ValType::I32, ValType::I32, ValType::I64],
        &[],
    ),
    (
        "fb_settled_impl",
        &[ValType::I32, ValType::I32, ValType::I64],
        &[],
    ),
    ("fb_run_result", &[ValType::I32, ValType::I32], &[]),
    ("fb_cause_string", &[ValType::I32], &[ValType::I32]),
    (
        "fb_watch_settle",
        &[ValType::I32, ValType::I32],
        &[ValType::I32, ValType::I64],
    ),
    (
        "fb_cause_new",
        &[ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    (
        "fb_scope",
        &[ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I64],
    ),
    (
        "fb_fork",
        &[ValType::I32, ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    ("fb_join", &[ValType::I32, ValType::I32], &[ValType::I64]),
    ("fb_await", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("fb_interrupt", &[ValType::I32], &[]),
    ("fb_deferred", &[ValType::I32], &[ValType::I32]),
    (
        "fb_complete",
        &[ValType::I32, ValType::I64, ValType::I32],
        &[ValType::I32],
    ),
    (
        "fb_deferred_await",
        &[ValType::I32, ValType::I32],
        &[ValType::I64],
    ),
    (
        "fb_poll",
        &[ValType::I32, ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    ("fb_yield", &[ValType::I32], &[]),
    ("fb_hold", &[], &[]),
    ("fb_unhold", &[], &[]),
    ("fb_interrupting", &[ValType::I32], &[ValType::I32]),
    ("fb_is_unwinding", &[ValType::I32], &[ValType::I32]),
    ("fb_deliver", &[ValType::I32], &[ValType::I32]),
    ("fb_interrupt_here", &[], &[ValType::I32]),
    ("fb_interrupt_fiber", &[ValType::I32, ValType::I32], &[]),
    ("fb_is_interrupted", &[ValType::I32], &[ValType::I32]),
    (
        "fb_wait_new",
        &[ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    (
        "fb_wait_wake",
        &[ValType::I32, ValType::I32, ValType::I64],
        &[],
    ),
    ("fb_wait_chain", &[ValType::I32], &[ValType::I32]),
    ("fb_watch_add", &[ValType::I32, ValType::I32], &[]),
    ("fb_watch_remove", &[ValType::I32], &[]),
    (
        "fb_queue_push",
        &[
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I64,
            ValType::I32,
            ValType::I32,
        ],
        &[],
    ),
    (
        "fb_later_push",
        &[
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I64,
            ValType::I32,
            ValType::I32,
        ],
        &[],
    ),
    ("fb_later_move", &[], &[]),
    ("fb_scope_body_resume", &[ValType::I32], &[]),
    ("fb_scope_close_resume", &[ValType::I32], &[]),
    ("fb_scope_failed", &[ValType::I32, ValType::I32], &[]),
    ("fb_scope_child_ended", &[ValType::I32, ValType::I32], &[]),
    (
        "fb_scope_finish",
        &[ValType::I32, ValType::I32, ValType::I64, ValType::I32],
        &[ValType::I64],
    ),
    (
        "fb_scope_body_done",
        &[ValType::I32, ValType::I32, ValType::I64, ValType::I32],
        &[ValType::I64],
    ),
    (
        "fb_pending_has",
        &[ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    (
        "fb_unwinds_has",
        &[ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    ("fb_interrupted_new", &[ValType::I32], &[ValType::I32]),
    ("fb_pending_push", &[ValType::I32, ValType::I32], &[]),
    ("fb_pending_clear", &[ValType::I32], &[]),
    ("fb_pending_remove", &[ValType::I32, ValType::I32], &[]),
    ("fb_scope_of", &[ValType::I32], &[ValType::I32]),
    ("fb_fiber_of", &[ValType::I32], &[ValType::I32]),
    ("fb_note", &[ValType::I32, ValType::I32], &[]),
    ("fb_notes_of", &[ValType::I32], &[ValType::I32]),
    ("fb_notes_add", &[ValType::I32, ValType::I32], &[]),
    ("fb_notes_clear", &[], &[]),
    ("fb_call_i32", &[ValType::I32], &[ValType::I64]),
    ("fb_call_i64", &[ValType::I32], &[ValType::I64]),
    ("fb_call_f64", &[ValType::I32], &[ValType::I64]),
    ("fb_call_ref", &[ValType::I32], &[ValType::I64]),
    ("fb_call_void", &[ValType::I32], &[ValType::I64]),
    (
        "fb_call1_i32",
        &[ValType::I32, ValType::I32],
        &[ValType::I64],
    ),
    (
        "fb_call1_i64",
        &[ValType::I32, ValType::I32],
        &[ValType::I64],
    ),
    (
        "fb_call1_f64",
        &[ValType::I32, ValType::I32],
        &[ValType::I64],
    ),
    (
        "fb_call1_ref",
        &[ValType::I32, ValType::I32],
        &[ValType::I64],
    ),
    (
        "fb_call1_void",
        &[ValType::I32, ValType::I32],
        &[ValType::I64],
    ),
    ("fb_sleep_start", &[ValType::I32], &[]),
    ("fb_test_sleep", &[ValType::I64], &[]),
    ("fb_idle", &[], &[ValType::I32]),
    ("fb_has_waits", &[], &[ValType::I32]),
    ("fb_wake_test", &[], &[ValType::I32]),
    ("fb_root_interrupt", &[], &[]),
    ("fb_unlink_wait", &[ValType::I32], &[]),
    ("fb_drop_wait", &[ValType::I32], &[]),
];

pub(super) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        "fb_root_begin" | "fb_root_run" | "fb_finish_run" => i32s(4),
        "fb_drain" => vec![(8, ValType::I32), (1, ValType::I64), (2, ValType::I32)],
        "fb_begin" => vec![(7, ValType::I32), (1, ValType::I64)],
        "fb_advance" => vec![(8, ValType::I32), (1, ValType::I64)],
        "fb_suspended" => i32s(6),
        "fb_strand_end" => vec![(3, ValType::I32), (1, ValType::I64)],
        "fb_cause_string" => i32s(3),
        "fb_watch_settle" => vec![(5, ValType::I32), (1, ValType::I64)],
        "fb_finish" => vec![(7, ValType::I32), (1, ValType::I64)],
        "fb_settled_impl" => i32s(4),
        "fb_run_result" => i32s(2),
        "fb_cause_new" => i32s(2),
        "fb_scope" => vec![(6, ValType::I32), (1, ValType::I64)],
        "fb_fork" => i32s(7),
        "fb_join" => vec![(5, ValType::I32), (1, ValType::I64)],
        "fb_await" => vec![(5, ValType::I32), (1, ValType::I64)],
        "fb_interrupt" => i32s(6),
        "fb_deferred" => i32s(4),
        "fb_complete" => vec![(5, ValType::I32), (1, ValType::I64)],
        "fb_deferred_await" => i32s(6),
        "fb_poll" => i32s(7),
        "fb_yield" => i32s(5),
        "fb_hold" | "fb_unhold" => i32s(1),
        "fb_interrupt_here" => i32s(1),
        "fb_is_interrupted" => i32s(2),
        "fb_deliver" => i32s(7),
        "fb_interrupt_fiber" => i32s(7),
        "fb_wait_new" => i32s(4),
        "fb_wait_wake" => i32s(7),
        "fb_wait_chain" => i32s(5),
        "fb_watch_add" => i32s(4),
        "fb_watch_remove" => i32s(6),
        "fb_unlink_wait" => i32s(5),
        "fb_drop_wait" => i32s(5),
        "fb_queue_push" | "fb_later_push" => i32s(9),
        "fb_later_move" => i32s(3),
        "fb_scope_body_resume" => vec![(3, ValType::I32), (1, ValType::I64)],
        "fb_scope_close_resume" => vec![(6, ValType::I32), (1, ValType::I64)],
        "fb_scope_failed" => i32s(4),
        "fb_scope_child_ended" => i32s(4),
        "fb_scope_finish" => i32s(5),
        "fb_scope_body_done" => i32s(10),
        "fb_pending_has" | "fb_unwinds_has" => i32s(1),
        "fb_interrupted_new" => i32s(1),
        "fb_pending_push" => i32s(4),
        "fb_pending_clear" => i32s(4),
        "fb_pending_remove" => i32s(5),
        "fb_note" => i32s(5),
        "fb_notes_of" => i32s(4),
        "fb_notes_add" => i32s(7),
        "fb_notes_clear" => i32s(3),
        "fb_idle" => i32s(8),
        "fb_sleep_start" => i32s(4),
        "fb_test_sleep" => i32s(5),
        "fb_wake_test" => i32s(6),
        "fb_root_interrupt" => i32s(6),
        _ => Vec::new(),
    }
}

// ---- small emission helpers ----

fn c(ins: &mut InstructionSink, v: i32) {
    ins.i32_const(v);
}

fn g(ins: &mut InstructionSink, l: u32) {
    ins.local_get(l);
}

fn ldl(ins: &mut InstructionSink, l: u32, off: u32) {
    ins.local_get(l);
    ins.i32_load(mem(off, 4));
}

fn stl(ins: &mut InstructionSink, l: u32, off: u32, v: u32) {
    ins.local_get(l);
    ins.local_get(v);
    ins.i32_store(mem(off, 4));
}

fn st_const(ins: &mut InstructionSink, l: u32, off: u32, v: i32) {
    ins.local_get(l);
    ins.i32_const(v);
    ins.i32_store(mem(off, 4));
}

fn call(b: &mut Builder, ins: &mut InstructionSink, name: &'static str) {
    let f = b.rt(name);
    ins.call(f);
}

fn dropv(b: &mut Builder, ins: &mut InstructionSink, l: u32) {
    ins.local_get(l);
    call(b, ins, "drop");
}

fn raise_msg(b: &mut Builder, ins: &mut InstructionSink, msg: u32) {
    ins.i32_const(msg as i32);
    call(b, ins, "raise");
}

/// Stores a reference (dup'ed) in a slot.
fn st_dup(b: &mut Builder, ins: &mut InstructionSink, base: u32, off: u32, src: u32) {
    ins.local_get(src);
    call(b, ins, "dup");
    ins.local_get(base);
    ins.local_get(src);
    ins.i32_store(mem(off, 4));
}

pub(crate) fn prepare(b: &mut Builder<'_>) {
    if !super::suspend::used(b) {
        return;
    }
    let cells: Vec<u32> = (0..25).map(|_| b.alloc_static(4)).collect();
    let fiber_desc = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "Fiber".to_owned(),
        show: None,
        refs: vec![
            true, false, false, false, false, true, false, false, false, true, false, false, false,
            false, false, false, false, false, false, false, false, false,
        ],
    });
    let scope_desc = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "Scope".to_owned(),
        show: None,
        refs: vec![false, false, false, true, false, false, false],
    });
    let deferred_desc = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "Deferred".to_owned(),
        show: None,
        refs: vec![true, false, false, false, true, false, false],
    });
    let waiter_desc = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "wait".to_owned(),
        show: None,
        refs: vec![
            true, true, false, false, false, false, false, false, false, true, false, false, false,
            false, false, false, false, false, true,
        ],
    });
    let node_desc = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "interruption".to_owned(),
        show: None,
        refs: vec![false, true],
    });
    let job_desc = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "job".to_owned(),
        show: None,
        refs: vec![false, false, true, true, true, false, false, false],
    });
    let cause_desc = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "Cause".to_owned(),
        show: None,
        refs: vec![true],
    });
    let failure_desc = b.desc(DescKey {
        kind: 0,
        tag: 1,
        enum_id: -1,
        name: "Failure".to_owned(),
        show: None,
        refs: vec![true],
    });
    let success_val = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "Success".to_owned(),
        show: None,
        refs: vec![false],
    });
    let success_ref = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "Success".to_owned(),
        show: None,
        refs: vec![true],
    });
    let note_desc = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "note".to_owned(),
        show: None,
        refs: vec![true, true, false],
    });
    let scope_body_desc = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "Fiber.scope".to_owned(),
        show: None,
        refs: vec![false, false, true, true, false],
    });
    let scope_close_desc = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "Fiber.scope".to_owned(),
        show: None,
        // Slots 0..7. Slot 6 is `RESVAL`: it shares its word with the high half of the i64
        // `WORD` in slot 5, and only one of the two is live (the body's representation chooses).
        // Slot 7 is the i32 `MODE`, not a reference.
        refs: vec![false, false, true, true, false, false, true, false],
    });
    let mut call_index = 0;
    for name in [
        "fb_call_void",
        "fb_call_i32",
        "fb_call_i64",
        "fb_call_f64",
        "fb_call_ref",
    ] {
        let index = b.rt(name);
        call_index += 1;
        let _ = index;
        b.table_funcs.push(index);
    }
    let call_table = b.table_funcs.len() as u32 - call_index;
    for name in [
        "fb_call1_void",
        "fb_call1_i32",
        "fb_call1_i64",
        "fb_call1_f64",
        "fb_call1_ref",
    ] {
        let index = b.rt(name);
        b.table_funcs.push(index);
    }
    let call1_table = b.table_funcs.len() as u32 - 5;
    for name in ["fb_scope_body_resume", "fb_scope_close_resume"] {
        let index = b.rt(name);
        b.table_funcs.push(index);
    }
    let scope_body_table = b.table_funcs.len() as u32 - 2;
    let scope_close_table = scope_body_table + 1;
    let notes = if used(b) { b.alloc_static(8) } else { 0 };
    let msg = |b: &mut Builder<'_>, s: &[u8]| b.string_literal(s);
    let msg_join = msg(b, b"fiber was interrupted");
    let msg_scope_used = msg(b, b"fiber scope was used after it ended");
    let msg_scope_fork = msg(b, b"fiber scope was used after it ended");
    let msg_deferred_complete = msg(b, b"deferred was completed after its scope ended");
    let msg_deferred_await = msg(b, b"deferred was awaited after its scope ended");
    let msg_deferred_poll = msg(b, b"deferred was polled after its scope ended");
    let def_join = msg(b, b"fiber was joined after its scope ended");
    let def_await = msg(b, b"fiber was awaited after its scope ended");
    let def_interrupt = msg(b, b"fiber was interrupted after its scope ended");
    let failed_after = msg(b, b" failed after resume: ");
    let note_other = msg(b, b"note: another fiber also failed: ");
    b.fiber_plan = Some(Plan {
        cur_fiber: cells[0],
        cur_main: cells[1],
        stepping: cells[20],
        susp_wait: cells[2],
        fhead: cells[3],
        ftail: cells[4],
        whead: cells[5],
        wtail: cells[6],
        qhead: cells[7],
        qtail: cells[8],
        lhead: cells[21],
        ltail: cells[22],
        pauses: cells[9],
        run_mode: cells[23],
        run_word: cells[24],
        run_over: cells[10],
        live_waits: cells[11],
        orphan: cells[12],
        host_head: cells[13],
        host_tail: cells[14],
        test_head: cells[15],
        test_tail: cells[16],
        test_seq: cells[17],
        wait_set: cells[18],
        fiber_desc,
        scope_desc,
        deferred_desc,
        waiter_desc,
        node_desc,
        job_desc,
        interrupted_desc: b.sus.as_ref().map(|p| p.interrupted_desc).unwrap_or(0),
        scope_body_table,
        scope_close_table,
        call_table,
        call1_table,
        notes,
        cause_desc,
        failure_desc,
        success_val,
        success_ref,
        note_desc,
        scope_body_desc,
        scope_close_desc,
        msg_join,
        msg_scope_used,
        msg_scope_fork,
        msg_deferred_complete,
        msg_deferred_await,
        msg_deferred_poll,
        def_join,
        def_await,
        def_interrupt,
        failed_after,
        note_other,
    });
}

pub(super) fn emit(b: &mut Builder<'_>, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "fb_root_begin" => root_begin(b, ins),
        "fb_root_run" => root_run(b, ins),
        "fb_finish_run" => finish_run(b, ins),
        "fb_drain" => drain(b, ins),
        "fb_begin" => begin(b, ins),
        "fb_advance" => advance(b, ins),
        "fb_suspended" => suspended(b, ins),
        "fb_strand_end" => strand_end(b, ins),
        "fb_finish" => finish(b, ins),
        "fb_settled_impl" => settled_impl(b, ins),
        "fb_run_result" => run_result(b, ins),
        "fb_cause_string" => cause_string(b, ins),
        "fb_watch_settle" => watch_settle(b, ins),
        "fb_cause_new" => cause_new(b, ins),
        "fb_scope" => scope(b, ins),
        "fb_fork" => fork(b, ins),
        "fb_join" => join(b, ins),
        "fb_await" => await_(b, ins),
        "fb_interrupt" => interrupt(b, ins),
        "fb_deferred" => deferred(b, ins),
        "fb_complete" => complete(b, ins),
        "fb_deferred_await" => deferred_await(b, ins),
        "fb_poll" => poll(b, ins),
        "fb_yield" => yield_(b, ins),
        "fb_hold" => hold(b, ins),
        "fb_unhold" => unhold(b, ins),
        "fb_interrupting" => interrupting(b, ins),
        "fb_is_unwinding" => is_unwinding(b, ins),
        "fb_deliver" => deliver(b, ins),
        "fb_interrupt_here" => interrupt_here(b, ins),
        "fb_interrupt_fiber" => interrupt_fiber(b, ins),
        "fb_is_interrupted" => is_interrupted(b, ins),
        "fb_wait_new" => wait_new(b, ins),
        "fb_wait_wake" => wait_wake(b, ins),
        "fb_wait_chain" => wait_chain(b, ins),
        "fb_watch_add" => watch_add(b, ins),
        "fb_watch_remove" => watch_remove(b, ins),
        "fb_queue_push" => queue_push(b, ins, |p| (p.qhead, p.qtail)),
        "fb_later_push" => queue_push(b, ins, |p| (p.lhead, p.ltail)),
        "fb_later_move" => later_move(b, ins),
        "fb_scope_body_resume" => scope_body_resume(b, ins),
        "fb_scope_close_resume" => scope_close_resume(b, ins),
        "fb_scope_failed" => scope_failed(b, ins),
        "fb_scope_child_ended" => scope_child_ended(b, ins),
        "fb_scope_finish" => scope_finish(b, ins),
        "fb_scope_body_done" => scope_body_done(b, ins),
        "fb_pending_has" => pending_has(b, ins),
        "fb_unwinds_has" => unwinds_has(b, ins),
        "fb_interrupted_new" => interrupted_new(b, ins),
        "fb_pending_push" => pending_push(b, ins),
        "fb_pending_clear" => pending_clear(b, ins),
        "fb_pending_remove" => pending_remove(b, ins),
        "fb_scope_of" => scope_of(b, ins),
        "fb_fiber_of" => fiber_of(b, ins),
        "fb_note" => note(b, ins),
        "fb_notes_of" => notes_of(b, ins),
        "fb_notes_add" => notes_add(b, ins),
        "fb_notes_clear" => notes_clear(b, ins),
        "fb_call_i32" => call0(b, ins, Repr::I32),
        "fb_call_i64" => call0(b, ins, Repr::I64),
        "fb_call_f64" => call0(b, ins, Repr::F64),
        "fb_call_ref" => call0(b, ins, Repr::Ref),
        "fb_call_void" => call0(b, ins, Repr::Void),
        "fb_call1_i32" => call1(b, ins, Repr::I32),
        "fb_call1_i64" => call1(b, ins, Repr::I64),
        "fb_call1_f64" => call1(b, ins, Repr::F64),
        "fb_call1_ref" => call1(b, ins, Repr::Ref),
        "fb_call1_void" => call1(b, ins, Repr::Void),
        "fb_sleep_start" => sleep_start(b, ins),
        "fb_test_sleep" => test_sleep(b, ins),
        "fb_idle" => idle(b, ins),
        "fb_has_waits" => has_waits(b, ins),
        "fb_wake_test" => wake_test(b, ins),
        "fb_root_interrupt" => root_interrupt(b, ins),
        "fb_unlink_wait" => unlink_wait(b, ins),
        "fb_drop_wait" => drop_wait(b, ins),
        _ => return false,
    }
    true
}

// ---- records and intrusive lists ----

/// `fb_scope_of(o)`: the scope record is the Scope value itself.
fn scope_of(_b: &mut Builder<'_>, ins: &mut InstructionSink) {
    ins.local_get(0);
}

/// `fb_fiber_of(o)`: the fiber record is the Fiber handle itself.
fn fiber_of(_b: &mut Builder<'_>, ins: &mut InstructionSink) {
    ins.local_get(0);
}

/// The nodes of the pending and unwinding interruption lists, appended in arrival order.
fn push_node(
    b: &mut Builder<'_>,
    ins: &mut InstructionSink,
    f: u32,
    target: u32,
    head: u32,
    tail: u32,
    n: u32,
) {
    let node: u32 = n;
    let desc = plan(b).node_desc;
    c(ins, N_SLOTS);
    c(ins, desc as i32);
    call(b, ins, "alloc");
    ins.local_set(node);
    st_const(ins, node, N_NEXT, 0);
    g(ins, target);
    ins.if_(BlockType::Empty);
    st_dup(b, ins, node, N_TARGET, target);
    ins.else_();
    st_const(ins, node, N_TARGET, 0);
    ins.end();
    ldl(ins, f, tail);
    ins.if_(BlockType::Empty);
    ldl(ins, f, tail);
    g(ins, node);
    ins.i32_store(mem(N_NEXT, 4));
    ins.else_();
    stl(ins, f, head, node);
    ins.end();
    stl(ins, f, tail, node);
}

/// `fb_pending_has(f, target) -> i32`: whether an interruption of `target` (0: the whole fiber)
/// is pending.
fn pending_has(_b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const P: u32 = 2;
    ldl(ins, 0, F_PHEAD);
    ins.local_set(P);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, P);
    ins.i32_eqz();
    ins.br_if(1);
    ldl(ins, P, N_TARGET);
    g(ins, 1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    c(ins, 1);
    ins.return_();
    ins.end();
    ldl(ins, P, N_NEXT);
    ins.local_set(P);
    ins.br(0);
    ins.end();
    ins.end();
    c(ins, 0);
}

/// `fb_pending_push(f, target)`: add an interruption to the pending list (target is borrowed).
fn pending_push(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    push_node(b, ins, 0, 1, F_PHEAD, F_PTAIL, 4);
}

/// `fb_unwinds_has(f, target) -> i32`: whether `target` is in the unwinding list.
fn unwinds_has(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const P: u32 = 2;
    ldl(ins, 0, F_UHEAD);
    ins.local_set(P);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, P);
    ins.i32_eqz();
    ins.br_if(1);
    ldl(ins, P, N_TARGET);
    g(ins, 1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    c(ins, 1);
    ins.return_();
    ins.end();
    ldl(ins, P, N_NEXT);
    ins.local_set(P);
    ins.br(0);
    ins.end();
    ins.end();
    c(ins, 0);
    let _ = b;
}

/// `fb_pending_clear(f)`: drop every pending interruption and every scope that unwinds.
fn pending_clear(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const N: u32 = 2;
    const NEXT: u32 = 3;
    for head in [F_PHEAD, F_UHEAD] {
        ldl(ins, 0, head);
        ins.local_set(N);
        ins.block(BlockType::Empty);
        ins.loop_(BlockType::Empty);
        g(ins, N);
        ins.i32_eqz();
        ins.br_if(1);
        ldl(ins, N, N_NEXT);
        ins.local_set(NEXT);
        dropv(b, ins, N);
        g(ins, NEXT);
        ins.local_set(N);
        ins.br(0);
        ins.end();
        ins.end();
        st_const(ins, 0, head, 0);
    }
    st_const(ins, 0, F_PTAIL, 0);
    st_const(ins, 0, F_UTAIL, 0);
}

/// `fb_pending_remove(f, target)`: remove every node of `target` from the pending and the
/// unwinding lists.
fn pending_remove(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const N: u32 = 2;
    const NEXT: u32 = 3;
    const PREV: u32 = 4;
    for (head, tail) in [(F_PHEAD, F_PTAIL), (F_UHEAD, F_UTAIL)] {
        c(ins, 0);
        ins.local_set(PREV);
        ldl(ins, 0, head);
        ins.local_set(N);
        ins.block(BlockType::Empty);
        ins.loop_(BlockType::Empty);
        g(ins, N);
        ins.i32_eqz();
        ins.br_if(1);
        ldl(ins, N, N_NEXT);
        ins.local_set(NEXT);
        ldl(ins, N, N_TARGET);
        g(ins, 1);
        ins.i32_eq();
        ins.if_(BlockType::Empty);
        g(ins, PREV);
        ins.if_(BlockType::Empty);
        stl(ins, PREV, N_NEXT, NEXT);
        ins.else_();
        stl(ins, 0, head, NEXT);
        ins.end();
        g(ins, NEXT);
        ins.if_(BlockType::Empty);
        ins.else_();
        stl(ins, 0, tail, PREV);
        ins.end();
        dropv(b, ins, N);
        ins.else_();
        g(ins, N);
        ins.local_set(PREV);
        ins.end();
        g(ins, NEXT);
        ins.local_set(N);
        ins.br(0);
        ins.end();
        ins.end();
    }
}

/// `fb_interrupting(f) -> i32`: whether an interruption is ready to be delivered (spec 11.4: not
/// during a release).
fn interrupting(_b: &mut Builder<'_>, ins: &mut InstructionSink) {
    ldl(ins, 0, F_HOLDS);
    ins.if_(BlockType::Empty);
    c(ins, 0);
    ins.return_();
    ins.end();
    ldl(ins, 0, F_FLAGS);
    c(ins, FF_UNWINDING);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    c(ins, 1);
    ins.return_();
    ins.end();
    ldl(ins, 0, F_PHEAD);
    ins.if_(BlockType::Empty);
    c(ins, 1);
    ins.return_();
    ins.end();
    ldl(ins, 0, F_UHEAD);
    ins.i32_eqz();
    ins.i32_eqz();
}

/// `fb_is_unwinding(f) -> i32`: whether the fiber is already unwinding as interrupted.
fn is_unwinding(_b: &mut Builder<'_>, ins: &mut InstructionSink) {
    ldl(ins, 0, F_FLAGS);
    c(ins, FF_UNWINDING);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    c(ins, 1);
    ins.return_();
    ins.end();
    ldl(ins, 0, F_UHEAD);
    ins.i32_eqz();
    ins.i32_eqz();
}

/// `fb_interrupted_new(scope) -> Ref`: an interruption of `scope`'s body (0: the whole fiber).
fn interrupted_new(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const I: u32 = 1;
    let desc = plan(b).interrupted_desc;
    c(ins, 1);
    c(ins, desc as i32);
    call(b, ins, "alloc");
    ins.local_set(I);
    g(ins, 0);
    ins.if_(BlockType::Empty);
    st_dup(b, ins, I, HDR, 0);
    ins.else_();
    st_const(ins, I, HDR, 0);
    ins.end();
    g(ins, I);
}

/// Pushes `scope` and calls `fb_interrupted_new`.
fn new_interruption(b: &mut Builder<'_>, ins: &mut InstructionSink, scope: u32) {
    g(ins, scope);
    call(b, ins, "fb_interrupted_new");
}

/// `fb_is_interrupted(x) -> i32`: whether `x` is an interruption. A scope's interruption is a
/// fresh object, so the descriptor decides, not the pointer.
fn is_interrupted(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    let desc = plan(b).interrupted_desc;
    g(ins, 0);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    c(ins, 0);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(8, 4));
    c(ins, desc as i32);
    ins.i32_eq();
}

/// `fb_deliver(f) -> Ref`: take the outermost pending interruption and return it as the defect.
fn deliver(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const TARGET: u32 = 1;
    const N: u32 = 2;
    const BEST: u32 = 3;
    const MIN: u32 = 4;
    const NEXT: u32 = 5;
    const F: u32 = 6;
    // A whole-fiber interruption overrides everything.
    ldl(ins, 0, F_FLAGS);
    c(ins, FF_UNWINDING);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    c(ins, 0);
    ins.local_set(TARGET);
    ins.else_();
    g(ins, 0);
    c(ins, 0);
    call(b, ins, "fb_pending_has");
    ins.if_(BlockType::Empty);
    c(ins, 0);
    ins.local_set(TARGET);
    ins.else_();
    // The outermost scope of the pending and unwinding lists (the smallest depth).
    c(ins, 0);
    ins.local_set(MIN);
    c(ins, 0);
    ins.local_set(TARGET);
    for head in [F_PHEAD, F_UHEAD] {
        ldl(ins, 0, head);
        ins.local_set(N);
        ins.block(BlockType::Empty);
        ins.loop_(BlockType::Empty);
        g(ins, N);
        ins.i32_eqz();
        ins.br_if(1);
        ldl(ins, N, N_TARGET);
        ins.local_set(BEST);
        g(ins, BEST);
        ins.if_(BlockType::Empty);
        g(ins, TARGET);
        ins.i32_eqz();
        ldl(ins, BEST, S_DEPTH);
        g(ins, MIN);
        ins.i32_lt_s();
        ins.i32_or();
        ins.if_(BlockType::Empty);
        g(ins, BEST);
        ins.local_set(TARGET);
        ldl(ins, BEST, S_DEPTH);
        ins.local_set(MIN);
        ins.end();
        ins.end();
        ldl(ins, N, N_NEXT);
        ins.local_set(NEXT);
        g(ins, NEXT);
        ins.local_set(N);
        ins.br(0);
        ins.end();
        ins.end();
    }
    ins.end();
    ins.end();
    g(ins, TARGET);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    // The whole fiber: forget every narrower interruption and remember that it unwinds.
    ldl(ins, 0, F_FLAGS);
    c(ins, FF_UNWINDING);
    ins.i32_or();
    ins.local_set(F);
    stl(ins, 0, F_FLAGS, F);
    g(ins, 0);
    call(b, ins, "fb_pending_clear");
    c(ins, 0);
    ins.local_set(TARGET);
    ins.else_();
    // A scope's body unwinds: it moves from pending to unwinding, so the scope boundary can
    // clear it when the body is done.
    g(ins, 0);
    g(ins, TARGET);
    call(b, ins, "fb_pending_remove");
    push_node(b, ins, 0, TARGET, F_UHEAD, F_UTAIL, 6);
    ins.end();
    new_interruption(b, ins, TARGET);
}

// ---- waits ----

/// Emits the head/tail slot offsets of the wait's target into locals `h` and `t`.
fn target_slots(b: &mut Builder<'_>, ins: &mut InstructionSink, w: u32, h: u32, t: u32) {
    // A Deferred keeps its waiter list at D_WHEAD/D_WTAIL; a fiber at F_WHEAD/F_WTAIL.
    ldl(ins, w, W_KIND);
    c(ins, WK_DEFERRED);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    c(ins, D_WHEAD as i32);
    ins.local_set(h);
    c(ins, D_WTAIL as i32);
    ins.local_set(t);
    ins.else_();
    c(ins, F_WHEAD as i32);
    ins.local_set(h);
    c(ins, F_WTAIL as i32);
    ins.local_set(t);
    ins.end();
    let _ = b;
}

/// `fb_wait_new(kind, repr) -> Ref`: a new wait, not yet registered or suspended.
fn wait_new(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const W: u32 = 2;
    let desc = plan(b).waiter_desc;
    c(ins, W_SLOTS);
    c(ins, desc as i32);
    call(b, ins, "alloc");
    ins.local_set(W);
    for off in [
        W_CHAIN, W_FIBER, W_TARGET, W_PREV, W_NEXT, W_LIVE, W_ONDROP, W_END, W_FLAGS, W_SEQ,
        W_HPREV, W_HNEXT, W_TPREV, W_TNEXT, W_IMPL,
    ] {
        st_const(ins, W, off, 0);
    }
    stl(ins, W, W_KIND, 0);
    stl(ins, W, W_VERB, 0);
    stl(ins, W, W_REPR, 1);
    ins.local_get(W);
    ins.i64_const(0);
    ins.i64_store(mem(W_AT, 8));
    g(ins, W);
}

/// `fb_watch_add(target, w)`: add a wait to a fiber's or a Deferred's list of waiters.
fn watch_add(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const T: u32 = 2;
    const HEAD: u32 = 3;
    const TAIL: u32 = 4;
    const TAILV: u32 = 5;
    g(ins, 0);
    ins.local_set(T);
    target_slots(b, ins, 1, HEAD, TAIL);
    stl(ins, 1, W_TARGET, 0);
    // `head`/`tail` are offsets from the target's record, held as the local values.
    g(ins, T);
    g(ins, TAIL);
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.local_set(TAILV);
    stl(ins, 1, W_TPREV, TAILV);
    st_const(ins, 1, W_TNEXT, 0);
    g(ins, TAILV);
    ins.if_(BlockType::Empty);
    stl(ins, TAILV, W_TNEXT, 1);
    ins.else_();
    g(ins, T);
    g(ins, HEAD);
    ins.i32_add();
    g(ins, 1);
    ins.i32_store(mem(0, 4));
    ins.end();
    g(ins, T);
    g(ins, TAIL);
    ins.i32_add();
    g(ins, 1);
    ins.i32_store(mem(0, 4));
    ldl(ins, 1, W_FLAGS);
    c(ins, WF_TARGET);
    ins.i32_or();
    ins.local_set(2);
    stl(ins, 1, W_FLAGS, 2);
}

/// `fb_watch_remove(w)`: take a wait out of its target's list.
fn watch_remove(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const T: u32 = 2;
    const HEAD: u32 = 3;
    const TAIL: u32 = 4;
    const PREV: u32 = 5;
    const NEXT: u32 = 6;
    ldl(ins, 0, W_FLAGS);
    c(ins, WF_TARGET);
    ins.i32_and();
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ldl(ins, 0, W_TARGET);
    ins.local_set(T);
    target_slots(b, ins, 0, HEAD, TAIL);
    ldl(ins, 0, W_TPREV);
    ins.local_set(PREV);
    ldl(ins, 0, W_TNEXT);
    ins.local_set(NEXT);
    g(ins, PREV);
    ins.if_(BlockType::Empty);
    stl(ins, PREV, W_TNEXT, NEXT);
    ins.else_();
    g(ins, T);
    g(ins, HEAD);
    ins.i32_add();
    g(ins, NEXT);
    ins.i32_store(mem(0, 4));
    ins.end();
    g(ins, NEXT);
    ins.if_(BlockType::Empty);
    stl(ins, NEXT, W_TPREV, PREV);
    ins.else_();
    g(ins, T);
    g(ins, TAIL);
    ins.i32_add();
    g(ins, PREV);
    ins.i32_store(mem(0, 4));
    ins.end();
    ldl(ins, 0, W_FLAGS);
    c(ins, -1 - WF_TARGET);
    ins.i32_and();
    ins.local_set(2);
    stl(ins, 0, W_FLAGS, 2);
    st_const(ins, 0, W_TARGET, 0);
    st_const(ins, 0, W_TPREV, 0);
    st_const(ins, 0, W_TNEXT, 0);
}

/// `fb_unlink_wait(w)`: take a wait out of every list it is in and forget the links to it.
fn unlink_wait(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const F: u32 = 2;
    const P: u32 = 3;
    const PREV: u32 = 4;
    const NEXT: u32 = 5;
    ldl(ins, 0, W_FLAGS);
    c(ins, WF_TARGET);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    g(ins, 0);
    call(b, ins, "fb_watch_remove");
    ins.end();
    // The run's list, then the sleepers' list: both use their own pair of links.
    ldl(ins, 0, W_FLAGS);
    c(ins, WF_ROOT);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ldl(ins, 0, W_PREV);
    ins.local_set(PREV);
    ldl(ins, 0, W_NEXT);
    ins.local_set(NEXT);
    g(ins, PREV);
    ins.if_(BlockType::Empty);
    stl(ins, PREV, W_NEXT, NEXT);
    ins.else_();
    c(ins, cell(b, |p| p.whead) as i32);
    g(ins, NEXT);
    ins.i32_store(mem(0, 4));
    ins.end();
    g(ins, NEXT);
    ins.if_(BlockType::Empty);
    stl(ins, NEXT, W_PREV, PREV);
    ins.else_();
    c(ins, cell(b, |p| p.wtail) as i32);
    g(ins, PREV);
    ins.i32_store(mem(0, 4));
    ins.end();
    ldl(ins, 0, W_FLAGS);
    c(ins, -1 - WF_ROOT);
    ins.i32_and();
    ins.local_set(P);
    stl(ins, 0, W_FLAGS, P);
    c(ins, cell(b, |p| p.live_waits) as i32);
    c(ins, cell(b, |p| p.live_waits) as i32);
    ins.i32_load(mem(0, 4));
    c(ins, 1);
    ins.i32_sub();
    ins.i32_store(mem(0, 4));
    ins.end();
    for (flag, head, tail) in [
        (WF_HOST, cell(b, |p| p.host_head), cell(b, |p| p.host_tail)),
        (WF_TEST, cell(b, |p| p.test_head), cell(b, |p| p.test_tail)),
    ] {
        ldl(ins, 0, W_FLAGS);
        c(ins, flag);
        ins.i32_and();
        ins.if_(BlockType::Empty);
        ldl(ins, 0, W_HPREV);
        ins.local_set(PREV);
        ldl(ins, 0, W_HNEXT);
        ins.local_set(NEXT);
        g(ins, PREV);
        ins.if_(BlockType::Empty);
        stl(ins, PREV, W_HNEXT, NEXT);
        ins.else_();
        c(ins, head as i32);
        g(ins, NEXT);
        ins.i32_store(mem(0, 4));
        ins.end();
        g(ins, NEXT);
        ins.if_(BlockType::Empty);
        stl(ins, NEXT, W_HPREV, PREV);
        ins.else_();
        c(ins, tail as i32);
        g(ins, PREV);
        ins.i32_store(mem(0, 4));
        ins.end();
        ldl(ins, 0, W_FLAGS);
        c(ins, -1 - flag);
        ins.i32_and();
        ins.local_set(P);
        stl(ins, 0, W_FLAGS, P);
        ins.end();
    }
    st_const(ins, 0, W_LIVE, 0);
    // The fiber parked on it, and the operation call it belongs to.
    ldl(ins, 0, W_FIBER);
    ins.local_set(F);
    g(ins, F);
    ins.if_(BlockType::Empty);
    ldl(ins, F, F_WAITER);
    g(ins, 0);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    st_const(ins, F, F_WAITER, 0);
    ins.end();
    ins.end();
    // The operation calls that pointed at this wait release it.
    ldl(ins, 0, W_ONDROP);
    ins.local_set(P);
    g(ins, P);
    ins.if_(BlockType::Empty);
    ldl(ins, P, P_CALLER);
    g(ins, 0);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ldl(ins, P, P_CALLER);
    ins.local_set(NEXT);
    st_const(ins, P, P_CALLER, 0);
    g(ins, NEXT);
    call(b, ins, "drop");
    ins.end();
    ins.end();
    ldl(ins, 0, W_IMPL);
    ins.local_set(P);
    g(ins, P);
    ins.if_(BlockType::Empty);
    ldl(ins, P, P_IMPL);
    g(ins, 0);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ldl(ins, P, P_IMPL);
    ins.local_set(NEXT);
    st_const(ins, P, P_IMPL, 0);
    g(ins, NEXT);
    call(b, ins, "drop");
    ins.end();
    ins.end();
    let _ = F;
}

/// `fb_drop_wait(w)`: a waiting continuation is abandoned (the run stalled, or its fiber is
/// interrupted). The implementation it waits for is unwound too, innermost first, as an orphan
/// continuation (`em_drop_wait` of the C runtime).
fn drop_wait(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const P: u32 = 1;
    const IW: u32 = 2;
    const CHAIN: u32 = 3;
    const F: u32 = 4;
    ldl(ins, 0, W_LIVE);
    ins.if_(BlockType::Empty);
    ins.else_();
    ins.return_();
    ins.end();
    st_const(ins, 0, W_LIVE, 0);
    g(ins, 0);
    call(b, ins, "fb_unlink_wait");
    ldl(ins, 0, W_ONDROP);
    ins.local_set(P);
    g(ins, P);
    ins.if_(BlockType::Empty);
    ldl(ins, P, P_USED);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ldl(ins, P, P_IMPL);
    ins.local_set(IW);
    g(ins, IW);
    ins.if_(BlockType::Empty);
    ldl(ins, IW, W_LIVE);
    ins.if_(BlockType::Empty);
    g(ins, IW);
    call(b, ins, "fb_drop_wait");
    ldl(ins, IW, W_CHAIN);
    ins.local_set(CHAIN);
    st_const(ins, IW, W_CHAIN, 0);
    ldl(ins, IW, W_FIBER);
    ins.local_set(F);
    g(ins, F);
    call(b, ins, "dup");
    c(ins, JK_INTERRUPT);
    g(ins, F);
    g(ins, CHAIN);
    c(ins, WE_ORPHAN);
    c(ins, 0);
    ins.i64_const(0);
    c(ins, 0);
    c(ins, 0);
    let push = b.rt("fb_queue_push");
    ins.call(push);
    // The recursion left the wait's own reference for this caller.
    dropv(b, ins, IW);
    ins.end();
    ins.end();
    ins.end();
    ins.end();
    // The wait's references to operation calls go with it.
    for off in [W_ONDROP, W_IMPL] {
        ldl(ins, 0, off);
        ins.local_set(IW);
        st_const(ins, 0, off, 0);
        g(ins, IW);
        ins.if_(BlockType::Empty);
        g(ins, IW);
        call(b, ins, "drop");
        ins.end();
    }
}

/// `fb_wait_chain(w) -> Ref`: take the suspended continuation out of the wait.
fn wait_chain(_b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const C: u32 = 1;
    ldl(ins, 0, W_CHAIN);
    ins.local_set(C);
    st_const(ins, 0, W_CHAIN, 0);
    g(ins, C);
}

/// `fb_wait_wake(w, mode, word)`: continue the wait with the input. The wait is consumed.
///
/// A continuation of the fiber that is running resumes now, inside the wake; one of another
/// fiber is queued (the C runtime's `em_cont`). The event loop runs with nothing stepping, so its
/// wakes are queued too.
fn wait_wake(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const F: u32 = 3;
    const CHAIN: u32 = 4;
    const END: u32 = 5;
    const PERF: u32 = 6;
    const LIVE: u32 = 7;
    // A value that is unused (a second wakeup, an interruption of a finished fiber) is released.
    ldl(ins, 0, W_LIVE);
    ins.local_set(LIVE);
    g(ins, LIVE);
    ins.if_(BlockType::Empty);
    ins.else_();
    // Drop the input when the wait was already woken or unwound.
    g(ins, 1);
    ins.i32_eqz();
    ldl(ins, 0, W_REPR);
    c(ins, R_REF);
    ins.i32_eq();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.i32_wrap_i64();
    call(b, ins, "drop");
    ins.end();
    // The caller still owns this wait: the wake consumes it even when it was already dead.
    dropv(b, ins, 0);
    ins.return_();
    ins.end();
    ldl(ins, 0, W_FIBER);
    ins.local_set(F);
    g(ins, F);
    ins.if_(BlockType::Empty);
    ins.else_();
    // A live wait always names its fiber, so this is defensive; consume the wake like the dead
    // case so the caller's reference is not leaked.
    g(ins, 1);
    ins.i32_eqz();
    ldl(ins, 0, W_REPR);
    c(ins, R_REF);
    ins.i32_eq();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.i32_wrap_i64();
    call(b, ins, "drop");
    ins.end();
    dropv(b, ins, 0);
    ins.return_();
    ins.end();
    g(ins, F);
    call(b, ins, "dup");
    ldl(ins, 0, W_CHAIN);
    ins.local_set(CHAIN);
    st_const(ins, 0, W_CHAIN, 0);
    ldl(ins, 0, W_END);
    ins.local_set(END);
    // The operation call whose strand this continuation belongs to (0 for a fiber's own).  The
    // wait keeps the reference while it is unlinked: `fb_unlink_wait` clears the perform's
    // back-pointer and releases the wait reference the perform held.
    ldl(ins, 0, W_IMPL);
    ins.local_set(PERF);
    g(ins, 0);
    call(b, ins, "fb_unlink_wait");
    st_const(ins, 0, W_IMPL, 0);
    // The perform the wait held for a dropped caller is released with the wait.
    ldl(ins, 0, W_ONDROP);
    ins.local_set(LIVE);
    st_const(ins, 0, W_ONDROP, 0);
    g(ins, LIVE);
    ins.if_(BlockType::Empty);
    g(ins, LIVE);
    call(b, ins, "drop");
    ins.end();
    dropv(b, ins, 0);
    g(ins, CHAIN);
    ins.if_(BlockType::Empty);
    ins.else_();
    dropv(b, ins, F);
    dropv(b, ins, PERF);
    g(ins, 1);
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.i32_wrap_i64();
    ins.drop();
    ins.end();
    ins.return_();
    ins.end();
    // Resume inside the wake when the continuation is the fiber that is stepping now.
    i32_const_cell(b, ins, |p| p.stepping);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    i32_const_cell(b, ins, |p| p.cur_fiber);
    ins.i32_load(mem(0, 4));
    g(ins, F);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, F);
    g(ins, CHAIN);
    g(ins, END);
    g(ins, 1);
    ins.local_get(2);
    g(ins, PERF);
    let advance = b.rt("fb_advance");
    ins.call(advance);
    ins.return_();
    ins.end();
    ins.end();
    c(ins, JK_RESUME);
    g(ins, F);
    g(ins, CHAIN);
    g(ins, END);
    g(ins, 1);
    ins.local_get(2);
    g(ins, PERF);
    ldl(ins, 0, W_REPR);
    call(b, ins, "fb_queue_push");
}

// ---- the run's queue ----

/// `fb_queue_push(kind, fiber, chain, end, mode, word, perform, repr)`: append a job. The job
/// takes the references; when the run is over they are released here. `repr` is the
/// representation of a value-mode `word` (0 when it cannot be a reference), so the drain can
/// release the value an interruption replaces.
fn queue_push(b: &mut Builder<'_>, ins: &mut InstructionSink, list: impl Fn(&Plan) -> (u32, u32)) {
    const J: u32 = 8;
    let desc = plan(b).job_desc;
    let (head, tail) = list(plan(b));
    i32_const_cell(b, ins, |p| p.run_over);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    for l in [1u32, 2, 6] {
        dropv(b, ins, l);
    }
    ins.return_();
    ins.end();
    c(ins, J_SLOTS);
    c(ins, desc as i32);
    call(b, ins, "alloc");
    ins.local_set(J);
    stl(ins, J, J_KIND, 0);
    stl(ins, J, J_END, 3);
    stl(ins, J, J_FIBER, 1);
    stl(ins, J, J_CHAIN, 2);
    stl(ins, J, J_PERF, 6);
    stl(ins, J, J_MODE, 4);
    stl(ins, J, J_REPR, 7);
    ins.local_get(J);
    ins.local_get(5);
    ins.i64_store(mem(J_WORD, 8));
    st_const(ins, J, J_NEXT, 0);
    c(ins, tail as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    c(ins, tail as i32);
    ins.i32_load(mem(0, 4));
    g(ins, J);
    ins.i32_store(mem(J_NEXT, 4));
    ins.else_();
    c(ins, head as i32);
    g(ins, J);
    ins.i32_store(mem(0, 4));
    ins.end();
    c(ins, tail as i32);
    g(ins, J);
    ins.i32_store(mem(0, 4));
}

/// `fb_later_move()`: move the head of the later list to the run's queue, so the next drain runs
/// it after a turn of the event loop (the C runtime's `em_run`).
fn later_move(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const J: u32 = 0;
    let lhead = cell(b, |p| p.lhead);
    let ltail = cell(b, |p| p.ltail);
    let qhead = cell(b, |p| p.qhead);
    let qtail = cell(b, |p| p.qtail);
    c(ins, lhead as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(J);
    g(ins, J);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    c(ins, lhead as i32);
    ldl(ins, J, J_NEXT);
    ins.i32_store(mem(0, 4));
    c(ins, lhead as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    c(ins, ltail as i32);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    ins.end();
    st_const(ins, J, J_NEXT, 0);
    c(ins, qtail as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    c(ins, qtail as i32);
    ins.i32_load(mem(0, 4));
    g(ins, J);
    ins.i32_store(mem(J_NEXT, 4));
    ins.else_();
    c(ins, qhead as i32);
    g(ins, J);
    ins.i32_store(mem(0, 4));
    ins.end();
    c(ins, qtail as i32);
    g(ins, J);
    ins.i32_store(mem(0, 4));
}

fn i32_const_cell(b: &mut Builder<'_>, ins: &mut InstructionSink, f: impl Fn(&Plan) -> u32) {
    ins.i32_const(cell(b, f) as i32);
}

/// The same for a cell of the suspend runtime.
fn sic(
    b: &mut Builder<'_>,
    ins: &mut InstructionSink,
    f: impl Fn(&super::super::suspend::Plan) -> u32,
) {
    ins.i32_const(super::sched::cell(b, f) as i32);
}

/// `fb_drain()`: run the queued jobs in order until the queue is empty or the run ends.
fn drain(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const J: u32 = 0;
    const KIND: u32 = 1;
    const F: u32 = 2;
    const CHAIN: u32 = 3;
    const PERF: u32 = 4;
    const END: u32 = 5;
    const MODE: u32 = 6;
    const TMP: u32 = 7;
    const WORD: u32 = 8; // i64
    const REPR: u32 = 9;
    const WAIT: u32 = 10;
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    i32_const_cell(b, ins, |p| p.qhead);
    ins.i32_load(mem(0, 4));
    ins.local_tee(J);
    ins.i32_eqz();
    ins.br_if(1);
    // Pop it and cut the links.
    ldl(ins, J, J_NEXT);
    ins.local_set(TMP);
    i32_const_cell(b, ins, |p| p.qhead);
    g(ins, TMP);
    ins.i32_store(mem(0, 4));
    g(ins, TMP);
    ins.if_(BlockType::Empty);
    ins.else_();
    i32_const_cell(b, ins, |p| p.qtail);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    ins.end();
    ldl(ins, J, J_KIND);
    ins.local_set(KIND);
    ldl(ins, J, J_FIBER);
    ins.local_set(F);
    ldl(ins, J, J_CHAIN);
    ins.local_set(CHAIN);
    ldl(ins, J, J_PERF);
    ins.local_set(PERF);
    ldl(ins, J, J_END);
    ins.local_set(END);
    ldl(ins, J, J_MODE);
    ins.local_set(MODE);
    ldl(ins, J, J_REPR);
    ins.local_set(REPR);
    ins.local_get(J);
    ins.i64_load(mem(J_WORD, 8));
    ins.local_set(WORD);
    // The job's shell goes; the values are now in locals.
    st_const(ins, J, J_FIBER, 0);
    st_const(ins, J, J_CHAIN, 0);
    st_const(ins, J, J_PERF, 0);
    dropv(b, ins, J);
    i32_const_cell(b, ins, |p| p.run_over);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    dropv(b, ins, F);
    dropv(b, ins, CHAIN);
    dropv(b, ins, PERF);
    // The loop is one label out from inside this `if`.
    ins.br(1);
    ins.end();
    // A fairness yield: the job holds the pause wait, which holds the continuation (the C
    // runtime's `EM_JOB_GO`). Taking it here keeps the wait's links to the fiber and to the
    // operation call, so an interruption can still find and unwind them; a wait an interruption
    // already took is skipped.
    g(ins, KIND);
    c(ins, JK_GO);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, CHAIN);
    ins.local_set(WAIT);
    ldl(ins, WAIT, W_LIVE);
    ins.if_(BlockType::Empty);
    ldl(ins, WAIT, W_CHAIN);
    ins.local_set(CHAIN);
    st_const(ins, WAIT, W_CHAIN, 0);
    ldl(ins, WAIT, W_END);
    ins.local_set(END);
    ldl(ins, WAIT, W_IMPL);
    ins.local_set(PERF);
    g(ins, WAIT);
    call(b, ins, "fb_unlink_wait");
    // The perform reference moved to `PERF`; the wait must not drop it too.
    st_const(ins, WAIT, W_IMPL, 0);
    // The run's list and the job each owned a reference to the wait.
    dropv(b, ins, WAIT);
    dropv(b, ins, WAIT);
    c(ins, JK_RESUME);
    ins.local_set(KIND);
    ins.else_();
    // The wait was taken (an interruption unwound its continuation): drop the job's references
    // and go on with the next job (the loop is two labels out: the target `if` and the kind `if`).
    dropv(b, ins, WAIT);
    dropv(b, ins, F);
    ins.br(2);
    ins.end();
    ins.end();
    g(ins, KIND);
    c(ins, JK_BEGIN);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, F);
    g(ins, CHAIN);
    g(ins, MODE);
    g(ins, WORD);
    ins.i32_wrap_i64();
    let begin = b.rt("fb_begin");
    ins.call(begin);
    ins.br(1);
    ins.end();
    g(ins, KIND);
    c(ins, JK_INTERRUPT);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, END);
    c(ins, WE_MAIN);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, F);
    let deliver = b.rt("fb_deliver");
    ins.call(deliver);
    ins.local_set(TMP);
    ins.local_get(TMP);
    ins.i64_extend_i32_u();
    ins.local_set(WORD);
    c(ins, 2);
    ins.local_set(MODE);
    ins.else_();
    c(ins, 0);
    let intr = b.rt("fb_interrupted_new");
    ins.call(intr);
    ins.i64_extend_i32_u();
    ins.local_set(WORD);
    c(ins, 2);
    ins.local_set(MODE);
    ins.end();
    ins.end();
    // A queued wake resumes as the main continuation only when the fiber is not interrupted: the
    // value is not delivered, the interruption is (the C runtime's `em_fiber_resume`). A fairness
    // yield of the main continuation is queued the same way.
    g(ins, KIND);
    c(ins, JK_RESUME);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, END);
    c(ins, WE_MAIN);
    ins.i32_eq();
    g(ins, MODE);
    c(ins, 2);
    ins.i32_ne();
    ins.i32_and();
    g(ins, F);
    let interrupting = b.rt("fb_interrupting");
    ins.call(interrupting);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    // Release the value the interruption replaces: a failure word is always a reference, a value
    // word only in the reference representation.
    g(ins, MODE);
    c(ins, 1);
    ins.i32_eq();
    g(ins, MODE);
    ins.i32_eqz();
    g(ins, REPR);
    c(ins, R_REF);
    ins.i32_eq();
    ins.i32_and();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.local_get(WORD);
    ins.i32_wrap_i64();
    let drop = b.rt("drop");
    ins.call(drop);
    ins.end();
    g(ins, F);
    let deliver = b.rt("fb_deliver");
    ins.call(deliver);
    ins.local_set(TMP);
    g(ins, TMP);
    ins.i64_extend_i32_u();
    ins.local_set(WORD);
    c(ins, 2);
    ins.local_set(MODE);
    ins.end();
    ins.end();
    g(ins, F);
    g(ins, CHAIN);
    g(ins, END);
    g(ins, MODE);
    g(ins, WORD);
    g(ins, PERF);
    let advance = b.rt("fb_advance");
    ins.call(advance);
    ins.br(0);
    ins.end();
    ins.end();
}

/// `fb_advance(f, chain, end, mode, word, perform)`: run the continuation until it ends or
/// suspends. It takes the references to the fiber, the chain and the operation call.
fn advance(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const PREVF: u32 = 6;
    const PREVM: u32 = 7;
    const FR: u32 = 8;
    const MODE: u32 = 9;
    const W: u32 = 10;
    const CHAIN: u32 = 11;
    const PREVS: u32 = 13;
    const WORD: u32 = 14; // i64
    i32_const_cell(b, ins, |p| p.cur_fiber);
    ins.i32_load(mem(0, 4));
    ins.local_set(PREVF);
    i32_const_cell(b, ins, |p| p.stepping);
    ins.i32_load(mem(0, 4));
    ins.local_set(PREVS);
    i32_const_cell(b, ins, |p| p.stepping);
    c(ins, 1);
    ins.i32_store(mem(0, 4));
    i32_const_cell(b, ins, |p| p.cur_main);
    ins.i32_load(mem(0, 4));
    ins.local_set(PREVM);
    i32_const_cell(b, ins, |p| p.cur_fiber);
    g(ins, 0);
    ins.i32_store(mem(0, 4));
    i32_const_cell(b, ins, |p| p.cur_main);
    g(ins, 2);
    ins.i32_eqz();
    ins.i32_store(mem(0, 4));
    g(ins, 3);
    ins.local_set(MODE);
    ins.local_get(4);
    ins.local_set(WORD);
    g(ins, 1);
    ins.local_set(CHAIN);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    i32_const_cell(b, ins, |p| p.run_over);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    dropv(b, ins, CHAIN);
    // The perform reference was handed to this continuation and is not used any more.
    dropv(b, ins, 5);
    g(ins, MODE);
    ins.if_(BlockType::Empty);
    g(ins, WORD);
    ins.i32_wrap_i64();
    ins.drop();
    ins.end();
    // Inside the `if`, depth 2 is the end of the outer block (0: the `if`, 1: the loop).
    ins.br(2);
    ins.end();
    g(ins, CHAIN);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    g(ins, 0);
    g(ins, 2);
    g(ins, MODE);
    ins.local_get(WORD);
    g(ins, 5);
    let strand = b.rt("fb_strand_end");
    ins.call(strand);
    ins.br(2);
    ins.end();
    g(ins, CHAIN);
    ins.local_set(FR);
    ldl(ins, FR, F_NEXT);
    ins.local_set(CHAIN);
    st_const(ins, FR, F_NEXT, 0);
    sic(b, ins, |p| p.in_mode);
    g(ins, MODE);
    ins.i32_store(mem(0, 4));
    sic(b, ins, |p| p.in_word);
    ins.local_get(WORD);
    ins.i64_store(mem(0, 8));
    // The frame's resume trampoline takes the frame.
    let ty = b.call_ty(&[Repr::Ref], Repr::Void);
    g(ins, FR);
    g(ins, FR);
    ins.i32_load(mem(HDR, 4));
    ins.call_indirect(0, ty);
    ins.global_get(G_SUSPENDING);
    ins.if_(BlockType::Empty);
    c(ins, 0);
    let take = b.rt("frame_take");
    g(ins, CHAIN);
    ins.call(take);
    ins.local_set(CHAIN);
    i32_const_cell(b, ins, |p| p.susp_wait);
    ins.i32_load(mem(0, 4));
    ins.local_set(W);
    i32_const_cell(b, ins, |p| p.susp_wait);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    g(ins, W);
    ins.if_(BlockType::Empty);
    g(ins, W);
    g(ins, CHAIN);
    g(ins, 2);
    g(ins, 5);
    let suspended = b.rt("fb_suspended");
    ins.call(suspended);
    ins.else_();
    dropv(b, ins, CHAIN);
    ins.end();
    // `fb_suspended` took its own reference to the perform; release the one this continuation
    // was called with.
    dropv(b, ins, 5);
    // Inside the suspension `if`, depth 2 is the end of the outer block: stop running the
    // continuation here; the wait resumes it when it is woken.
    ins.br(2);
    ins.end();
    // The frame's resume function reports the next frame's input as `out_mode`/`out_word`
    // (0 value, 1 failure, 2 defect; 3 is the suspension handled above), like the C runtime's
    // `$advance` reads it.
    sic(b, ins, |p| p.out_mode);
    ins.i32_load(mem(0, 4));
    ins.local_set(MODE);
    sic(b, ins, |p| p.out_word);
    ins.i64_load(mem(0, 8));
    ins.local_set(WORD);
    ins.br(0);
    ins.end();
    ins.end();
    i32_const_cell(b, ins, |p| p.cur_fiber);
    g(ins, PREVF);
    ins.i32_store(mem(0, 4));
    i32_const_cell(b, ins, |p| p.cur_main);
    g(ins, PREVM);
    ins.i32_store(mem(0, 4));
    i32_const_cell(b, ins, |p| p.stepping);
    g(ins, PREVS);
    ins.i32_store(mem(0, 4));
    dropv(b, ins, 0);
}

/// `fb_begin(f, closure, thunk, repr)`: start a forked fiber by calling its function.
fn begin(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const PREVF: u32 = 4;
    const PREVM: u32 = 5;
    const MODE: u32 = 6;
    const CHAIN: u32 = 7;
    const W: u32 = 8;
    const TMP: u32 = 9;
    const WORD: u32 = 11; // i64
    // A child interrupted before it started ends without calling its function (spec 11.2).
    g(ins, 0);
    c(ins, 0);
    let has = b.rt("fb_pending_has");
    ins.call(has);
    ins.if_(BlockType::Empty);
    dropv(b, ins, 1);
    c(ins, 0);
    let intr = b.rt("fb_interrupted_new");
    ins.call(intr);
    ins.local_set(TMP);
    g(ins, 0);
    c(ins, 2);
    g(ins, TMP);
    ins.i64_extend_i32_u();
    let finish = b.rt("fb_finish");
    ins.call(finish);
    // The job's reference to the fiber (the run's list reference is the one `fb_finish` drops).
    dropv(b, ins, 0);
    ins.return_();
    ins.end();
    i32_const_cell(b, ins, |p| p.cur_fiber);
    ins.i32_load(mem(0, 4));
    ins.local_set(PREVF);
    i32_const_cell(b, ins, |p| p.cur_main);
    ins.i32_load(mem(0, 4));
    ins.local_set(PREVM);
    i32_const_cell(b, ins, |p| p.cur_fiber);
    g(ins, 0);
    ins.i32_store(mem(0, 4));
    i32_const_cell(b, ins, |p| p.cur_main);
    c(ins, 1);
    ins.i32_store(mem(0, 4));
    let ty = b.call_ty(&[Repr::Ref], Repr::I64);
    g(ins, 1);
    g(ins, 2);
    ins.call_indirect(0, ty);
    ins.local_set(WORD);
    dropv(b, ins, 1);
    ins.global_get(G_SUSPENDING);
    ins.if_(BlockType::Empty);
    // The child suspended: attach its continuation while it is still the current fiber (a
    // fairness yield queues it under its own record).
    c(ins, 0);
    let take = b.rt("frame_take");
    ins.call(take);
    ins.local_set(CHAIN);
    i32_const_cell(b, ins, |p| p.susp_wait);
    ins.i32_load(mem(0, 4));
    ins.local_set(W);
    i32_const_cell(b, ins, |p| p.susp_wait);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    g(ins, W);
    ins.if_(BlockType::Empty);
    g(ins, W);
    g(ins, CHAIN);
    c(ins, WE_MAIN);
    c(ins, 0);
    let suspended = b.rt("fb_suspended");
    ins.call(suspended);
    ins.else_();
    dropv(b, ins, CHAIN);
    ins.end();
    // The wait took its own reference to the fiber; release the begin job's.
    dropv(b, ins, 0);
    ins.return_();
    ins.end();
    i32_const_cell(b, ins, |p| p.cur_fiber);
    g(ins, PREVF);
    ins.i32_store(mem(0, 4));
    i32_const_cell(b, ins, |p| p.cur_main);
    g(ins, PREVM);
    ins.i32_store(mem(0, 4));
    ins.global_get(G_DEFECT);
    ins.if_(BlockType::Empty);
    ins.global_get(G_DEFECT);
    ins.i64_extend_i32_u();
    ins.local_set(WORD);
    c(ins, 0);
    ins.global_set(G_DEFECT);
    c(ins, 2);
    ins.local_set(MODE);
    ins.else_();
    ins.global_get(G_FAILURE);
    ins.if_(BlockType::Empty);
    ins.global_get(G_FAILURE);
    ins.i64_extend_i32_u();
    ins.local_set(WORD);
    c(ins, 0);
    ins.global_set(G_FAILURE);
    c(ins, 1);
    ins.local_set(MODE);
    ins.else_();
    c(ins, 0);
    ins.local_set(MODE);
    ins.end();
    ins.end();
    g(ins, 0);
    c(ins, WE_MAIN);
    g(ins, MODE);
    ins.local_get(WORD);
    c(ins, 0);
    let strand = b.rt("fb_strand_end");
    ins.call(strand);
    // The job's reference to the fiber: `fb_strand_end` (through `fb_finish`) drops the run's
    // list reference and may free the record.
    dropv(b, ins, 0);
}

/// `fb_suspended(w, chain, end, impl)`: attach the captured continuation to the request it
/// suspended at. `end` is whose continuation it is; an implementation continuation also names the
/// operation call it settles (`impl`). A fairness yield queues the fiber again.
fn suspended(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const F: u32 = 4;
    const P: u32 = 5;
    const KIND: u32 = 6;
    const FLAGS: u32 = 7;
    const TMP: u32 = 8;
    ldl(ins, 0, W_KIND);
    ins.local_set(KIND);
    // A fairness yield waits like any other suspension (the C runtime's `em_suspended`): the
    // attach below links it to the fiber or to the operation call it belongs to, and the job
    // queued at the end resumes it. Interrupting the fiber then finds the wait and unwinds the
    // implementation.
    // Attach the captured frames to the request. Enclosing operation calls attach their own
    // frames as their implementation unwinds too, so append to the chain instead of replacing it
    // (the C runtime's `em_take_suspension`).
    g(ins, 1);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.else_();
    ldl(ins, 0, W_CHAIN);
    ins.local_set(KIND);
    g(ins, KIND);
    ins.if_(BlockType::Empty);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ldl(ins, KIND, F_NEXT);
    ins.local_tee(P);
    ins.i32_eqz();
    ins.br_if(1);
    g(ins, P);
    ins.local_set(KIND);
    ins.br(0);
    ins.end();
    ins.end();
    stl(ins, KIND, F_NEXT, 1);
    ins.else_();
    ins.local_get(0);
    g(ins, 1);
    ins.i32_store(mem(W_CHAIN, 4));
    ins.end();
    ins.end();
    i32_const_cell(b, ins, |p| p.cur_fiber);
    ins.i32_load(mem(0, 4));
    ins.local_set(F);
    st_dup(b, ins, 0, W_FIBER, F);
    stl(ins, 0, W_END, 2);
    st_const(ins, 0, W_LIVE, 1);
    // The run's list of waits (newest last).
    i32_const_cell(b, ins, |p| p.wtail);
    ins.i32_load(mem(0, 4));
    ins.local_set(P);
    stl(ins, 0, W_PREV, P);
    st_const(ins, 0, W_NEXT, 0);
    g(ins, P);
    ins.if_(BlockType::Empty);
    stl(ins, P, W_NEXT, 0);
    ins.else_();
    i32_const_cell(b, ins, |p| p.whead);
    g(ins, 0);
    ins.i32_store(mem(0, 4));
    ins.end();
    i32_const_cell(b, ins, |p| p.wtail);
    g(ins, 0);
    ins.i32_store(mem(0, 4));
    ldl(ins, 0, W_FLAGS);
    c(ins, WF_ROOT);
    ins.i32_or();
    ins.local_set(FLAGS);
    stl(ins, 0, W_FLAGS, FLAGS);
    i32_const_cell(b, ins, |p| p.live_waits);
    i32_const_cell(b, ins, |p| p.live_waits);
    ins.i32_load(mem(0, 4));
    c(ins, 1);
    ins.i32_add();
    ins.i32_store(mem(0, 4));
    g(ins, 2);
    c(ins, WE_MAIN);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    // The fiber's main continuation is parked here (the C runtime's `f->wait`).
    stl(ins, F, F_WAITER, 0);
    ins.else_();
    g(ins, 2);
    c(ins, WE_IMPL);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    // An implementation's continuation: the operation call settles when it ends. Both ways point
    // at each other, like `impl->impl_wait` and `w->impl` in the C runtime.
    g(ins, 3);
    ins.local_set(P);
    g(ins, P);
    ins.if_(BlockType::Empty);
    st_dup(b, ins, 0, W_IMPL, P);
    st_dup(b, ins, P, P_IMPL, 0);
    ins.end();
    ins.end();
    ins.end();
    // Every 16th fairness yield waits for a turn of the event loop (the C runtime's `later`), so
    // a fiber that never waits still lets the others run in the same order.
    ldl(ins, 0, W_KIND);
    c(ins, WK_PAUSE);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    c(ins, cell(b, |p| p.pauses) as i32);
    ins.i32_load(mem(0, 4));
    c(ins, 1);
    ins.i32_add();
    ins.local_set(TMP);
    c(ins, cell(b, |p| p.pauses) as i32);
    g(ins, TMP);
    ins.i32_store(mem(0, 4));
    g(ins, TMP);
    c(ins, 16);
    ins.i32_rem_u();
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    c(ins, JK_GO);
    g(ins, F);
    call(b, ins, "dup");
    g(ins, F);
    g(ins, 0);
    call(b, ins, "dup");
    g(ins, 0);
    c(ins, 0);
    c(ins, 0);
    ins.i64_const(0);
    c(ins, 0);
    c(ins, 0);
    let late = b.rt("fb_later_push");
    ins.call(late);
    ins.else_();
    c(ins, JK_GO);
    g(ins, F);
    call(b, ins, "dup");
    g(ins, F);
    g(ins, 0);
    call(b, ins, "dup");
    g(ins, 0);
    c(ins, 0);
    c(ins, 0);
    ins.i64_const(0);
    c(ins, 0);
    c(ins, 0);
    let push = b.rt("fb_queue_push");
    ins.call(push);
    ins.end();
    ins.end();
    let _ = F;
}

/// `fb_cause_new(kind, value) -> Cause`: the value is borrowed (the cause duplicates it).
fn cause_new(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const C: u32 = 2;
    let desc = plan(b).cause_desc;
    c(ins, 1);
    c(ins, desc as i32);
    call(b, ins, "alloc");
    ins.local_set(C);
    g(ins, C);
    g(ins, 0);
    ins.i32_store(mem(4, 4));
    g(ins, 1);
    ins.if_(BlockType::Empty);
    st_dup(b, ins, C, HDR, 1);
    ins.else_();
    st_const(ins, C, HDR, 0);
    ins.end();
    g(ins, C);
}

/// `fb_cause_string(cause) -> String`: `error: ...`, `defect: ...` or `interrupted`. Borrows the
/// cause.
fn cause_string(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const SHOWN: u32 = 1;
    const TMP: u32 = 2;
    let error = b.string_literal(b"error: ");
    let defect = b.string_literal(b"defect: ");
    let interrupted = b.string_literal(b"interrupted");
    let append = b.rt("str_append");
    let show = b.rt("show_failure");
    ins.local_get(0);
    ins.i32_load(mem(4, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    // `show_failure` borrows the cause's error; the cause keeps its own reference.
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.call(show);
    ins.local_tee(SHOWN);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    c(ins, error as i32);
    ins.return_();
    ins.end();
    c(ins, error as i32);
    g(ins, SHOWN);
    ins.call(append);
    ins.local_set(TMP);
    dropv(b, ins, SHOWN);
    g(ins, TMP);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(4, 4));
    c(ins, CAUSE_DIE);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    c(ins, defect as i32);
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.call(append);
    ins.return_();
    ins.end();
    c(ins, interrupted as i32);
}

/// Emits the `Any` value stored at `base`+`off`, unboxed to the representation code in local
/// `repr`, as an `i64` word in local `word` (the C runtime's `em_unbox`). A reference is the
/// value itself and is duplicated for the caller; `Void` is zero; any other value is the first
/// slot of its box.
fn unbox_word(
    b: &mut Builder<'_>,
    ins: &mut InstructionSink,
    repr: u32,
    base: u32,
    off: u32,
    tmp: u32,
    word: u32,
) {
    ldl(ins, base, off);
    ins.local_set(tmp);
    g(ins, repr);
    c(ins, R_REF);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, tmp);
    call(b, ins, "dup");
    g(ins, tmp);
    ins.i64_extend_i32_u();
    ins.local_set(word);
    ins.else_();
    g(ins, repr);
    c(ins, 0);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i64_const(0);
    ins.local_set(word);
    ins.else_();
    g(ins, repr);
    c(ins, 1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, tmp);
    ins.i32_load(mem(HDR, 4));
    ins.i64_extend_i32_u();
    ins.local_set(word);
    ins.else_();
    g(ins, tmp);
    ins.i64_load(mem(HDR, 8));
    ins.local_set(word);
    ins.end();
    ins.end();
    ins.end();
}

/// Stores the `Any` value at `base`+`off`, unboxed to the representation code in local `repr`,
/// into the first slot of the object in local `obj`.
fn unbox_slot(
    b: &mut Builder<'_>,
    ins: &mut InstructionSink,
    repr: u32,
    base: u32,
    off: u32,
    obj: u32,
) {
    g(ins, repr);
    c(ins, R_REF);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ldl(ins, base, off);
    call(b, ins, "dup");
    g(ins, obj);
    ldl(ins, base, off);
    ins.i32_store(mem(HDR, 4));
    ins.else_();
    g(ins, repr);
    c(ins, 0);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, obj);
    c(ins, 0);
    ins.i32_store(mem(HDR, 4));
    ins.else_();
    g(ins, repr);
    c(ins, 1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, obj);
    ldl(ins, base, off);
    ins.i32_load(mem(HDR, 4));
    ins.i32_store(mem(HDR, 4));
    ins.else_();
    g(ins, obj);
    ldl(ins, base, off);
    ins.i64_load(mem(HDR, 8));
    ins.i64_store(mem(HDR, 8));
    ins.end();
    ins.end();
    ins.end();
}

/// `fb_watch_settle(w, f) -> (mode, word)`: what a fiber's end delivers to one of its watchers.
/// The value is borrowed from the fiber where it can be (the wait duplicates it).
fn watch_settle(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const VERB: u32 = 2;
    const REPR: u32 = 3;
    const CAUSE: u32 = 4;
    const E: u32 = 5;
    const TMP: u32 = 6;
    const WORD: u32 = 7; // i64
    let success_val = plan(b).success_val;
    let success_ref = plan(b).success_ref;
    let failure = plan(b).failure_desc;
    let msg_join = plan(b).msg_join;
    let empty = b.string_literal(b"");
    ldl(ins, 0, W_VERB);
    ins.local_set(VERB);
    ldl(ins, 0, W_REPR);
    ins.local_set(REPR);
    ldl(ins, 1, F_CAUSE);
    ins.local_set(CAUSE);
    g(ins, CAUSE);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    // The child ended with a value. Its value is an `Any`: a reference is the value itself,
    // anything else is a box (the erased child returns `Any`), so unbox it to the expected
    // representation (the C runtime's `em_unbox`).
    g(ins, VERB);
    c(ins, WV_JOIN);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    unbox_word(b, ins, REPR, 1, F_RESVAL, TMP, WORD);
    c(ins, 0);
    ins.local_get(WORD);
    ins.return_();
    ins.end();
    g(ins, VERB);
    c(ins, WV_AWAIT);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    c(ins, 1);
    g(ins, REPR);
    c(ins, R_REF);
    ins.i32_eq();
    ins.if_(BlockType::Result(ValType::I32));
    c(ins, success_ref as i32);
    ins.else_();
    c(ins, success_val as i32);
    ins.end();
    call(b, ins, "alloc");
    ins.local_set(E);
    unbox_slot(b, ins, REPR, 1, F_RESVAL, E);
    c(ins, 0);
    g(ins, E);
    ins.i64_extend_i32_u();
    ins.return_();
    ins.end();
    ins.end();
    // An interruption waits for nobody; the others only reach join/await during a release.
    g(ins, VERB);
    c(ins, WV_JOIN);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, CAUSE);
    ins.i32_load(mem(4, 4));
    c(ins, CAUSE_INTERRUPT);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    c(ins, msg_join as i32);
    c(ins, empty as i32);
    call(b, ins, "str_append");
    ins.i64_extend_i32_u();
    ins.local_set(WORD);
    ins.else_();
    c(ins, 0);
    let intr = b.rt("fb_interrupted_new");
    ins.call(intr);
    ins.i64_extend_i32_u();
    ins.local_set(WORD);
    ins.end();
    c(ins, 2);
    ins.local_get(WORD);
    ins.return_();
    ins.end();
    g(ins, VERB);
    c(ins, WV_AWAIT);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    c(ins, 1);
    c(ins, failure as i32);
    call(b, ins, "alloc");
    ins.local_set(E);
    st_dup(b, ins, E, HDR, CAUSE);
    c(ins, 0);
    g(ins, E);
    ins.i64_extend_i32_u();
    ins.return_();
    ins.end();
    c(ins, 0);
    ins.i64_const(0);
    ins.return_();
    let _ = (TMP, CAUSE_INTERRUPT);
}

/// `fb_strand_end(f, end, mode, word, perform)`: a continuation ended. The input is owned.
fn strand_end(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const MODE: u32 = 5;
    const PERF: u32 = 6;
    const TMP: u32 = 7;
    const WORD: u32 = 8; // i64
    g(ins, 2);
    ins.local_set(MODE);
    ins.local_get(3);
    ins.local_set(WORD);
    g(ins, 4);
    ins.local_set(PERF);
    g(ins, 1);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    g(ins, 0);
    g(ins, MODE);
    ins.local_get(WORD);
    let finish = b.rt("fb_finish");
    ins.call(finish);
    ins.return_();
    ins.end();
    g(ins, 1);
    c(ins, WE_IMPL);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, PERF);
    g(ins, MODE);
    ins.local_get(WORD);
    let settled = b.rt("fb_settled_impl");
    ins.call(settled);
    dropv(b, ins, PERF);
    ins.return_();
    ins.end();
    g(ins, 1);
    c(ins, WE_SYNC);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, MODE);
    c(ins, 1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(WORD);
    ins.i32_wrap_i64();
    ins.global_set(G_FAILURE);
    ins.return_();
    ins.end();
    g(ins, MODE);
    c(ins, 2);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(WORD);
    ins.i32_wrap_i64();
    ins.global_set(G_DEFECT);
    ins.return_();
    ins.end();
    ins.local_get(WORD);
    ins.i32_wrap_i64();
    ins.drop();
    ins.return_();
    ins.end();
    // An orphan continuation: its defect is the run's result when nothing else has one (spec
    // 14.5), and an interruption is not a defect of the run.
    g(ins, MODE);
    c(ins, 2);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(WORD);
    ins.i32_wrap_i64();
    let intr = b.rt("fb_is_interrupted");
    ins.call(intr);
    ins.if_(BlockType::Empty);
    ins.local_get(WORD);
    ins.i32_wrap_i64();
    ins.drop();
    ins.return_();
    ins.end();
    i32_const_cell(b, ins, |p| p.orphan);
    ins.i32_load(mem(0, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    i32_const_cell(b, ins, |p| p.orphan);
    ins.local_get(WORD);
    ins.i32_wrap_i64();
    ins.i32_store(mem(0, 4));
    ins.return_();
    ins.end();
    ins.local_get(WORD);
    ins.i32_wrap_i64();
    ins.drop();
    ins.return_();
    ins.end();
    g(ins, MODE);
    c(ins, 1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(WORD);
    ins.i32_wrap_i64();
    ins.drop();
    ins.end();
    let _ = TMP;
}

/// `fb_finish(f, mode, word)`: the fiber ended with the input (owned).
fn finish(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const FLAGS: u32 = 3;
    const CAUSE: u32 = 4;
    const REPR: u32 = 5;
    const SCOPE: u32 = 6;
    const W: u32 = 7;
    const NEXT: u32 = 8;
    const TMP: u32 = 9;
    const WORD2: u32 = 10; // i64
    ldl(ins, 0, F_FLAGS);
    ins.local_set(FLAGS);
    g(ins, FLAGS);
    c(ins, FF_ENDED);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    g(ins, 1);
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.i32_wrap_i64();
    ins.drop();
    ins.else_();
    ldl(ins, 0, F_REPR);
    c(ins, R_REF);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.i32_wrap_i64();
    ins.drop();
    ins.end();
    ins.end();
    ins.return_();
    ins.end();
    g(ins, FLAGS);
    c(ins, FF_ENDED);
    ins.i32_or();
    ins.local_set(FLAGS);
    stl(ins, 0, F_FLAGS, FLAGS);
    st_const(ins, 0, F_WAITER, 0);
    c(ins, 0);
    ins.local_set(CAUSE);
    g(ins, 1);
    c(ins, 1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    c(ins, CAUSE_FAIL);
    ins.local_get(2);
    ins.i32_wrap_i64();
    let make = b.rt("fb_cause_new");
    ins.call(make);
    ins.local_set(CAUSE);
    ins.end();
    g(ins, 1);
    c(ins, 2);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.i32_wrap_i64();
    let intr = b.rt("fb_is_interrupted");
    ins.call(intr);
    ins.if_(BlockType::Empty);
    c(ins, CAUSE_INTERRUPT);
    c(ins, 0);
    let make = b.rt("fb_cause_new");
    ins.call(make);
    ins.local_set(CAUSE);
    ins.else_();
    c(ins, CAUSE_DIE);
    ins.local_get(2);
    ins.i32_wrap_i64();
    let make = b.rt("fb_cause_new");
    ins.call(make);
    ins.local_set(CAUSE);
    ins.end();
    ins.end();
    stl(ins, 0, F_CAUSE, CAUSE);
    ldl(ins, 0, F_REPR);
    ins.local_set(REPR);
    ldl(ins, 0, F_SCOPE);
    ins.local_set(SCOPE);
    g(ins, SCOPE);
    ins.if_(BlockType::Empty);
    // A child fiber: remember how it ended, then tell the scope and the watchers.
    stl(ins, 0, F_RESMODE, 1);
    ins.local_get(0);
    ins.local_get(2);
    ins.i64_store(mem(F_RESWORD, 8));
    c(ins, 0);
    ins.local_set(TMP);
    g(ins, 1);
    ins.i32_eqz();
    g(ins, REPR);
    c(ins, R_REF);
    ins.i32_eq();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.local_get(2);
    ins.i32_wrap_i64();
    ins.i32_store(mem(F_RESVAL, 4));
    ins.end();
    ins.else_();
    // The root fiber: its end is the run's result.
    g(ins, 1);
    c(ins, 1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    c(ins, 1);
    ins.local_get(2);
    ins.i32_wrap_i64();
    let result = b.rt("fb_run_result");
    ins.call(result);
    ins.else_();
    g(ins, 1);
    c(ins, 2);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    c(ins, 2);
    ins.local_get(2);
    ins.i32_wrap_i64();
    let result = b.rt("fb_run_result");
    ins.call(result);
    ins.else_();
    g(ins, REPR);
    c(ins, R_REF);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.i32_wrap_i64();
    ins.drop();
    ins.end();
    c(ins, 0);
    c(ins, 0);
    let result = b.rt("fb_run_result");
    ins.call(result);
    ins.end();
    ins.end();
    ins.end();
    // The fiber's own copy of the input is released once the cause or the result has it.
    g(ins, SCOPE);
    ins.if_(BlockType::Empty);
    g(ins, 1);
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.i32_wrap_i64();
    ins.drop();
    ins.end();
    // The scope, then the fibers waiting for it.
    g(ins, SCOPE);
    g(ins, 0);
    let child_ended = b.rt("fb_scope_child_ended");
    ins.call(child_ended);
    ldl(ins, 0, F_WHEAD);
    ins.local_set(W);
    st_const(ins, 0, F_WHEAD, 0);
    st_const(ins, 0, F_WTAIL, 0);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, W);
    ins.i32_eqz();
    ins.br_if(1);
    ldl(ins, W, W_TNEXT);
    ins.local_set(NEXT);
    st_const(ins, W, W_TNEXT, 0);
    st_const(ins, W, W_TPREV, 0);
    ldl(ins, W, W_FLAGS);
    c(ins, -1 - WF_TARGET);
    ins.i32_and();
    ins.local_set(TMP);
    stl(ins, W, W_FLAGS, TMP);
    g(ins, W);
    g(ins, 0);
    let settle = b.rt("fb_watch_settle");
    ins.call(settle);
    // (mode, word): the word is on top.
    ins.local_set(WORD2);
    ins.local_set(FLAGS);
    g(ins, W);
    g(ins, FLAGS);
    ins.local_get(WORD2);
    let wake = b.rt("fb_wait_wake");
    ins.call(wake);
    g(ins, NEXT);
    ins.local_set(W);
    ins.br(0);
    ins.end();
    ins.end();
    ins.end();
    // Out of the fiber's list; the list's reference is released at the end.
    ldl(ins, 0, F_PREVF);
    ins.local_set(TMP);
    g(ins, TMP);
    ins.if_(BlockType::Empty);
    stl(ins, TMP, F_NEXTF, NEXT);
    ins.else_();
    i32_const_cell(b, ins, |p| p.fhead);
    ldl(ins, 0, F_NEXTF);
    ins.i32_store(mem(0, 4));
    ins.end();
    ldl(ins, 0, F_NEXTF);
    ins.local_set(NEXT);
    g(ins, NEXT);
    ins.if_(BlockType::Empty);
    stl(ins, NEXT, F_PREVF, TMP);
    ins.else_();
    i32_const_cell(b, ins, |p| p.ftail);
    ldl(ins, 0, F_PREVF);
    ins.i32_store(mem(0, 4));
    ins.end();
    // A fiber that ends may still carry interruptions (one interrupted before it started ends
    // this way); their nodes are not reachable from the record once it is dropped.
    let clear = b.rt("fb_pending_clear");
    g(ins, 0);
    ins.call(clear);
    dropv(b, ins, 0);
}

/// `fb_run_result(mode, word)`: record the run's result (0 value, 1 failure, 2 defect). The word
/// is owned. The result outlives the strands still finishing, so `run_word` keeps its own
/// reference and `finish_run` puts it back before the exit report.
fn run_result(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    let run_over = cell(b, |p| p.run_over);
    let run_mode = cell(b, |p| p.run_mode);
    let run_word = cell(b, |p| p.run_word);
    let dup = b.rt("dup");
    let drop = b.rt("drop");
    // The first result stays (the C runtime's `em_root_finish`).
    c(ins, run_over as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    g(ins, 0);
    ins.if_(BlockType::Empty);
    g(ins, 1);
    ins.call(drop);
    ins.end();
    ins.return_();
    ins.end();
    c(ins, run_over as i32);
    c(ins, 1);
    ins.i32_store(mem(0, 4));
    c(ins, run_mode as i32);
    g(ins, 0);
    ins.i32_store(mem(0, 4));
    c(ins, run_word as i32);
    g(ins, 0);
    ins.if_(BlockType::Result(ValType::I32));
    g(ins, 1);
    ins.call(dup);
    g(ins, 1);
    ins.else_();
    c(ins, 0);
    ins.end();
    ins.i32_store(mem(0, 4));
    g(ins, 0);
    c(ins, 1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, 1);
    ins.global_set(G_FAILURE);
    ins.return_();
    ins.end();
    g(ins, 0);
    c(ins, 2);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, 1);
    ins.global_set(G_DEFECT);
    ins.end();
}

/// `fb_settled_impl(perform, mode, word)`: the continuation of an operation implementation
/// ended. A defect or failure before the resume is the caller's input; after the resume it ends
/// the run (spec 14.5, the C runtime's `em_settled`). A value is not observable.
fn settled_impl(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const SHOWN: u32 = 3;
    const TMP: u32 = 4;
    const W: u32 = 5;
    let word: u32 = 2; // the i64 parameter
    let wake = b.rt("fb_wait_wake");
    // A defect.
    g(ins, 1);
    c(ins, 2);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(P_USED, 4));
    ins.if_(BlockType::Empty);
    c(ins, 2);
    ins.local_get(word);
    ins.i32_wrap_i64();
    let result = b.rt("fb_run_result");
    ins.call(result);
    ins.return_();
    ins.end();
    ins.local_get(0);
    c(ins, 1);
    ins.i32_store(mem(P_USED, 4));
    ldl(ins, 0, P_CALLER);
    ins.local_set(W);
    st_const(ins, 0, P_CALLER, 0);
    g(ins, W);
    ins.if_(BlockType::Empty);
    g(ins, W);
    call(b, ins, "drop");
    g(ins, W);
    c(ins, 2);
    ins.local_get(word);
    ins.call(wake);
    ins.else_();
    ins.local_get(word);
    ins.i32_wrap_i64();
    ins.drop();
    ins.end();
    ins.return_();
    ins.end();
    // A failure.
    g(ins, 1);
    c(ins, 1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(P_USED, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(word);
    ins.i32_wrap_i64();
    let show = b.rt("show_failure");
    ins.call(show);
    ins.local_set(SHOWN);
    // `show_failure` borrows the failure; release it after building the message.
    ins.local_get(word);
    ins.i32_wrap_i64();
    call(b, ins, "drop");
    ldl(ins, 0, P_WHERE);
    let after = plan(b).failed_after;
    c(ins, after as i32);
    let append = b.rt("str_append");
    ins.call(append);
    ins.local_set(TMP);
    g(ins, SHOWN);
    ins.if_(BlockType::Empty);
    g(ins, TMP);
    g(ins, SHOWN);
    ins.call(append);
    ins.local_set(TMP);
    ins.end();
    g(ins, TMP);
    let raise = b.rt("raise");
    ins.call(raise);
    dropv(b, ins, TMP);
    dropv(b, ins, SHOWN);
    c(ins, 2);
    ins.global_get(G_DEFECT);
    let result = b.rt("fb_run_result");
    ins.call(result);
    ins.return_();
    ins.end();
    ins.local_get(0);
    c(ins, 1);
    ins.i32_store(mem(P_USED, 4));
    ldl(ins, 0, P_CALLER);
    ins.local_set(W);
    st_const(ins, 0, P_CALLER, 0);
    g(ins, W);
    ins.if_(BlockType::Empty);
    g(ins, W);
    call(b, ins, "drop");
    g(ins, W);
    c(ins, 1);
    ins.local_get(word);
    ins.call(wake);
    ins.else_();
    ins.local_get(word);
    ins.i32_wrap_i64();
    ins.drop();
    ins.end();
    ins.return_();
    ins.end();
    // A value after a resume is not observable; release it when it is a reference.
    ldl(ins, 0, P_REPR);
    c(ins, R_REF);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(word);
    ins.i32_wrap_i64();
    ins.drop();
    ins.end();
}

/// `fb_scope(body, thunk, repr)`: `Fiber.scope` (spec 11.2). The body takes the scope capability.
fn scope(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const SO: u32 = 3;
    const F: u32 = 4;
    const FR: u32 = 5;
    const MODE: u32 = 6;
    const TMP: u32 = 7;
    const DEPTH: u32 = 8;
    const WORD: u32 = 9; // i64
    let sdesc = plan(b).scope_desc;
    let bdesc = plan(b).scope_body_desc;
    let table = plan(b).scope_body_table;
    c(ins, S_SLOTS);
    c(ins, sdesc as i32);
    call(b, ins, "alloc");
    ins.local_set(SO);
    i32_const_cell(b, ins, |p| p.cur_fiber);
    ins.i32_load(mem(0, 4));
    ins.local_set(F);
    stl(ins, SO, S_FIBER, F);
    ldl(ins, F, F_SCOPES);
    ins.local_set(DEPTH);
    stl(ins, SO, S_DEPTH, DEPTH);
    g(ins, DEPTH);
    c(ins, 1);
    ins.i32_add();
    ins.local_set(DEPTH);
    stl(ins, F, F_SCOPES, DEPTH);
    st_const(ins, SO, S_FLAGS, 0);
    st_const(ins, SO, S_FIRST, 0);
    st_const(ins, SO, S_CHEAD, 0);
    st_const(ins, SO, S_CTAIL, 0);
    st_const(ins, SO, S_CLOSER, 0);
    // Call the body with the scope capability as its argument.
    let ty = b.call_ty(&[Repr::Ref, Repr::Ref], Repr::I64);
    g(ins, 0);
    g(ins, SO);
    call(b, ins, "dup");
    g(ins, SO);
    g(ins, 1);
    ins.call_indirect(0, ty);
    ins.local_set(WORD);
    ins.global_get(G_SUSPENDING);
    ins.if_(BlockType::Empty);
    // The body suspended: close the scope when its continuation returns.
    c(ins, bdesc as i32);
    c(ins, table as i32);
    c(ins, 0);
    c(ins, 2);
    let new = b.rt("frame_new");
    ins.call(new);
    ins.local_set(FR);
    stl(ins, FR, HDR + 24, SO);
    stl(ins, FR, HDR + 32, 2);
    let push = b.rt("frame_push");
    g(ins, FR);
    ins.call(push);
    ins.i64_const(0);
    ins.return_();
    ins.end();
    // The body returned: its value, failure or defect is the scope's input.
    c(ins, 0);
    ins.local_set(MODE);
    ins.global_get(G_DEFECT);
    ins.if_(BlockType::Empty);
    ins.global_get(G_DEFECT);
    ins.i64_extend_i32_u();
    ins.local_set(WORD);
    c(ins, 0);
    ins.global_set(G_DEFECT);
    c(ins, 2);
    ins.local_set(MODE);
    ins.else_();
    ins.global_get(G_FAILURE);
    ins.if_(BlockType::Empty);
    ins.global_get(G_FAILURE);
    ins.i64_extend_i32_u();
    ins.local_set(WORD);
    c(ins, 0);
    ins.global_set(G_FAILURE);
    c(ins, 1);
    ins.local_set(MODE);
    ins.end();
    ins.end();
    g(ins, SO);
    g(ins, MODE);
    ins.local_get(WORD);
    g(ins, 2);
    let done = b.rt("fb_scope_body_done");
    ins.call(done);
    let _ = TMP;
}

/// `fb_scope_body_done(so, mode, word, repr) -> word`: the body ended; close the scope.
fn scope_body_done(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const F: u32 = 4;
    const CHILD: u32 = 5;
    const NEXT: u32 = 6;
    const W: u32 = 7;
    const FR: u32 = 8;
    const CAUSE: u32 = 9;
    const TMP: u32 = 10;
    let cdesc = plan(b).scope_close_desc;
    let table = plan(b).scope_close_table;
    ldl(ins, 0, S_FIBER);
    ins.local_set(F);
    ldl(ins, F, F_SCOPES);
    c(ins, 1);
    ins.i32_sub();
    ins.local_set(TMP);
    stl(ins, F, F_SCOPES, TMP);
    g(ins, F);
    g(ins, 0);
    let remove = b.rt("fb_pending_remove");
    ins.call(remove);
    ldl(ins, 0, S_FLAGS);
    c(ins, SF_CLOSED | SF_BODY_DONE);
    ins.i32_or();
    ins.local_set(TMP);
    stl(ins, 0, S_FLAGS, TMP);
    // The body's own failure.
    g(ins, 1);
    c(ins, 1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    c(ins, CAUSE_FAIL);
    ins.local_get(2);
    ins.i32_wrap_i64();
    let make = b.rt("fb_cause_new");
    ins.call(make);
    ins.local_set(CAUSE);
    g(ins, 0);
    g(ins, CAUSE);
    let failed = b.rt("fb_scope_failed");
    ins.call(failed);
    ins.else_();
    g(ins, 1);
    c(ins, 2);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.i32_wrap_i64();
    let intr = b.rt("fb_is_interrupted");
    ins.call(intr);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    c(ins, CAUSE_DIE);
    ins.local_get(2);
    ins.i32_wrap_i64();
    let make = b.rt("fb_cause_new");
    ins.call(make);
    ins.local_set(CAUSE);
    g(ins, 0);
    g(ins, CAUSE);
    let failed = b.rt("fb_scope_failed");
    ins.call(failed);
    ins.end();
    ins.end();
    ins.end();
    // Interrupt the children that are left.
    ldl(ins, 0, S_CHEAD);
    ins.local_set(CHILD);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, CHILD);
    ins.i32_eqz();
    ins.br_if(1);
    ldl(ins, CHILD, F_ITEM);
    ins.local_set(NEXT);
    g(ins, CHILD);
    c(ins, 0);
    let interrupt = b.rt("fb_interrupt_fiber");
    ins.call(interrupt);
    g(ins, NEXT);
    ins.local_set(CHILD);
    ins.br(0);
    ins.end();
    ins.end();
    ldl(ins, 0, S_CHEAD);
    ins.if_(BlockType::Empty);
    // Children remain: wait for the last one, holding interruption off.
    ldl(ins, F, F_HOLDS);
    c(ins, 1);
    ins.i32_add();
    ins.local_set(TMP);
    stl(ins, F, F_HOLDS, TMP);
    c(ins, WK_CLOSER);
    c(ins, 0);
    let wait_new = b.rt("fb_wait_new");
    ins.call(wait_new);
    ins.local_set(W);
    ldl(ins, W, W_FLAGS);
    c(ins, WF_CLOSING);
    ins.i32_or();
    ins.local_set(TMP);
    stl(ins, W, W_FLAGS, TMP);
    stl(ins, W, W_TARGET, 0);
    // The scope's field is raw (the descriptor does not count it): the suspension owns the
    // wait, and `fb_scope_child_ended` wakes it through this pointer.  A counted reference here
    // would leak with the (finalizer-less) scope record.
    stl(ins, 0, S_CLOSER, W);
    i32_const_cell(b, ins, |p| p.susp_wait);
    g(ins, W);
    ins.i32_store(mem(0, 4));
    // The frame holds the body's result until every child ended.
    c(ins, cdesc as i32);
    c(ins, table as i32);
    c(ins, 0);
    c(ins, 5);
    let new = b.rt("frame_new");
    ins.call(new);
    ins.local_set(FR);
    ins.local_get(FR);
    g(ins, 0);
    ins.i32_store(mem(HDR + 24, 4));
    stl(ins, FR, HDR + 32, 3);
    ins.local_get(FR);
    ins.local_get(2);
    ins.i64_store(mem(HDR + 40, 8));
    st_const(ins, FR, HDR + 48, 0);
    ins.local_get(FR);
    g(ins, 1);
    ins.i32_store(mem(HDR + 56, 4));
    g(ins, 3);
    c(ins, R_REF);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(FR);
    ins.local_get(2);
    ins.i32_wrap_i64();
    ins.i32_store(mem(HDR + 48, 4));
    ins.end();
    let push = b.rt("frame_push");
    g(ins, FR);
    ins.call(push);
    c(ins, 1);
    ins.global_set(G_SUSPENDING);
    ins.i64_const(0);
    ins.return_();
    ins.end();
    // No children left: the scope ends now.
    g(ins, 0);
    g(ins, 1);
    ins.local_get(2);
    g(ins, 3);
    let finish = b.rt("fb_scope_finish");
    ins.call(finish);
}

/// `fb_scope_finish(so, mode, word, repr) -> word`: the first failure, or the body's result.
fn scope_finish(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const FIRST: u32 = 4;
    const V: u32 = 5;
    ldl(ins, 0, S_FLAGS);
    c(ins, SF_ENDED);
    ins.i32_or();
    ins.local_set(FIRST);
    stl(ins, 0, S_FLAGS, FIRST);
    ldl(ins, 0, S_FIRST);
    ins.local_set(FIRST);
    g(ins, FIRST);
    ins.if_(BlockType::Empty);
    // A failure wins over how the body ended. The body's own input is released with the
    // reference-counting `drop`: a failure or defect/interruption value (modes 1 and 2) is a
    // reference, a value only when it is one.
    g(ins, 1);
    c(ins, 1);
    ins.i32_eq();
    g(ins, 1);
    c(ins, 2);
    ins.i32_eq();
    ins.i32_or();
    g(ins, 1);
    ins.i32_eqz();
    g(ins, 3);
    c(ins, R_REF);
    ins.i32_eq();
    ins.i32_and();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.i32_wrap_i64();
    call(b, ins, "drop");
    ins.end();
    ldl(ins, FIRST, HDR);
    ins.local_set(V);
    g(ins, V);
    call(b, ins, "dup");
    ldl(ins, FIRST, 4);
    c(ins, CAUSE_FAIL);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, V);
    ins.global_set(G_FAILURE);
    ins.else_();
    g(ins, V);
    ins.global_set(G_DEFECT);
    ins.end();
    ins.i64_const(0);
    ins.local_set(2);
    ins.else_();
    g(ins, 1);
    c(ins, 1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.i32_wrap_i64();
    ins.global_set(G_FAILURE);
    ins.i64_const(0);
    ins.local_set(2);
    ins.end();
    g(ins, 1);
    c(ins, 2);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.i32_wrap_i64();
    ins.global_set(G_DEFECT);
    ins.i64_const(0);
    ins.local_set(2);
    ins.end();
    ins.end();
    dropv(b, ins, 0);
    ins.local_get(2);
}

/// `fb_scope_body_resume(frame)`: the body's continuation returned; close the scope.
fn scope_body_resume(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const SO: u32 = 1;
    const REPR: u32 = 2;
    const TMP: u32 = 3;
    const RES: u32 = 4; // i64
    ins.local_get(0);
    ins.i32_load(mem(HDR + 24, 4));
    ins.local_set(SO);
    ins.local_get(0);
    ins.i32_load(mem(HDR + 32, 4));
    ins.local_set(REPR);
    st_const(ins, 0, HDR + 24, 0);
    dropv(b, ins, 0);
    g(ins, SO);
    sic(b, ins, |p| p.in_mode);
    ins.i32_load8_u(mem(0, 1));
    sic(b, ins, |p| p.in_word);
    ins.i64_load(mem(0, 8));
    g(ins, REPR);
    let done = b.rt("fb_scope_body_done");
    ins.call(done);
    ins.local_set(RES);
    write_out(b, ins, RES, REPR);
    let _ = TMP;
}

/// Writes the result of a custom resume function: the value, or the pending failure or defect.
fn write_out(b: &mut Builder<'_>, ins: &mut InstructionSink, word: u32, repr: u32) {
    ins.global_get(G_DEFECT);
    ins.if_(BlockType::Empty);
    sic(b, ins, |p| p.out_mode);
    c(ins, 2);
    ins.i32_store(mem(0, 4));
    sic(b, ins, |p| p.out_word);
    ins.global_get(G_DEFECT);
    ins.i32_store(mem(0, 4));
    c(ins, 0);
    ins.global_set(G_DEFECT);
    ins.return_();
    ins.end();
    ins.global_get(G_FAILURE);
    ins.if_(BlockType::Empty);
    sic(b, ins, |p| p.out_mode);
    c(ins, 1);
    ins.i32_store(mem(0, 4));
    sic(b, ins, |p| p.out_word);
    ins.global_get(G_FAILURE);
    ins.i32_store(mem(0, 4));
    c(ins, 0);
    ins.global_set(G_FAILURE);
    ins.return_();
    ins.end();
    sic(b, ins, |p| p.out_mode);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    sic(b, ins, |p| p.out_word);
    g(ins, word);
    ins.i64_store(mem(0, 8));
    let _ = repr;
}

/// `fb_scope_close_resume(frame)`: every child ended; finish the scope with the saved result.
fn scope_close_resume(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const SO: u32 = 1;
    const REPR: u32 = 2;
    const MODE: u32 = 3;
    const RESVAL: u32 = 4;
    const RES: u32 = 5;
    const F: u32 = 6;
    const WORD: u32 = 7; // i64
    ins.local_get(0);
    ins.i32_load(mem(HDR + 24, 4));
    ins.local_set(SO);
    ins.local_get(0);
    ins.i32_load(mem(HDR + 32, 4));
    ins.local_set(REPR);
    ins.local_get(0);
    ins.i32_load(mem(HDR + 56, 4));
    ins.local_set(MODE);
    ins.local_get(0);
    ins.i64_load(mem(HDR + 40, 8));
    ins.local_set(WORD);
    ins.local_get(0);
    ins.i32_load(mem(HDR + 48, 4));
    ins.local_set(RESVAL);
    st_const(ins, 0, HDR + 48, 0);
    st_const(ins, 0, HDR + 24, 0);
    dropv(b, ins, 0);
    ldl(ins, SO, S_FIBER);
    ins.local_set(F);
    ldl(ins, F, F_HOLDS);
    c(ins, 1);
    ins.i32_sub();
    ins.local_set(RES);
    stl(ins, F, F_HOLDS, RES);
    // The saved word is the reference when the body's value was one.
    g(ins, REPR);
    c(ins, R_REF);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, RESVAL);
    ins.i64_extend_i32_u();
    ins.local_set(WORD);
    ins.end();
    g(ins, SO);
    g(ins, MODE);
    ins.local_get(WORD);
    g(ins, REPR);
    let finish = b.rt("fb_scope_finish");
    ins.call(finish);
    ins.local_set(WORD);
    // The result is the next frame's input.
    ins.global_get(G_DEFECT);
    ins.if_(BlockType::Empty);
    sic(b, ins, |p| p.out_mode);
    c(ins, 2);
    ins.i32_store(mem(0, 4));
    sic(b, ins, |p| p.out_word);
    ins.global_get(G_DEFECT);
    ins.i32_store(mem(0, 4));
    c(ins, 0);
    ins.global_set(G_DEFECT);
    ins.return_();
    ins.end();
    ins.global_get(G_FAILURE);
    ins.if_(BlockType::Empty);
    sic(b, ins, |p| p.out_mode);
    c(ins, 1);
    ins.i32_store(mem(0, 4));
    sic(b, ins, |p| p.out_word);
    ins.global_get(G_FAILURE);
    ins.i32_store(mem(0, 4));
    c(ins, 0);
    ins.global_set(G_FAILURE);
    ins.return_();
    ins.end();
    sic(b, ins, |p| p.out_mode);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    sic(b, ins, |p| p.out_word);
    g(ins, WORD);
    ins.i64_store(mem(0, 8));
}

/// `fb_scope_failed(so, cause)`: a failure of the body or a child (spec 11.2). Takes the cause.
fn scope_failed(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const CHILD: u32 = 2;
    const NEXT: u32 = 3;
    const F: u32 = 4;
    const TMP: u32 = 5;
    ldl(ins, 0, S_FIRST);
    ins.if_(BlockType::Empty);
    // The first failure stays the result; this one becomes a note.
    ldl(ins, 0, S_FIRST);
    g(ins, 1);
    let note = b.rt("fb_note");
    ins.call(note);
    dropv(b, ins, 1);
    ins.return_();
    ins.end();
    stl(ins, 0, S_FIRST, 1);
    ldl(ins, 0, S_FLAGS);
    c(ins, SF_CLOSED);
    ins.i32_or();
    ins.local_set(TMP);
    stl(ins, 0, S_FLAGS, TMP);
    ldl(ins, 0, S_CHEAD);
    ins.local_set(CHILD);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, CHILD);
    ins.i32_eqz();
    ins.br_if(1);
    ldl(ins, CHILD, F_ITEM);
    ins.local_set(NEXT);
    g(ins, CHILD);
    c(ins, 0);
    let interrupt = b.rt("fb_interrupt_fiber");
    ins.call(interrupt);
    g(ins, NEXT);
    ins.local_set(CHILD);
    ins.br(0);
    ins.end();
    ins.end();
    ldl(ins, 0, S_FIBER);
    ins.local_set(F);
    ldl(ins, 0, S_FLAGS);
    c(ins, SF_BODY_DONE);
    ins.i32_and();
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    g(ins, F);
    g(ins, 0);
    let interrupt = b.rt("fb_interrupt_fiber");
    ins.call(interrupt);
    ins.end();
}

/// `fb_scope_child_ended(so, child)`: a child finished. Unlinks it, records its failure and wakes
/// a scope that waits for its last child.
fn scope_child_ended(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const PREV: u32 = 2;
    const NEXT: u32 = 3;
    const CAUSE: u32 = 4;
    const W: u32 = 5;
    ldl(ins, 1, F_ITEMPREV);
    ins.local_set(PREV);
    ldl(ins, 1, F_ITEM);
    ins.local_set(NEXT);
    g(ins, PREV);
    ins.if_(BlockType::Empty);
    stl(ins, PREV, F_ITEM, NEXT);
    ins.else_();
    stl(ins, 0, S_CHEAD, NEXT);
    ins.end();
    g(ins, NEXT);
    ins.if_(BlockType::Empty);
    stl(ins, NEXT, F_ITEMPREV, PREV);
    ins.else_();
    stl(ins, 0, S_CTAIL, PREV);
    ins.end();
    st_const(ins, 1, F_ITEM, 0);
    st_const(ins, 1, F_ITEMPREV, 0);
    ldl(ins, 1, F_CAUSE);
    ins.local_set(CAUSE);
    g(ins, CAUSE);
    ins.if_(BlockType::Empty);
    ldl(ins, CAUSE, 4);
    c(ins, CAUSE_INTERRUPT);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    g(ins, CAUSE);
    call(b, ins, "dup");
    g(ins, 0);
    g(ins, CAUSE);
    let failed = b.rt("fb_scope_failed");
    ins.call(failed);
    ins.end();
    ins.end();
    ldl(ins, 0, S_CHEAD);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ldl(ins, 0, S_CLOSER);
    ins.local_tee(W);
    ins.if_(BlockType::Empty);
    st_const(ins, 0, S_CLOSER, 0);
    g(ins, W);
    c(ins, 0);
    ins.i64_const(0);
    let wake = b.rt("fb_wait_wake");
    ins.call(wake);
    ins.end();
    ins.end();
}

/// `fb_fork(scope, f, thunk, repr) -> Fiber`: launch a child (spec 11.2).
fn fork(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const C: u32 = 4;
    const PREV: u32 = 5;
    const F: u32 = 6;
    let desc = plan(b).fiber_desc;
    let msg = plan(b).msg_scope_fork;
    ldl(ins, 0, S_FLAGS);
    c(ins, SF_CLOSED);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    raise_msg(b, ins, msg);
    dropv(b, ins, 1);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    // The child's record (also the user's handle).
    c(ins, F_SLOTS);
    c(ins, desc as i32);
    call(b, ins, "alloc");
    ins.local_set(C);
    ins.local_get(C);
    ins.i64_const(0);
    ins.i64_store(mem(F_RESWORD, 8));
    for off in [
        F_SCOPE, F_FLAGS, F_HOLDS, F_SCOPES, F_BUDGET, F_CAUSE, F_RESMODE, F_RESREF, F_RESVAL,
        F_WAITER, F_WHEAD, F_WTAIL, F_PHEAD, F_PTAIL, F_UHEAD, F_UTAIL, F_NEXTF, F_PREVF, F_ITEM,
        F_ITEMPREV, F_REPR,
    ] {
        st_const(ins, C, off, 0);
    }
    st_dup(b, ins, C, F_SCOPE, 0);
    ldl(ins, C, F_FLAGS);
    c(ins, FF_LISTED);
    ins.i32_or();
    ins.local_set(PREV);
    stl(ins, C, F_FLAGS, PREV);
    st_const(ins, C, F_BUDGET, BUDGET);
    stl(ins, C, F_REPR, 3);
    // The run's list of live fibers.
    i32_const_cell(b, ins, |p| p.ftail);
    ins.i32_load(mem(0, 4));
    ins.local_set(PREV);
    stl(ins, C, F_PREVF, PREV);
    g(ins, PREV);
    ins.if_(BlockType::Empty);
    stl(ins, PREV, F_NEXTF, C);
    ins.else_();
    i32_const_cell(b, ins, |p| p.fhead);
    g(ins, C);
    ins.i32_store(mem(0, 4));
    ins.end();
    i32_const_cell(b, ins, |p| p.ftail);
    g(ins, C);
    ins.i32_store(mem(0, 4));
    // The scope's list of children.
    ldl(ins, 0, S_CTAIL);
    ins.local_set(PREV);
    stl(ins, C, F_ITEMPREV, PREV);
    g(ins, PREV);
    ins.if_(BlockType::Empty);
    stl(ins, PREV, F_ITEM, C);
    ins.else_();
    stl(ins, 0, S_CHEAD, C);
    ins.end();
    stl(ins, 0, S_CTAIL, C);
    // The child starts when the parent next suspends (spec 11.2: fork is not a suspension point).
    c(ins, JK_BEGIN);
    g(ins, C);
    call(b, ins, "dup");
    g(ins, C);
    g(ins, 1);
    call(b, ins, "dup");
    g(ins, 1);
    c(ins, WE_MAIN);
    g(ins, 2);
    g(ins, 3);
    ins.i64_extend_i32_u();
    c(ins, 0);
    c(ins, 0);
    let push = b.rt("fb_queue_push");
    ins.call(push);
    g(ins, C);
    call(b, ins, "dup");
    g(ins, C);
    let _ = F;
}

/// `fb_join(h, repr) -> word`: wait for a child and return its value (spec 11.5).
fn join(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const W: u32 = 2;
    const MODE: u32 = 3;
    const WORD: u32 = 7; // i64
    let def_join = plan(b).def_join;
    call(b, ins, "fb_interrupt_here");
    ins.if_(BlockType::Empty);
    ins.i64_const(0);
    ins.return_();
    ins.end();
    ldl(ins, 0, F_SCOPE);
    ins.local_tee(MODE);
    ins.if_(BlockType::Empty);
    ldl(ins, MODE, S_FLAGS);
    c(ins, SF_ENDED);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    raise_msg(b, ins, def_join);
    ins.i64_const(0);
    ins.return_();
    ins.end();
    ins.end();
    ldl(ins, 0, F_FLAGS);
    c(ins, FF_ENDED);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    // Already finished: the result or the defect is immediate.
    c(ins, WK_FIBER);
    g(ins, 1);
    call(b, ins, "fb_wait_new");
    ins.local_set(W);
    st_const(ins, W, W_VERB, WV_JOIN);
    g(ins, W);
    g(ins, 0);
    let settle = b.rt("fb_watch_settle");
    ins.call(settle);
    ins.local_set(WORD);
    ins.local_set(MODE);
    dropv(b, ins, W);
    g(ins, MODE);
    ins.if_(BlockType::Empty);
    ins.local_get(WORD);
    ins.i32_wrap_i64();
    ins.global_set(G_DEFECT);
    ins.i64_const(0);
    ins.return_();
    ins.end();
    ins.local_get(WORD);
    ins.return_();
    ins.end();
    c(ins, WK_FIBER);
    g(ins, 1);
    call(b, ins, "fb_wait_new");
    ins.local_set(W);
    st_const(ins, W, W_VERB, WV_JOIN);
    g(ins, 0);
    g(ins, W);
    call(b, ins, "fb_watch_add");
    i32_const_cell(b, ins, |p| p.susp_wait);
    g(ins, W);
    ins.i32_store(mem(0, 4));
    c(ins, 1);
    ins.global_set(G_SUSPENDING);
    ins.i64_const(0);
}

/// `fb_await(h, repr) -> Exit`: wait for a child and return how it ended (spec 11.5).
fn await_(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const W: u32 = 2;
    const MODE: u32 = 3;
    const WORD: u32 = 7; // i64
    let def_await = plan(b).def_await;
    call(b, ins, "fb_interrupt_here");
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ldl(ins, 0, F_SCOPE);
    ins.local_tee(MODE);
    ins.if_(BlockType::Empty);
    ldl(ins, MODE, S_FLAGS);
    c(ins, SF_ENDED);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    raise_msg(b, ins, def_await);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.end();
    c(ins, WK_FIBER);
    g(ins, 1);
    call(b, ins, "fb_wait_new");
    ins.local_set(W);
    st_const(ins, W, W_VERB, WV_AWAIT);
    ldl(ins, 0, F_FLAGS);
    c(ins, FF_ENDED);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    g(ins, W);
    g(ins, 0);
    let settle = b.rt("fb_watch_settle");
    ins.call(settle);
    ins.local_set(WORD); // i64 on top
    ins.local_set(MODE);
    dropv(b, ins, W);
    g(ins, WORD);
    ins.i32_wrap_i64();
    ins.return_();
    ins.end();
    g(ins, 0);
    g(ins, W);
    call(b, ins, "fb_watch_add");
    i32_const_cell(b, ins, |p| p.susp_wait);
    g(ins, W);
    ins.i32_store(mem(0, 4));
    c(ins, 1);
    ins.global_set(G_SUSPENDING);
    ins.i32_const(0);
}

/// `fb_interrupt(h)`: interrupt a child and wait until its unwinding finished (spec 11.4).
fn interrupt(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const W: u32 = 2;
    const F: u32 = 3;
    let def = plan(b).def_interrupt;
    call(b, ins, "fb_interrupt_here");
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ldl(ins, 0, F_SCOPE);
    ins.local_tee(F);
    ins.if_(BlockType::Empty);
    ldl(ins, F, S_FLAGS);
    c(ins, SF_ENDED);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    raise_msg(b, ins, def);
    ins.return_();
    ins.end();
    ins.end();
    g(ins, 0);
    c(ins, 0);
    let ifib = b.rt("fb_interrupt_fiber");
    ins.call(ifib);
    i32_const_cell(b, ins, |p| p.cur_fiber);
    ins.i32_load(mem(0, 4));
    g(ins, 0);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ldl(ins, 0, F_HOLDS);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    g(ins, 0);
    let deliver = b.rt("fb_deliver");
    ins.call(deliver);
    ins.global_set(G_DEFECT);
    ins.end();
    ins.return_();
    ins.end();
    ldl(ins, 0, F_FLAGS);
    c(ins, FF_ENDED);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    c(ins, WK_FIBER);
    c(ins, 0);
    call(b, ins, "fb_wait_new");
    ins.local_set(W);
    st_const(ins, W, W_VERB, WV_INTERRUPT);
    g(ins, 0);
    g(ins, W);
    call(b, ins, "fb_watch_add");
    i32_const_cell(b, ins, |p| p.susp_wait);
    g(ins, W);
    ins.i32_store(mem(0, 4));
    c(ins, 1);
    ins.global_set(G_SUSPENDING);
}

/// `fb_deferred(scope) -> Deferred`: a new Deferred belonging to the scope (spec 11.7).
fn deferred(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const D: u32 = 2;
    let desc = plan(b).deferred_desc;
    let msg = plan(b).msg_scope_used;
    ldl(ins, 0, S_FLAGS);
    c(ins, SF_CLOSED);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    raise_msg(b, ins, msg);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    c(ins, D_SLOTS);
    c(ins, desc as i32);
    call(b, ins, "alloc");
    ins.local_set(D);
    st_dup(b, ins, D, D_SCOPE, 0);
    st_const(ins, D, D_FULL, 0);
    st_const(ins, D, D_ISREF, 0);
    st_const(ins, D, D_VAL, 0);
    st_const(ins, D, D_WHEAD, 0);
    st_const(ins, D, D_WTAIL, 0);
    ins.local_get(D);
    ins.i64_const(0);
    ins.i64_store(mem(D_WORD, 8));
    g(ins, D);
}

/// `fb_complete(d, word, repr) -> Bool`: store the value if empty (spec 11.7).
fn complete(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const W: u32 = 4;
    const NEXT: u32 = 5;
    const V: u32 = 6;
    const TMP: u32 = 7;
    const WORD: u32 = 8; // i64
    let msg = plan(b).msg_deferred_complete;
    ins.local_get(1);
    ins.local_set(WORD);
    ldl(ins, 0, D_SCOPE);
    ins.local_set(TMP);
    ldl(ins, TMP, S_FLAGS);
    c(ins, SF_ENDED);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    raise_msg(b, ins, msg);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ldl(ins, 0, D_FULL);
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    st_const(ins, 0, D_FULL, 1);
    g(ins, 2);
    c(ins, R_REF);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    st_const(ins, 0, D_ISREF, 1);
    // The Deferred keeps its own reference: the caller drops the value it passed.
    ins.local_get(WORD);
    ins.i32_wrap_i64();
    call(b, ins, "dup");
    ins.local_get(0);
    ins.local_get(WORD);
    ins.i32_wrap_i64();
    ins.i32_store(mem(D_VAL, 4));
    ins.else_();
    st_const(ins, 0, D_ISREF, 0);
    ins.end();
    ins.local_get(0);
    ins.local_get(WORD);
    ins.i64_store(mem(D_WORD, 8));
    ldl(ins, 0, D_WHEAD);
    ins.local_set(W);
    st_const(ins, 0, D_WHEAD, 0);
    st_const(ins, 0, D_WTAIL, 0);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, W);
    ins.i32_eqz();
    ins.br_if(1);
    ldl(ins, W, W_TNEXT);
    ins.local_set(NEXT);
    st_const(ins, W, W_TNEXT, 0);
    st_const(ins, W, W_TPREV, 0);
    st_const(ins, W, W_TARGET, 0);
    ldl(ins, W, W_FLAGS);
    c(ins, -1 - WF_TARGET);
    ins.i32_and();
    ins.local_set(TMP);
    stl(ins, W, W_FLAGS, TMP);
    ldl(ins, W, W_REPR);
    c(ins, R_REF);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ldl(ins, 0, D_VAL);
    call(b, ins, "dup");
    ldl(ins, 0, D_VAL);
    ins.local_set(V);
    ins.else_();
    ins.local_get(WORD);
    ins.i32_wrap_i64();
    ins.local_set(V);
    ins.end();
    g(ins, W);
    c(ins, 0);
    g(ins, V);
    ins.i64_extend_i32_u();
    let wake = b.rt("fb_wait_wake");
    ins.call(wake);
    g(ins, NEXT);
    ins.local_set(W);
    ins.br(0);
    ins.end();
    ins.end();
    c(ins, 1);
}

/// `fb_deferred_await(d, repr) -> word`: wait until a value is stored (spec 11.7).
fn deferred_await(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const W: u32 = 2;
    let msg = plan(b).msg_deferred_await;
    call(b, ins, "fb_interrupt_here");
    ins.if_(BlockType::Empty);
    ins.i64_const(0);
    ins.return_();
    ins.end();
    ldl(ins, 0, D_SCOPE);
    ins.local_set(W);
    ldl(ins, W, S_FLAGS);
    c(ins, SF_ENDED);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    raise_msg(b, ins, msg);
    ins.i64_const(0);
    ins.return_();
    ins.end();
    ldl(ins, 0, D_FULL);
    ins.if_(BlockType::Empty);
    g(ins, 1);
    c(ins, R_REF);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ldl(ins, 0, D_VAL);
    call(b, ins, "dup");
    ldl(ins, 0, D_VAL);
    ins.i64_extend_i32_u();
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i64_load(mem(D_WORD, 8));
    ins.return_();
    ins.end();
    c(ins, WK_DEFERRED);
    g(ins, 1);
    call(b, ins, "fb_wait_new");
    ins.local_set(W);
    g(ins, 0);
    g(ins, W);
    call(b, ins, "fb_watch_add");
    i32_const_cell(b, ins, |p| p.susp_wait);
    g(ins, W);
    ins.i32_store(mem(0, 4));
    c(ins, 1);
    ins.global_set(G_SUSPENDING);
    ins.i64_const(0);
}

/// `fb_poll(d, repr, some, none) -> Option`: the stored value without waiting (spec 11.7).
fn poll(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const E: u32 = 4;
    let msg = plan(b).msg_deferred_poll;
    ldl(ins, 0, D_SCOPE);
    ins.local_set(E);
    ldl(ins, E, S_FLAGS);
    c(ins, SF_ENDED);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    raise_msg(b, ins, msg);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ldl(ins, 0, D_FULL);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    g(ins, 3);
    ins.return_();
    ins.end();
    c(ins, 1);
    g(ins, 2);
    call(b, ins, "alloc");
    ins.local_set(E);
    g(ins, 1);
    c(ins, R_REF);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ldl(ins, 0, D_VAL);
    call(b, ins, "dup");
    ins.local_get(E);
    ldl(ins, 0, D_VAL);
    ins.i32_store(mem(HDR, 4));
    ins.else_();
    ins.local_get(E);
    ins.local_get(0);
    ins.i64_load(mem(D_WORD, 8));
    ins.i64_store(mem(HDR, 8));
    ins.end();
    g(ins, E);
}

/// `fb_yield(perform)`: the fairness budget (spec 11.1). A `perform` yield also delivers a pending
/// interruption (spec 11.4).
fn yield_(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const F: u32 = 1;
    const W: u32 = 2;
    i32_const_cell(b, ins, |p| p.cur_fiber);
    ins.i32_load(mem(0, 4));
    ins.local_tee(F);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    let sync = super::sched::cell(b, |p| p.sync_depth);
    ins.i32_const(sync as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ldl(ins, F, F_BUDGET);
    c(ins, 1);
    ins.i32_sub();
    ins.local_set(W);
    stl(ins, F, F_BUDGET, W);
    ldl(ins, F, F_BUDGET);
    ins.i32_eqz();
    ins.i32_eqz();
    // budget > 0: only a `perform` yield may deliver an interruption.
    ins.if_(BlockType::Empty);
    g(ins, 0);
    ins.if_(BlockType::Empty);
    call(b, ins, "fb_interrupt_here");
    ins.drop();
    ins.end();
    ins.return_();
    ins.end();
    i32_const_cell(b, ins, |p| p.cur_main);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    g(ins, F);
    let interrupting = b.rt("fb_interrupting");
    ins.call(interrupting);
    ins.if_(BlockType::Empty);
    g(ins, F);
    let deliver = b.rt("fb_deliver");
    ins.call(deliver);
    ins.global_set(G_DEFECT);
    ins.return_();
    ins.end();
    ins.end();
    st_const(ins, F, F_BUDGET, BUDGET);
    c(ins, WK_PAUSE);
    c(ins, 0);
    call(b, ins, "fb_wait_new");
    ins.local_set(W);
    i32_const_cell(b, ins, |p| p.susp_wait);
    g(ins, W);
    ins.i32_store(mem(0, 4));
    c(ins, 1);
    ins.global_set(G_SUSPENDING);
}

/// `fb_hold()` / `fb_unhold()`: a release that may suspend is not interrupted (spec 11.4).
fn hold(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const F: u32 = 0;
    i32_const_cell(b, ins, |p| p.cur_fiber);
    ins.i32_load(mem(0, 4));
    ins.local_tee(F);
    ins.if_(BlockType::Empty);
    // `F` stays the fiber in this branch: the count is read, changed and stored back through it.
    g(ins, F);
    g(ins, F);
    ins.i32_load(mem(F_HOLDS, 4));
    c(ins, 1);
    ins.i32_add();
    ins.i32_store(mem(F_HOLDS, 4));
    ins.end();
}

fn unhold(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const F: u32 = 0;
    i32_const_cell(b, ins, |p| p.cur_fiber);
    ins.i32_load(mem(0, 4));
    ins.local_tee(F);
    ins.if_(BlockType::Empty);
    g(ins, F);
    g(ins, F);
    ins.i32_load(mem(F_HOLDS, 4));
    c(ins, 1);
    ins.i32_sub();
    ins.i32_store(mem(F_HOLDS, 4));
    ins.end();
}

/// `fb_interrupt_here() -> i32`: deliver a pending interruption at a suspension point of the
/// current fiber's main continuation (spec 11.4).
fn interrupt_here(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const F: u32 = 0;
    i32_const_cell(b, ins, |p| p.cur_fiber);
    ins.i32_load(mem(0, 4));
    ins.local_tee(F);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    i32_const_cell(b, ins, |p| p.cur_main);
    ins.i32_load(mem(0, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    let sync = super::sched::cell(b, |p| p.sync_depth);
    ins.i32_const(sync as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    g(ins, F);
    let interrupting = b.rt("fb_interrupting");
    ins.call(interrupting);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    g(ins, F);
    let deliver = b.rt("fb_deliver");
    ins.call(deliver);
    ins.global_set(G_DEFECT);
    ins.i32_const(1);
}

/// `fb_interrupt_fiber(f, target)`: ask a fiber to unwind (spec 11.4). `target` is 0 for the whole
/// fiber, or a scope whose body runs on it.
fn interrupt_fiber(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const W: u32 = 2;
    const P: u32 = 3;
    const CHAIN: u32 = 5;
    const END: u32 = 6;
    ldl(ins, 0, F_FLAGS);
    c(ins, FF_ENDED);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ldl(ins, 0, F_FLAGS);
    c(ins, FF_UNWINDING);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    g(ins, 1);
    ins.if_(BlockType::Result(ValType::I32));
    g(ins, 0);
    g(ins, 1);
    let has = b.rt("fb_pending_has");
    ins.call(has);
    g(ins, 0);
    g(ins, 1);
    let uh = b.rt("fb_unwinds_has");
    ins.call(uh);
    ins.i32_or();
    ins.else_();
    g(ins, 0);
    c(ins, 0);
    let has = b.rt("fb_pending_has");
    ins.call(has);
    ins.end();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    g(ins, 0);
    g(ins, 1);
    let push = b.rt("fb_pending_push");
    ins.call(push);
    ldl(ins, 0, F_HOLDS);
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ldl(ins, 0, F_WAITER);
    ins.local_tee(W);
    ins.if_(BlockType::Empty);
    ins.else_();
    ins.return_();
    ins.end();
    // Dropping a caller also unwinds the implementation it waits for, and any implementation that
    // one waits for in turn, innermost first (spec 14.5; the C runtime's `em_drop_wait`).
    g(ins, W);
    let drop_wait = b.rt("fb_drop_wait");
    ins.call(drop_wait);
    // Then the fiber's own continuation, with the interruption when the job runs.
    ldl(ins, W, W_CHAIN);
    ins.local_set(CHAIN);
    st_const(ins, W, W_CHAIN, 0);
    ldl(ins, W, W_END);
    ins.local_set(END);
    g(ins, 0);
    ins.local_set(P);
    g(ins, P);
    call(b, ins, "dup");
    dropv(b, ins, W);
    c(ins, JK_INTERRUPT);
    g(ins, P);
    g(ins, CHAIN);
    g(ins, END);
    c(ins, 0);
    ins.i64_const(0);
    c(ins, 0);
    c(ins, 0);
    let push = b.rt("fb_queue_push");
    ins.call(push);
}

/// `fb_note(first, other)`: add `note: another fiber also failed: ...` to the first cause (spec
/// 11.2). Both causes are borrowed.
fn note(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const SHOWN: u32 = 2;
    const NOTE: u32 = 3;
    const D: u32 = 4;
    const TMP: u32 = 5;
    let prefix = plan(b).note_other;
    let newline = b.string_literal(b"\n");
    g(ins, 1);
    let string = b.rt("fb_cause_string");
    ins.call(string);
    ins.local_set(SHOWN);
    c(ins, prefix as i32);
    g(ins, SHOWN);
    let append = b.rt("str_append");
    ins.call(append);
    ins.local_set(NOTE);
    dropv(b, ins, SHOWN);
    ldl(ins, 0, 4);
    c(ins, CAUSE_FAIL);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    // The error slot of the first cause (the note is keyed by the error it becomes).
    g(ins, 0);
    c(ins, HDR as i32);
    ins.i32_add();
    g(ins, NOTE);
    let add = b.rt("fb_notes_add");
    ins.call(add);
    ins.else_();
    ldl(ins, 0, 4);
    c(ins, CAUSE_DIE);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ldl(ins, 0, HDR);
    ins.local_set(D);
    g(ins, D);
    c(ins, newline as i32);
    ins.call(append);
    ins.local_set(TMP);
    g(ins, TMP);
    g(ins, NOTE);
    ins.call(append);
    ins.local_set(D);
    // The defect keeps its markers (`aux`).
    g(ins, D);
    ldl(ins, 0, HDR);
    ins.i32_load(mem(4, 4));
    ins.i32_store(mem(4, 4));
    // Drop the old defect and the intermediate string.
    g(ins, TMP);
    call(b, ins, "drop");
    ldl(ins, 0, HDR);
    call(b, ins, "drop");
    stl(ins, 0, HDR, D);
    ins.end();
    ins.end();
    dropv(b, ins, NOTE);
}

/// `fb_notes_of(error) -> String`: the notes of a failure (a new reference), or 0.
fn notes_of(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const E: u32 = 1;
    let head = plan(b).notes;
    // A program without the fiber runtime has no notes cell; 0 is the scratch area, not a list.
    // The check is compile-time so the emitted function never reads address 0.
    if head == 0 {
        c(ins, 0);
        ins.return_();
        return;
    }
    ins.i32_const(head as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(E);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, E);
    ins.i32_eqz();
    ins.br_if(1);
    ldl(ins, E, HDR);
    g(ins, 0);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ldl(ins, E, HDR + 8);
    call(b, ins, "dup");
    ldl(ins, E, HDR + 8);
    ins.return_();
    ins.end();
    ldl(ins, E, HDR + 16);
    ins.local_set(E);
    ins.br(0);
    ins.end();
    ins.end();
    ins.i32_const(0);
}

/// `fb_notes_add(slot, note)`: attach a note to the error held in `slot` (a cause's error slot).
/// `slot` is updated to a copy when the error is static (a constructor with `rc == 0`), so the
/// notes stay with this failure; `note` is borrowed.
fn notes_add(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const E: u32 = 2;
    const COPY: u32 = 3;
    const N: u32 = 4;
    const LINE: u32 = 5;
    const NEXT: u32 = 6;
    const I: u32 = 7;
    const D: u32 = 8;
    let head = plan(b).notes;
    let desc = plan(b).note_desc;
    let append = b.rt("str_append");
    // A static error is copied first, and the cause's slot is updated, so later notes of this
    // failure find the copy (the C runtime's `em_notes_add`).
    ldl(ins, 0, 0);
    ins.local_set(E);
    ldl(ins, E, 0);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ldl(ins, E, 8);
    ins.local_set(D);
    ldl(ins, D, DESC_NSLOTS);
    ins.local_set(N);
    g(ins, N);
    g(ins, D);
    call(b, ins, "alloc");
    ins.local_set(COPY);
    c(ins, 0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, I);
    g(ins, N);
    ins.i32_ge_u();
    ins.br_if(1);
    g(ins, COPY);
    c(ins, HDR as i32);
    ins.i32_add();
    g(ins, I);
    c(ins, 8);
    ins.i32_mul();
    ins.i32_add();
    g(ins, E);
    c(ins, HDR as i32);
    ins.i32_add();
    g(ins, I);
    c(ins, 8);
    ins.i32_mul();
    ins.i32_add();
    ins.i64_load(mem(0, 8));
    ins.i64_store(mem(0, 8));
    ldl(ins, D, DESC_REFS);
    g(ins, I);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.if_(BlockType::Empty);
    g(ins, E);
    c(ins, HDR as i32);
    ins.i32_add();
    g(ins, I);
    c(ins, 8);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    call(b, ins, "dup");
    ins.end();
    g(ins, I);
    c(ins, 1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(0);
    g(ins, COPY);
    ins.i32_store(mem(0, 4));
    g(ins, COPY);
    ins.local_set(E);
    ins.end();
    // An existing entry: append a line, releasing the note string it replaces.
    ins.i32_const(head as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(NEXT);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, NEXT);
    ins.i32_eqz();
    ins.br_if(1);
    ldl(ins, NEXT, HDR);
    g(ins, E);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ldl(ins, NEXT, HDR + 8);
    let nl = b.string_literal(b"\n");
    c(ins, nl as i32);
    ins.call(append);
    ins.local_set(LINE);
    g(ins, LINE);
    g(ins, 1);
    ins.call(append);
    ins.local_set(I);
    g(ins, LINE);
    call(b, ins, "drop");
    ldl(ins, NEXT, HDR + 8);
    call(b, ins, "drop");
    stl(ins, NEXT, HDR + 8, I);
    ins.return_();
    ins.end();
    ldl(ins, NEXT, HDR + 16);
    ins.local_set(NEXT);
    ins.br(0);
    ins.end();
    ins.end();
    // Push the entry at the head of the list.
    c(ins, 3);
    c(ins, desc as i32);
    call(b, ins, "alloc");
    ins.local_set(COPY);
    st_dup(b, ins, COPY, HDR, E);
    st_dup(b, ins, COPY, HDR + 8, 1);
    ins.i32_const(head as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(NEXT);
    stl(ins, COPY, HDR + 16, NEXT);
    ins.i32_const(head as i32);
    g(ins, COPY);
    ins.i32_store(mem(0, 4));
}

/// `fb_notes_clear()`: release every note entry at the end of a run.
fn notes_clear(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const E: u32 = 0;
    const NEXT: u32 = 1;
    let head = plan(b).notes;
    // No fiber runtime: nothing was ever recorded, and 0 is the scratch area. Emit no read.
    if head == 0 {
        ins.return_();
        return;
    }
    ins.i32_const(head as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(E);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, E);
    ins.i32_eqz();
    ins.br_if(1);
    ldl(ins, E, HDR + 16);
    ins.local_set(NEXT);
    dropv(b, ins, E);
    g(ins, NEXT);
    ins.local_set(E);
    ins.br(0);
    ins.end();
    ins.end();
    ins.i32_const(head as i32);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    let _ = (E, NEXT);
}

// ---- the call thunks ----
//
// A forked function and a scope body have the value's own representation, but the queued job and
// the scope frame have a fixed shape. One thunk per representation calls the closure and turns its
// result into a word; the reference-counted reference is already the word.

fn thunk(b: &mut Builder<'_>, ins: &mut InstructionSink, ret: Repr, args: usize) {
    let mut params = vec![Repr::Ref];
    if args == 1 {
        params.push(Repr::Ref);
    }
    let ty = b.call_ty(&params, ret);
    g(ins, 0);
    if args == 1 {
        g(ins, 1);
    }
    g(ins, 0);
    ins.i32_load(mem(HDR, 4));
    ins.call_indirect(0, ty);
    match ret {
        Repr::I32 | Repr::Ref => {
            ins.i64_extend_i32_u();
        }
        Repr::I64 => {}
        Repr::F64 => {
            ins.i64_reinterpret_f64();
        }
        Repr::Void => {
            ins.i64_const(0);
        }
    }
}

fn call0(b: &mut Builder<'_>, ins: &mut InstructionSink, ret: Repr) {
    thunk(b, ins, ret, 0);
}

fn call1(b: &mut Builder<'_>, ins: &mut InstructionSink, ret: Repr) {
    thunk(b, ins, ret, 1);
}

// ---- the run ----

/// `fb_root_begin()`: make the run's root fiber the current one.
fn root_begin(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const F: u32 = 1;
    let desc = plan(b).fiber_desc;
    // An orphan defect left by the previous run was not the run's result (it only replaces an
    // interruption): release it before the cells are reset.
    i32_const_cell(b, ins, |p| p.orphan);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    i32_const_cell(b, ins, |p| p.orphan);
    ins.i32_load(mem(0, 4));
    call(b, ins, "drop");
    ins.end();
    // Reset the run's state (a test run follows the previous one).
    for f in [
        |p: &Plan| p.fhead,
        |p: &Plan| p.ftail,
        |p: &Plan| p.whead,
        |p: &Plan| p.wtail,
        |p: &Plan| p.qhead,
        |p: &Plan| p.qtail,
        |p: &Plan| p.lhead,
        |p: &Plan| p.ltail,
        |p: &Plan| p.host_head,
        |p: &Plan| p.host_tail,
        |p: &Plan| p.test_head,
        |p: &Plan| p.test_tail,
        |p: &Plan| p.test_seq,
        |p: &Plan| p.run_mode,
        |p: &Plan| p.run_word,
        |p: &Plan| p.live_waits,
        |p: &Plan| p.orphan,
        |p: &Plan| p.pauses,
    ] {
        i32_const_cell(b, ins, f);
        c(ins, 0);
        ins.i32_store(mem(0, 4));
    }
    i32_const_cell(b, ins, |p| p.run_over);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    // The notes belong to the run's result, which is reported after the run: drop them when the
    // next run starts.
    let clear = b.rt("fb_notes_clear");
    ins.call(clear);
    c(ins, F_SLOTS);
    c(ins, desc as i32);
    call(b, ins, "alloc");
    ins.local_set(F);
    ins.local_get(F);
    ins.i64_const(0);
    ins.i64_store(mem(F_RESWORD, 8));
    for off in [
        F_SCOPE, F_FLAGS, F_HOLDS, F_SCOPES, F_BUDGET, F_CAUSE, F_RESMODE, F_RESREF, F_RESVAL,
        F_WAITER, F_WHEAD, F_WTAIL, F_PHEAD, F_PTAIL, F_UHEAD, F_UTAIL, F_NEXTF, F_PREVF, F_ITEM,
        F_ITEMPREV, F_REPR,
    ] {
        st_const(ins, F, off, 0);
    }
    st_const(ins, F, F_FLAGS, FF_LISTED);
    st_const(ins, F, F_BUDGET, BUDGET);
    stl(ins, F, F_REPR, 0); // the root's result is not observed (the entry released it)
    i32_const_cell(b, ins, |p| p.fhead);
    g(ins, F);
    ins.i32_store(mem(0, 4));
    i32_const_cell(b, ins, |p| p.ftail);
    g(ins, F);
    ins.i32_store(mem(0, 4));
    i32_const_cell(b, ins, |p| p.cur_fiber);
    g(ins, F);
    ins.i32_store(mem(0, 4));
    i32_const_cell(b, ins, |p| p.cur_main);
    c(ins, 1);
    ins.i32_store(mem(0, 4));
}

/// `fb_root_run()`: after main or a test returned, finish the run.
fn root_run(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const F: u32 = 0;
    const CHAIN: u32 = 1;
    const W: u32 = 2;
    const MODE: u32 = 3;
    i32_const_cell(b, ins, |p| p.cur_fiber);
    ins.i32_load(mem(0, 4));
    ins.local_set(F);
    ins.global_get(G_SUSPENDING);
    ins.if_(BlockType::Empty);
    c(ins, 0);
    let take = b.rt("frame_take");
    ins.call(take);
    ins.local_set(CHAIN);
    i32_const_cell(b, ins, |p| p.susp_wait);
    ins.i32_load(mem(0, 4));
    ins.local_set(W);
    i32_const_cell(b, ins, |p| p.susp_wait);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    g(ins, W);
    ins.if_(BlockType::Empty);
    g(ins, W);
    g(ins, CHAIN);
    c(ins, WE_MAIN);
    c(ins, 0);
    let suspended = b.rt("fb_suspended");
    ins.call(suspended);
    ins.else_();
    dropv(b, ins, CHAIN);
    ins.end();
    ins.else_();
    c(ins, 0);
    ins.local_set(W);
    c(ins, 0);
    ins.local_set(MODE);
    ins.global_get(G_DEFECT);
    ins.if_(BlockType::Empty);
    ins.global_get(G_DEFECT);
    ins.local_set(W);
    c(ins, 0);
    ins.global_set(G_DEFECT);
    c(ins, 2);
    ins.local_set(MODE);
    ins.else_();
    ins.global_get(G_FAILURE);
    ins.if_(BlockType::Empty);
    ins.global_get(G_FAILURE);
    ins.local_set(W);
    c(ins, 0);
    ins.global_set(G_FAILURE);
    c(ins, 1);
    ins.local_set(MODE);
    ins.end();
    ins.end();
    g(ins, F);
    g(ins, MODE);
    ins.local_get(W);
    ins.i64_extend_i32_u();
    let finish = b.rt("fb_finish");
    ins.call(finish);
    ins.end();
    // Run the queue; wait on the host or the TestClock only when nothing is runnable.
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    let drain = b.rt("fb_drain");
    ins.call(drain);
    i32_const_cell(b, ins, |p| p.run_over);
    ins.i32_load(mem(0, 4));
    ins.br_if(1);
    // A job that waits for the event loop runs after the queue is empty; the C runtime moves one
    // per turn, so the jobs it adds run before the next.
    i32_const_cell(b, ins, |p| p.lhead);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    let later = b.rt("fb_later_move");
    ins.call(later);
    ins.br(1);
    ins.end();
    let has = b.rt("fb_has_waits");
    ins.call(has);
    ins.i32_eqz();
    ins.br_if(1);
    let idle = b.rt("fb_idle");
    ins.call(idle);
    ins.if_(BlockType::Empty);
    // Inside the `if`, depth 1 is the loop: a queued wake goes back to the drain, and only an
    // empty event loop falls through to the interruption of the run.
    ins.br(1);
    ins.end();
    let root_interrupt = b.rt("fb_root_interrupt");
    ins.call(root_interrupt);
    let drain = b.rt("fb_drain");
    ins.call(drain);
    // The stall unwinds as interrupted (spec 14.5 cuts off running releases).  The next turn
    // re-checks the later queue and the live waits before the run ends; the C runtime checks
    // `later`/`em_loop_busy` at the same point and ends when both are empty.
    ins.br(1);
    ins.end();
    ins.end();
    let finish_run = b.rt("fb_finish_run");
    ins.call(finish_run);
}

/// `fb_finish_run()`: release the waits and fibers left when the run ended (spec 14.5: their withs
/// are not released) and start the next run from a clean slate.
fn finish_run(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const W: u32 = 0;
    const NEXT: u32 = 1;
    const F: u32 = 2;
    // Put the run's result back: the strands that finished after the run ended cleared it from
    // the pending globals so they could not take it with them.
    let drop = b.rt("drop");
    i32_const_cell(b, ins, |p| p.run_mode);
    ins.i32_load(mem(0, 4));
    c(ins, 1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.global_get(G_FAILURE);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.global_get(G_FAILURE);
    ins.call(drop);
    i32_const_cell(b, ins, |p| p.run_word);
    ins.i32_load(mem(0, 4));
    ins.global_set(G_FAILURE);
    ins.else_();
    i32_const_cell(b, ins, |p| p.run_word);
    ins.i32_load(mem(0, 4));
    ins.call(drop);
    ins.end();
    ins.else_();
    i32_const_cell(b, ins, |p| p.run_mode);
    ins.i32_load(mem(0, 4));
    c(ins, 2);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.global_get(G_DEFECT);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.global_get(G_DEFECT);
    ins.call(drop);
    i32_const_cell(b, ins, |p| p.run_word);
    ins.i32_load(mem(0, 4));
    ins.global_set(G_DEFECT);
    ins.else_();
    i32_const_cell(b, ins, |p| p.run_word);
    ins.i32_load(mem(0, 4));
    ins.call(drop);
    ins.end();
    ins.end();
    ins.end();
    i32_const_cell(b, ins, |p| p.run_word);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    // A defect in an operation implementation's release wins over the run's interruption.
    let intr = b.rt("fb_is_interrupted");
    ins.global_get(G_DEFECT);
    ins.call(intr);
    ins.if_(BlockType::Empty);
    i32_const_cell(b, ins, |p| p.orphan);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.global_get(G_DEFECT);
    ins.call(rt_drop(b));
    i32_const_cell(b, ins, |p| p.orphan);
    ins.i32_load(mem(0, 4));
    ins.global_set(G_DEFECT);
    i32_const_cell(b, ins, |p| p.orphan);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    ins.end();
    ins.end();
    // The waits whose continuations will not run.
    i32_const_cell(b, ins, |p| p.whead);
    ins.i32_load(mem(0, 4));
    ins.local_set(W);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, W);
    ins.i32_eqz();
    ins.br_if(1);
    ldl(ins, W, W_NEXT);
    ins.local_set(NEXT);
    // Take it out of every list it is in (the run's, its target's, the host or TestClock
    // sleepers'): the heads and tails must not keep pointing at a released wait.
    g(ins, W);
    call(b, ins, "fb_unlink_wait");
    dropv(b, ins, W);
    g(ins, NEXT);
    ins.local_set(W);
    ins.br(0);
    ins.end();
    ins.end();
    i32_const_cell(b, ins, |p| p.whead);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    i32_const_cell(b, ins, |p| p.wtail);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    i32_const_cell(b, ins, |p| p.live_waits);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    // The fibers still in the run's list.
    i32_const_cell(b, ins, |p| p.fhead);
    ins.i32_load(mem(0, 4));
    ins.local_set(F);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, F);
    ins.i32_eqz();
    ins.br_if(1);
    ldl(ins, F, F_NEXTF);
    ins.local_set(NEXT);
    st_const(ins, F, F_NEXTF, 0);
    st_const(ins, F, F_PREVF, 0);
    g(ins, F);
    call(b, ins, "fb_pending_clear");
    dropv(b, ins, F);
    g(ins, NEXT);
    ins.local_set(F);
    ins.br(0);
    ins.end();
    ins.end();
    i32_const_cell(b, ins, |p| p.fhead);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    i32_const_cell(b, ins, |p| p.ftail);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    // The jobs a "failed after resume" left, queued or waiting for the event loop.
    for list in [|p: &Plan| p.qhead, |p: &Plan| p.lhead] {
        i32_const_cell(b, ins, list);
        ins.i32_load(mem(0, 4));
        ins.local_set(W);
        ins.block(BlockType::Empty);
        ins.loop_(BlockType::Empty);
        g(ins, W);
        ins.i32_eqz();
        ins.br_if(1);
        ldl(ins, W, J_NEXT);
        ins.local_set(NEXT);
        dropv(b, ins, W);
        g(ins, NEXT);
        ins.local_set(W);
        ins.br(0);
        ins.end();
        ins.end();
    }
    for list in [
        |p: &Plan| p.qhead,
        |p: &Plan| p.qtail,
        |p: &Plan| p.lhead,
        |p: &Plan| p.ltail,
    ] {
        i32_const_cell(b, ins, list);
        c(ins, 0);
        ins.i32_store(mem(0, 4));
    }
    i32_const_cell(b, ins, |p| p.cur_fiber);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    i32_const_cell(b, ins, |p| p.cur_main);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    let _ = W;
}

fn rt_drop(b: &mut Builder<'_>) -> u32 {
    b.rt("drop")
}

/// `fb_has_waits() -> i32`: whether a continuation is still registered.
fn has_waits(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    i32_const_cell(b, ins, |p| p.live_waits);
    ins.i32_load(mem(0, 4));
    let _ = b;
}

/// `fb_root_interrupt()`: nothing can go on; interrupt every fiber and unwind every waiting
/// continuation (spec 14.5). Scopes waiting for their children keep waiting.
fn root_interrupt(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const F: u32 = 0;
    const W: u32 = 1;
    const PREV: u32 = 2;
    const CHAIN: u32 = 3;
    const END: u32 = 4;
    const P: u32 = 5;
    // Every fiber gets a whole-fiber interruption.
    i32_const_cell(b, ins, |p| p.fhead);
    ins.i32_load(mem(0, 4));
    ins.local_set(F);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, F);
    ins.i32_eqz();
    ins.br_if(1);
    ldl(ins, F, F_FLAGS);
    c(ins, FF_UNWINDING);
    ins.i32_and();
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    g(ins, F);
    c(ins, 0);
    let has = b.rt("fb_pending_has");
    ins.call(has);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    g(ins, F);
    c(ins, 0);
    let push = b.rt("fb_pending_push");
    ins.call(push);
    ins.end();
    ins.end();
    ldl(ins, F, F_NEXTF);
    ins.local_set(F);
    ins.br(0);
    ins.end();
    ins.end();
    // The live waits, newest first: their continuations unwind with their releases.
    i32_const_cell(b, ins, |p| p.wtail);
    ins.i32_load(mem(0, 4));
    ins.local_set(W);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, W);
    ins.i32_eqz();
    ins.br_if(1);
    ldl(ins, W, W_PREV);
    ins.local_set(PREV);
    ldl(ins, W, W_FLAGS);
    c(ins, WF_CLOSING);
    ins.i32_and();
    ins.i32_eqz();
    ldl(ins, W, W_LIVE);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ldl(ins, W, W_FIBER);
    ins.local_set(F);
    g(ins, F);
    call(b, ins, "dup");
    ldl(ins, W, W_CHAIN);
    ins.local_set(CHAIN);
    st_const(ins, W, W_CHAIN, 0);
    ldl(ins, W, W_END);
    c(ins, WE_MAIN);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    c(ins, WE_MAIN);
    ins.local_set(END);
    ins.else_();
    c(ins, WE_ORPHAN);
    ins.local_set(END);
    ins.end();
    // Drop the wait first: a caller's implementation is unwound before the caller, and its
    // continuation is queued ahead of this one (the C runtime's `em_drop_wait`).
    g(ins, W);
    let drop_wait = b.rt("fb_drop_wait");
    ins.call(drop_wait);
    dropv(b, ins, W);
    c(ins, JK_INTERRUPT);
    g(ins, F);
    g(ins, CHAIN);
    g(ins, END);
    c(ins, 0);
    ins.i64_const(0);
    c(ins, 0);
    c(ins, 0);
    let push = b.rt("fb_queue_push");
    ins.call(push);
    ins.end();
    g(ins, PREV);
    ins.local_set(W);
    ins.br(0);
    ins.end();
    ins.end();
    let _ = P;
}

// ---- host timers and TestClock sleepers ----

/// `fb_sleep_start(handle)`: suspend on a host timer (the subtask joined to the waitable set).
fn sleep_start(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const W: u32 = 1;
    const TAIL: u32 = 2;
    c(ins, WK_HOST);
    c(ins, 0);
    call(b, ins, "fb_wait_new");
    ins.local_set(W);
    stl(ins, W, W_SEQ, 0);
    stl(ins, W, W_HPREV, 0);
    st_const(ins, W, W_HNEXT, 0);
    i32_const_cell(b, ins, |p| p.host_tail);
    ins.i32_load(mem(0, 4));
    ins.local_set(TAIL);
    stl(ins, W, W_HPREV, TAIL);
    g(ins, TAIL);
    ins.if_(BlockType::Empty);
    stl(ins, TAIL, W_HNEXT, W);
    ins.else_();
    i32_const_cell(b, ins, |p| p.host_head);
    g(ins, W);
    ins.i32_store(mem(0, 4));
    ins.end();
    i32_const_cell(b, ins, |p| p.host_tail);
    g(ins, W);
    ins.i32_store(mem(0, 4));
    ldl(ins, W, W_FLAGS);
    c(ins, WF_HOST);
    ins.i32_or();
    ins.local_set(TAIL);
    stl(ins, W, W_FLAGS, TAIL);
    i32_const_cell(b, ins, |p| p.susp_wait);
    g(ins, W);
    ins.i32_store(mem(0, 4));
    c(ins, 1);
    ins.global_set(G_SUSPENDING);
}

/// `fb_test_sleep(at)`: suspend until the run stalls and this is the earliest sleeper.
fn test_sleep(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const W: u32 = 2;
    const TAIL: u32 = 3;
    const SEQ: u32 = 4;
    c(ins, WK_TEST);
    c(ins, 0);
    call(b, ins, "fb_wait_new");
    ins.local_set(W);
    ins.local_get(W);
    ins.local_get(0);
    ins.i64_store(mem(W_AT, 8));
    i32_const_cell(b, ins, |p| p.test_seq);
    ins.i32_load(mem(0, 4));
    c(ins, 1);
    ins.i32_add();
    ins.local_set(SEQ);
    i32_const_cell(b, ins, |p| p.test_seq);
    g(ins, SEQ);
    ins.i32_store(mem(0, 4));
    stl(ins, W, W_SEQ, SEQ);
    st_const(ins, W, W_HPREV, 0);
    st_const(ins, W, W_HNEXT, 0);
    i32_const_cell(b, ins, |p| p.test_tail);
    ins.i32_load(mem(0, 4));
    ins.local_set(TAIL);
    stl(ins, W, W_HPREV, TAIL);
    g(ins, TAIL);
    ins.if_(BlockType::Empty);
    stl(ins, TAIL, W_HNEXT, W);
    ins.else_();
    i32_const_cell(b, ins, |p| p.test_head);
    g(ins, W);
    ins.i32_store(mem(0, 4));
    ins.end();
    i32_const_cell(b, ins, |p| p.test_tail);
    g(ins, W);
    ins.i32_store(mem(0, 4));
    ldl(ins, W, W_FLAGS);
    c(ins, WF_TEST);
    ins.i32_or();
    ins.local_set(TAIL);
    stl(ins, W, W_FLAGS, TAIL);
    i32_const_cell(b, ins, |p| p.susp_wait);
    g(ins, W);
    ins.i32_store(mem(0, 4));
    c(ins, 1);
    ins.global_set(G_SUSPENDING);
}

/// `fb_idle() -> i32`: 1 when a wake was queued, 0 when nothing can wake the run.
fn idle(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const W: u32 = 0;
    const HANDLE: u32 = 1;
    const STATUS: u32 = 2;
    const EVENT: u32 = 3;
    const PREV: u32 = 4;
    const NEXT: u32 = 5;
    const TAIL: u32 = 6;
    const FOUND: u32 = 7;
    let set = cell(b, |p| p.wait_set);
    if !b.uses_std_host(StdHost::is_monotonic) {
        // Only the TestClock can wake this run; a host subtask is never started.
        let wake_test = b.rt("fb_wake_test");
        ins.call(wake_test);
        return;
    }
    i32_const_cell(b, ins, |p| p.host_head);
    ins.i32_load(mem(0, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    let wake_test = b.rt("fb_wake_test");
    ins.call(wake_test);
    ins.return_();
    ins.end();
    let set_wait = b.waitable_set_wait();
    c(ins, set as i32);
    ins.i32_load(mem(0, 4));
    c(ins, WAITBUF as i32);
    ins.call(set_wait);
    ins.local_set(EVENT);
    g(ins, EVENT);
    c(ins, 1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    c(ins, WAITBUF as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(HANDLE);
    c(ins, (WAITBUF + 4) as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(STATUS);
    // Find the sleeper with this handle.
    c(ins, 0);
    ins.local_set(FOUND);
    i32_const_cell(b, ins, |p| p.host_head);
    ins.i32_load(mem(0, 4));
    ins.local_set(W);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, W);
    ins.i32_eqz();
    ins.br_if(1);
    ldl(ins, W, W_SEQ);
    g(ins, HANDLE);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, W);
    ins.local_set(FOUND);
    // Out of the loop and its enclosing block (0: the `if`, 1: the loop, 2: the block).
    ins.br(2);
    ins.end();
    ldl(ins, W, W_HNEXT);
    ins.local_set(W);
    ins.br(0);
    ins.end();
    ins.end();
    g(ins, FOUND);
    ins.if_(BlockType::Empty);
    // Unlink the sleeper.
    ldl(ins, FOUND, W_HPREV);
    ins.local_set(PREV);
    ldl(ins, FOUND, W_HNEXT);
    ins.local_set(NEXT);
    g(ins, PREV);
    ins.if_(BlockType::Empty);
    stl(ins, PREV, W_HNEXT, NEXT);
    ins.else_();
    i32_const_cell(b, ins, |p| p.host_head);
    g(ins, NEXT);
    ins.i32_store(mem(0, 4));
    ins.end();
    g(ins, NEXT);
    ins.if_(BlockType::Empty);
    stl(ins, NEXT, W_HPREV, PREV);
    ins.else_();
    i32_const_cell(b, ins, |p| p.host_tail);
    g(ins, PREV);
    ins.i32_store(mem(0, 4));
    ins.end();
    st_const(ins, FOUND, W_HPREV, 0);
    st_const(ins, FOUND, W_HNEXT, 0);
    ldl(ins, FOUND, W_FLAGS);
    c(ins, -1 - WF_HOST);
    ins.i32_and();
    ins.local_set(TAIL);
    stl(ins, FOUND, W_FLAGS, TAIL);
    g(ins, STATUS);
    c(ins, 2);
    ins.i32_eq();
    g(ins, STATUS);
    c(ins, 4);
    ins.i32_eq();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    let drop_subtask = b.subtask_drop();
    g(ins, HANDLE);
    ins.call(drop_subtask);
    g(ins, FOUND);
    c(ins, 0);
    ins.i64_const(0);
    let wake = b.rt("fb_wait_wake");
    ins.call(wake);
    c(ins, 1);
    ins.return_();
    ins.end();
    // Still pending: join it again and leave the run to stall. wasmtime 49 only returns terminal
    // statuses from `waitable-set.wait`, so leaving 0 here does not keep a run that could go on.
    let join = b.waitable_join();
    g(ins, HANDLE);
    c(ins, set as i32);
    ins.i32_load(mem(0, 4));
    ins.call(join);
    c(ins, 0);
    ins.return_();
    ins.end();
    // An event for no sleeper: consume it.
    let drop_subtask = b.subtask_drop();
    g(ins, HANDLE);
    ins.call(drop_subtask);
    c(ins, 1);
    ins.return_();
    ins.end();
    c(ins, 0);
}

/// `fb_wake_test() -> i32`: wake the earliest TestClock sleeper (by time, then by the order the
/// sleeps were called, spec 8.8).
fn wake_test(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const W: u32 = 0;
    const BEST: u32 = 1;
    const PREV: u32 = 2;
    const NEXT: u32 = 3;
    const TAIL: u32 = 4;
    i32_const_cell(b, ins, |p| p.test_head);
    ins.i32_load(mem(0, 4));
    ins.local_set(W);
    c(ins, 0);
    ins.local_set(BEST);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, W);
    ins.i32_eqz();
    ins.br_if(1);
    g(ins, BEST);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    g(ins, W);
    ins.local_set(BEST);
    ins.else_();
    g(ins, W);
    ins.i64_load(mem(W_AT, 8));
    g(ins, BEST);
    ins.i64_load(mem(W_AT, 8));
    ins.i64_lt_s();
    g(ins, W);
    ins.i64_load(mem(W_AT, 8));
    g(ins, BEST);
    ins.i64_load(mem(W_AT, 8));
    ins.i64_eq();
    ldl(ins, W, W_SEQ);
    ldl(ins, BEST, W_SEQ);
    ins.i32_lt_u();
    ins.i32_and();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    g(ins, W);
    ins.local_set(BEST);
    ins.end();
    ins.end();
    ldl(ins, W, W_HNEXT);
    ins.local_set(W);
    ins.br(0);
    ins.end();
    ins.end();
    g(ins, BEST);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    c(ins, 0);
    ins.return_();
    ins.end();
    // Unlink the chosen sleeper.
    ldl(ins, BEST, W_HPREV);
    ins.local_set(PREV);
    ldl(ins, BEST, W_HNEXT);
    ins.local_set(NEXT);
    g(ins, PREV);
    ins.if_(BlockType::Empty);
    stl(ins, PREV, W_HNEXT, NEXT);
    ins.else_();
    i32_const_cell(b, ins, |p| p.test_head);
    g(ins, NEXT);
    ins.i32_store(mem(0, 4));
    ins.end();
    g(ins, NEXT);
    ins.if_(BlockType::Empty);
    stl(ins, NEXT, W_HPREV, PREV);
    ins.else_();
    i32_const_cell(b, ins, |p| p.test_tail);
    g(ins, PREV);
    ins.i32_store(mem(0, 4));
    ins.end();
    st_const(ins, BEST, W_HPREV, 0);
    st_const(ins, BEST, W_HNEXT, 0);
    ldl(ins, BEST, W_FLAGS);
    c(ins, -1 - WF_TEST);
    ins.i32_and();
    ins.local_set(TAIL);
    stl(ins, BEST, W_FLAGS, TAIL);
    g(ins, BEST);
    c(ins, 0);
    ins.i64_const(0);
    let wake = b.rt("fb_wait_wake");
    ins.call(wake);
    c(ins, 1);
}
