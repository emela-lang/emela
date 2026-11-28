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
//! - The run-time scope of a group closes when its body returns a value or a task fails: the
//!   remaining tasks are interrupted and the scope waits for them (`closer`), then the first
//!   failure (or the body's result) ends the scope (`[task.group]`). That of a background waits
//!   for its tasks without interrupting them, unless an interruption reaches it
//!   (`[task.background]`).
//!
//! Every runtime record is an ordinary counted object, so `alloc` / `drop` free them. Only the
//! links between records (target lists, the run's lists, the queue) are raw, so no reference
//! cycle keeps a record alive. The scheduler runs on one thread and resumes one continuation at a
//! time; the order of the queue is the order of the wakeups, so a run is repeatable.
//!
//! This module holds the layouts of the records, the plan of statics and the emission helpers.
//! The runtime functions are in its submodules, by what they handle: frames and operation calls
//! (`sched`), the Semaphores and Queues (`line`), `Exit.attempt` (`attempt`) and the host's
//! asynchronous calls (`async_bridge`), then the fiber runtime itself from `interrupt` to `sleep`.

pub(super) mod async_bridge;
pub(in crate::emit) mod attempt;
mod deferred;
mod interrupt;
pub(super) mod line;
mod note;
mod queue;
mod run;
pub(super) mod sched;
mod scope;
mod sleep;
mod strand;
mod wait;

use super::*;
use deferred::{
    complete, deferred, deferred_await, fork, hold, interrupt_fiber, interrupt_here, join, poll,
    unhold, yield_,
};
use interrupt::{
    deliver, fiber_of, interrupted_new, interrupting, is_interrupted, is_unwinding, pending_clear,
    pending_has, pending_push, pending_remove, scope_of, unwinds_has,
};
use note::{note, notes_add, notes_clear, notes_of};
use queue::{advance, begin, drain, job_free, later_move, queue_push, suspended};
use run::{call0, call1, finish_run, has_waits, root_begin, root_interrupt, root_run};
use scope::{
    scope, scope_body_done, scope_body_resume, scope_child_ended, scope_close_resume, scope_failed,
    scope_finish, scope_interrupted,
};
use sleep::{idle, sleep_start, test_sleep, wake_test};
use strand::{cause_new, cause_string, finish, run_result, settled_impl, strand_end, watch_settle};
use wait::{drop_wait, unlink_wait, wait_chain, wait_new, wait_wake, watch_add, watch_remove};

use attempt::{CAUSE_DIE, CAUSE_FAIL};
use sched::{F_NEXT, P_CALLER, P_IMPL, P_USED, P_WHERE};

/// How an interrupted fiber ended. It is never a value of `CauseKind` (an interruption is not a
/// cause, `[task.cause]`).
const CAUSE_INTERRUPT: i32 = 2;
/// The `perform` record's result representation (see `sched`).
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
pub(super) const F_NEXTF: u32 = HDR + 136; // raw: the run's list of fibers
const F_PREVF: u32 = HDR + 144;
const F_ITEM: u32 = HDR + 152; // raw: the scope's list of children
const F_ITEMPREV: u32 = HDR + 160;
const F_REPR: u32 = HDR + 168;
const F_WAITING: u32 = HDR + 176; // raw: the background scope whose release waits on this fiber
const F_SLOTS: i32 = 23;

/// `F_FLAGS`: the fiber ended, unwinds as interrupted, or is in the run's list.
const FF_ENDED: i32 = 1;
const FF_UNWINDING: i32 = 2;
const FF_LISTED: i32 = 4;
/// The representation index of `Repr::Ref` (see `emit::suspend::repr_index`).
pub(super) const R_REF: i32 = 4;
/// The budget of suspending calls before a fairness yield (spec 11.1; the C runtime's
/// `EM_BUDGET`).
const BUDGET: i32 = 1000;

// ---- the scope record ----

const S_FIBER: u32 = HDR; // raw: the fiber its body runs on
const S_DEPTH: u32 = HDR + 8;
pub(super) const S_FLAGS: u32 = HDR + 16;
const S_FIRST: u32 = HDR + 24; // Ref: the first failure (a Cause)
const S_CHEAD: u32 = HDR + 32; // raw: children that have not ended
const S_CTAIL: u32 = HDR + 40;
const S_CLOSER: u32 = HDR + 48; // raw: the wait for the last child
const S_SLOTS: i32 = 7;

/// No more starts (a group only): the body ended or a task failed. A background does not close
/// (`[task.background]`).
pub(super) const SF_CLOSED: i32 = 1;
const SF_BODY_DONE: i32 = 2;
const SF_ENDED: i32 = 4;
/// A background (`[task.background]`): its tasks are not interrupted when the body ends or a task
/// fails.
const SF_BACKGROUND: i32 = 8;
/// A background whose with was interrupted from outside after its body ended (the body unwound by
/// an interruption, or one arrived while the release waits). While the body runs,
/// `fb_scope_interrupted` looks at the fiber instead.
const SF_INTERRUPTED: i32 = 16;

// ---- the Deferred record ----

const D_FULL: u32 = HDR;
const D_ISREF: u32 = HDR + 8;
const D_WORD: u32 = HDR + 16; // i64
const D_VAL: u32 = HDR + 24; // Ref
const D_WHEAD: u32 = HDR + 32;
const D_WTAIL: u32 = HDR + 40;
const D_SLOTS: i32 = 6;

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
/// Ref: the Semaphore or Queue in whose line the wait is (`WK_LINE`, `runtime::fiber::line`). Dropping
/// the wait takes it out of the line; a wait that is woken only forgets the owner, since whoever
/// served it already took it out.
pub(super) const W_OWNER: u32 = HDR + 152;
const W_SLOTS: i32 = 20;

/// What a wait waits for.
pub(super) const WK_FIBER: i32 = 0; // join of a task
pub(super) const WK_DEFERRED: i32 = 1;
pub(super) const WK_PAUSE: i32 = 2; // a fairness yield
pub(super) const WK_CLOSER: i32 = 3; // a scope waiting for its children
pub(super) const WK_CALLER: i32 = 4; // a caller waiting for an operation implementation
pub(super) const WK_HOST: i32 = 5; // a host timer
pub(super) const WK_TEST: i32 = 6; // a TestClock sleeper
pub(super) const WK_LINE: i32 = 7; // a line of a Semaphore or Queue (`runtime::fiber::line`)
/// The verb of a wait registered on a fiber.
const WV_JOIN: i32 = 0;

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
//
// A job is a tagged union: its kind says which of the fields are live. A wake does not allocate
// for it: the run's queue and the later list take slots from a static ring, and only a full ring
// falls back to a heap job. The ring is threaded as a free list through `J_NEXT`; a slot goes
// back to it only when the job is fully drained (`fb_job_free`), so a queued job is never
// overwritten. Both storages have the same layout, so the `J_*` offsets address either; the
// address tells which one a job used (the ring is static data below the heap).

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
/// The job slots the run's queue and later list share: the static job ring's capacity. When every
/// slot is in use, `queue_push` makes the job on the heap instead, so a full ring never drops a
/// job (see `fb_job_free`).
const JOB_ENTRIES: u32 = 64;
/// The bytes of a ring slot: the header and the eight job slots, so a ring slot and a heap job
/// have the same layout.
const JOB_BYTES: u32 = HDR + J_SLOTS as u32 * 8;

pub(super) const JK_RESUME: i32 = 0;
pub(super) const JK_BEGIN: i32 = 1;
pub(super) const JK_INTERRUPT: i32 = 2;
/// A fairness yield: `J_CHAIN` is the pause wait to resume from (the C runtime's `EM_JOB_GO`).
pub(super) const JK_GO: i32 = 3;
/// A wake of a fiber that was given what it waited for in a line of a Semaphore or Queue
/// (`fb_wait_served`, the C runtime's `em_job::served`): it resumes with its value even when an
/// interruption arrived after the wake, which then takes effect at its next suspension point.
pub(super) const JK_SERVED: i32 = 4;

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
    /// The job ring's base and the head of its free list (`J_NEXT` links a free slot to the next).
    pub(crate) job_ring: u32,
    pub(crate) job_free: u32,
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
    pub(crate) note_desc: u32,
    pub(crate) scope_body_desc: u32,
    pub(crate) scope_close_desc: u32,
    pub(crate) msg_scope_fork: u32,
    pub(crate) note_background: u32,
    pub(crate) failed_after: u32,
    pub(crate) note_other: u32,
    /// The Semaphores and Queues (`runtime::fiber::line`).
    pub(crate) line: line::Plan,
}

pub(super) fn plan<'a>(b: &'a Builder<'_>) -> &'a Plan {
    b.fiber_plan.as_ref().expect("the fiber runtime")
}

fn cell(b: &Builder<'_>, f: impl Fn(&Plan) -> u32) -> u32 {
    f(plan(b))
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
        &[ValType::I32, ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I64],
    ),
    (
        "fb_fork",
        &[ValType::I32, ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    ("fb_join", &[ValType::I32, ValType::I32], &[ValType::I64]),
    ("fb_deferred", &[], &[ValType::I32]),
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
    (
        "fb_wait_served",
        &[ValType::I32, ValType::I32, ValType::I64],
        &[],
    ),
    ("fb_wait_chain", &[ValType::I32], &[ValType::I32]),
    ("fb_job_free", &[ValType::I32], &[]),
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
    ("fb_scope_interrupted", &[ValType::I32], &[ValType::I32]),
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
        "fb_deferred" => i32s(1),
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
        "fb_wait_wake" | "fb_wait_served" => i32s(7),
        "fb_wait_chain" => i32s(5),
        "fb_job_free" => i32s(1),
        "fb_watch_add" => i32s(4),
        "fb_watch_remove" => i32s(6),
        "fb_unlink_wait" => i32s(5),
        "fb_drop_wait" => i32s(6),
        "fb_queue_push" | "fb_later_push" => i32s(9),
        "fb_later_move" => i32s(3),
        "fb_scope_body_resume" => vec![(3, ValType::I32), (1, ValType::I64)],
        "fb_scope_close_resume" => vec![(6, ValType::I32), (1, ValType::I64)],
        "fb_scope_failed" => i32s(4),
        "fb_scope_interrupted" => i32s(3),
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

pub(super) fn c(ins: &mut InstructionSink, v: i32) {
    ins.i32_const(v);
}

pub(super) fn g(ins: &mut InstructionSink, l: u32) {
    ins.local_get(l);
}

pub(super) fn ldl(ins: &mut InstructionSink, l: u32, off: u32) {
    ins.local_get(l);
    ins.i32_load(mem(off, 4));
}

pub(super) fn stl(ins: &mut InstructionSink, l: u32, off: u32, v: u32) {
    ins.local_get(l);
    ins.local_get(v);
    ins.i32_store(mem(off, 4));
}

pub(super) fn st_const(ins: &mut InstructionSink, l: u32, off: u32, v: i32) {
    ins.local_get(l);
    ins.i32_const(v);
    ins.i32_store(mem(off, 4));
}

pub(super) fn call(b: &mut Builder, ins: &mut InstructionSink, name: &'static str) {
    let f = b.rt(name);
    ins.call(f);
}

pub(super) fn dropv(b: &mut Builder, ins: &mut InstructionSink, l: u32) {
    ins.local_get(l);
    call(b, ins, "drop");
}

pub(super) fn raise_msg(b: &mut Builder, ins: &mut InstructionSink, msg: u32) {
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
            false, false, false, false, false, false, false, false, false, false,
        ],
        stride: 0,
    });
    let scope_desc = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "Scope".to_owned(),
        show: None,
        refs: vec![false, false, false, true, false, false, false],
        stride: 0,
    });
    let deferred_desc = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "Deferred".to_owned(),
        show: None,
        refs: vec![false, false, false, true, false, false],
        stride: 0,
    });
    let waiter_desc = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "wait".to_owned(),
        show: None,
        refs: vec![
            true, true, false, false, false, false, false, false, false, true, false, false, false,
            false, false, false, false, false, true, true,
        ],
        stride: 0,
    });
    let node_desc = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "interruption".to_owned(),
        show: None,
        refs: vec![false, true],
        stride: 0,
    });
    let job_desc = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "job".to_owned(),
        show: None,
        refs: vec![false, false, true, true, true, false, false, false],
        stride: 0,
    });
    let cause_desc = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "Cause".to_owned(),
        show: None,
        refs: vec![true],
        stride: 0,
    });
    let note_desc = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "note".to_owned(),
        show: None,
        refs: vec![true, true, false],
        stride: 0,
    });
    let scope_body_desc = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "Task.Group".to_owned(),
        show: None,
        refs: vec![false, false, true, true, false],
        stride: 0,
    });
    let scope_close_desc = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "Task.Group".to_owned(),
        show: None,
        // Slots 0..7. Slot 6 is `RESVAL`: it shares its word with the high half of the i64
        // `WORD` in slot 5, and only one of the two is live (the body's representation chooses).
        // Slot 7 is the i32 `MODE`, not a reference.
        refs: vec![false, false, true, true, false, false, true, false],
        stride: 0,
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
    let notes = if b.uses_fibers() {
        b.alloc_static(8)
    } else {
        0
    };
    let msg = |b: &mut Builder<'_>, s: &[u8]| b.string_literal(s);
    let msg_scope_fork = msg(b, b"task group was used after it closed");
    let note_background = msg(
        b,
        b"\nnote: the release of handler Task.Background also failed: ",
    );
    let failed_after = msg(b, b" failed after resume: ");
    let note_other = msg(b, b"note: another task also failed: ");
    let line = line::prepare(b);
    // The job ring and its free list. The queue takes a slot per wake instead of allocating; a
    // run releases every job before the next one starts (`finish_run`), so the list is ready then.
    let job_ring = b.alloc_static(JOB_ENTRIES * JOB_BYTES);
    let mut free = 0;
    for i in (0..JOB_ENTRIES).rev() {
        let slot = job_ring + i * JOB_BYTES;
        b.write_u32(slot + J_NEXT, free);
        free = slot;
    }
    let job_free = b.alloc_static(4);
    b.write_u32(job_free, free);
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
        job_ring,
        job_free,
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
        note_desc,
        scope_body_desc,
        scope_close_desc,
        msg_scope_fork,
        note_background,
        failed_after,
        note_other,
        line,
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
        "fb_wait_wake" => wait_wake(b, ins, false),
        "fb_wait_served" => wait_wake(b, ins, true),
        "fb_wait_chain" => wait_chain(b, ins),
        "fb_job_free" => job_free(b, ins),
        "fb_watch_add" => watch_add(b, ins),
        "fb_watch_remove" => watch_remove(b, ins),
        "fb_queue_push" => queue_push(b, ins, |p| (p.qhead, p.qtail)),
        "fb_later_push" => queue_push(b, ins, |p| (p.lhead, p.ltail)),
        "fb_later_move" => later_move(b, ins),
        "fb_scope_body_resume" => scope_body_resume(b, ins),
        "fb_scope_close_resume" => scope_close_resume(b, ins),
        "fb_scope_failed" => scope_failed(b, ins),
        "fb_scope_interrupted" => scope_interrupted(b, ins),
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

pub(super) fn i32_const_cell(
    b: &mut Builder<'_>,
    ins: &mut InstructionSink,
    f: impl Fn(&Plan) -> u32,
) {
    ins.i32_const(cell(b, f) as i32);
}

/// The same for a cell of the suspend runtime.
fn sic(
    b: &mut Builder<'_>,
    ins: &mut InstructionSink,
    f: impl Fn(&super::super::suspend::Plan) -> u32,
) {
    ins.i32_const(sched::cell(b, f) as i32);
}
