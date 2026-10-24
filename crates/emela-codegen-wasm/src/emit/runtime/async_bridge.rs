//! The bridge between the guest scheduler and the component model's asynchronous imports.
//!
//! `Std.Clock.sleep` on `SystemClock` is an `@external suspend fn` lowered to
//! `wasi:clocks/monotonic-clock.wait-for` (spec 8.7). The canonical ABI's asynchronous lower
//! returns the status of a subtask in a packed word (`status | handle << 4`): `RETURNED` means the
//! wait already elapsed, otherwise the subtask is joined to a waitable set and the scheduler
//! waits on it only when it has no continuation left to run. Waiting happens inside the
//! async-lifted `run` export, so the guest never spins while the host is idle.
//!
//! `TestClock` uses the same suspension machinery without a host timer: `when-stalled` records
//! the time the sleeper waits for, and the scheduler's idle hook wakes the earliest sleeper by
//! resuming its continuation (spec 8.8). The virtual time itself stays in TestClock's
//! `State[Instant]`; this module only decides when the run has stalled.

use super::*;

pub(super) const FUNCS: &[Sig] = &[
    ("sleep_start", &[ValType::I32], &[]),
    ("clock_when_stalled", &[ValType::I64], &[]),
    ("wait_park", &[ValType::I32], &[]),
    ("wait_run", &[], &[]),
    ("wait_interrupt", &[ValType::I32], &[]),
    ("wait_has", &[], &[ValType::I32]),
    ("wait_idle", &[], &[ValType::I32]),
    ("wait_wake", &[], &[]),
    ("wait_cancel", &[], &[]),
];

pub(super) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        "sleep_start" => vec![(3, ValType::I32), (1, ValType::I64)],
        "wait_park" => vec![(1, ValType::I32)],
        "wait_idle" => vec![(4, ValType::I32)],
        "wait_wake" => vec![(2, ValType::I32)],
        "wait_interrupt" => vec![(1, ValType::I32)],
        "wait_cancel" => vec![(1, ValType::I32)],
        _ => Vec::new(),
    }
}

/// The statics of the bridge. The scheduler cell (`pending`) belongs to `emit::suspend`; the rest
/// are only allocated when a `Std.Clock.sleep` handler is used.
pub(crate) struct Plan {
    pub(crate) host_subtask: u32,
    pub(crate) wait_set: u32,
    pub(crate) wait_chain: u32,
    pub(crate) sleeper_on: u32,
}

pub(super) fn used(b: &Builder<'_>) -> bool {
    b.uses_sleep()
}

pub(crate) fn prepare(b: &mut Builder<'_>) {
    if !used(b) {
        return;
    }
    let host_subtask = b.alloc_static(4);
    let wait_set = b.alloc_static(4);
    let wait_chain = b.alloc_static(4);
    let sleeper_on = b.alloc_static(4);
    b.async_plan = Some(Plan {
        host_subtask,
        wait_set,
        wait_chain,
        sleeper_on,
    });
}

fn plan<'a>(b: &'a Builder<'_>) -> &'a Plan {
    b.async_plan.as_ref().expect("the async bridge")
}

fn sus_cell(b: &Builder<'_>, f: impl Fn(&super::super::suspend::Plan) -> u32) -> u32 {
    f(b.sus.as_ref().expect("the suspend runtime"))
}

/// The canonical ABI status codes of an asynchronous lower (low four bits of the packed word).
const STATUS_RETURNED: i32 = 2;
const STATUS_RETURNED_CANCELLED: i32 = 4;
/// The event code for a completed subtask.
const EVENT_SUBTASK: i32 = 1;

pub(super) fn emit(b: &mut Builder<'_>, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "sleep_start" => sleep_start(b, ins),
        "clock_when_stalled" => clock_when_stalled(b, ins),
        "wait_park" => wait_park(b, ins),
        "wait_run" => wait_run(b, ins),
        "wait_interrupt" => wait_interrupt(b, ins),
        "wait_has" => wait_has(b, ins),
        "wait_idle" => wait_idle(b, ins),
        "wait_wake" => wait_wake(b, ins),
        "wait_cancel" => wait_cancel(b, ins),
        _ => return false,
    }
    true
}

/// `sleep_start(ms)`: start `wait-for(ms * 1_000_000)`. Zero and negative milliseconds are
/// clamped to zero, as in `clock.mjs` and `clock.c`. When the wait has not elapsed yet, the
/// subtask is joined to a waitable set and the calling continuation suspends.
fn sleep_start(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const PACKED: u32 = 1;
    if !b.uses_std_host(StdHost::is_monotonic) {
        // Only the TestClock hook is used: this program has no monotonic-clock import.
        ins.unreachable();
        return;
    }
    const CODE: u32 = 2;
    const HANDLE: u32 = 3;
    const NS: u32 = 4;
    let host_subtask = plan(b).host_subtask;
    let wait_set = plan(b).wait_set;
    let wait_for = b.monotonic_wait_for();
    let set_new = b.waitable_set_new();
    let join = b.waitable_join();
    ins.local_get(0);
    ins.i32_const(0);
    ins.i32_lt_s();
    ins.if_(BlockType::Result(ValType::I32));
    ins.i32_const(0);
    ins.else_();
    ins.local_get(0);
    ins.end();
    ins.i64_extend_i32_u();
    ins.i64_const(1_000_000);
    ins.i64_mul();
    ins.local_set(NS);
    ins.local_get(NS);
    ins.call(wait_for);
    ins.local_set(PACKED);
    ins.local_get(PACKED);
    ins.i32_const(0xF);
    ins.i32_and();
    ins.local_set(CODE);
    ins.local_get(PACKED);
    ins.i32_const(4);
    ins.i32_shr_u();
    ins.local_set(HANDLE);
    // The wait already elapsed: nothing suspends.
    ins.local_get(CODE);
    ins.i32_const(STATUS_RETURNED);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    // Only STARTING and STARTED have a subtask to wait for.
    ins.local_get(CODE);
    ins.i32_const(2);
    ins.i32_lt_u();
    ins.if_(BlockType::Empty);
    ins.i32_const(wait_set as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(wait_set as i32);
    ins.call(set_new);
    ins.i32_store(mem(0, 4));
    ins.end();
    ins.local_get(HANDLE);
    ins.i32_const(wait_set as i32);
    ins.i32_load(mem(0, 4));
    ins.call(join);
    ins.i32_const(host_subtask as i32);
    ins.local_get(HANDLE);
    ins.i32_store(mem(0, 4));
    ins.i32_const(1);
    ins.global_set(G_SUSPENDING);
    ins.else_();
    // START_CANCELLED (3) or RETURN_CANCELLED (4): the subtask will not be waited for, so its
    // handle is dropped instead of leaked.
    let drop_subtask = b.subtask_drop();
    ins.local_get(HANDLE);
    ins.call(drop_subtask);
    ins.end();
}

/// `clock_when_stalled(at)`: register the TestClock sleeper. The run wakes it only when it has
/// nothing else to do. The wake time itself stays in TestClock's `State[Instant]`, so the
/// scheduler only needs to know that a sleeper waits; with one fiber there is at most one.
fn clock_when_stalled(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    let sleeper_on = plan(b).sleeper_on;
    ins.i32_const(sleeper_on as i32);
    ins.i32_const(1);
    ins.i32_store(mem(0, 4));
    ins.i32_const(1);
    ins.global_set(G_SUSPENDING);
}

/// `wait_park(chain)`: remember the caller continuation of the suspension cascade. `pending` is
/// the cascade's outermost operation, so the continuation is its caller; parking ends the
/// cascade, so the next suspension starts a new one. The innermost operation's implementation is
/// resumed when the host event (or the TestClock sleeper) wakes it, and the caller chains follow
/// through the perform records' caller links.
fn wait_park(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const P: u32 = 1;
    let pending = sus_cell(b, |p| p.pending);
    let wait_chain = plan(b).wait_chain;
    ins.i32_const(pending as i32);
    ins.i32_load(mem(0, 4));
    ins.local_tee(P);
    ins.if_(BlockType::Empty);
    ins.local_get(P);
    ins.local_get(0);
    ins.i32_store(mem(super::sched::P_CALLER, 4));
    ins.local_get(P);
    ins.i32_const(1);
    ins.i32_store(mem(super::sched::P_HAS_CALLER, 4));
    // The cascade is parked; the next suspension is a new cascade. The record's reference is
    // released when it completes (`resume_with`), not here.
    ins.i32_const(pending as i32);
    ins.i32_const(0);
    ins.i32_store(mem(0, 4));
    ins.else_();
    ins.i32_const(wait_chain as i32);
    ins.local_get(0);
    ins.i32_store(mem(0, 4));
    ins.end();
}

fn wait_has(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    let sleep_op = sus_cell(b, |p| p.sleep_op);
    let pending = sus_cell(b, |p| p.pending);
    let wait_chain = plan(b).wait_chain;
    for cell in [sleep_op, pending, wait_chain] {
        ins.i32_const(cell as i32);
        ins.i32_load(mem(0, 4));
        ins.if_(BlockType::Empty);
        ins.i32_const(1);
        ins.return_();
        ins.end();
    }
    ins.i32_const(0);
}

/// `wait_run()`: drain the continuation queue, and wait on the host or wake the TestClock sleeper
/// only when no continuation is runnable. Without either, unwind the waiting continuations as
/// interrupted (the run ends with 130, spec 14.5).
fn wait_run(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    let drain = b.rt("sched_drain");
    let has = b.rt("wait_has");
    let idle = b.rt("wait_idle");
    let interrupt = b.rt("wait_interrupt");
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.call(drain);
    ins.call(has);
    ins.i32_eqz();
    ins.br_if(1);
    ins.call(idle);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.call(interrupt);
    ins.call(drain);
    ins.br(1);
    ins.end();
    ins.br(0);
    ins.end();
    ins.end();
}

fn wait_interrupt(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const EXTRA: u32 = 1;
    let wait_chain = plan(b).wait_chain;
    let cancel = b.rt("wait_cancel");
    let unwind = b.rt("unwind_ops");
    ins.call(cancel);
    ins.local_get(0);
    ins.i32_const(-1);
    ins.call(unwind);
    ins.i32_const(wait_chain as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.i32_const(wait_chain as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(EXTRA);
    ins.i32_const(wait_chain as i32);
    ins.i32_const(0);
    ins.i32_store(mem(0, 4));
    ins.local_get(EXTRA);
    let push = b.rt("queue_push");
    ins.i32_const(2);
    let interrupted = b.rt("interrupted");
    ins.call(interrupted);
    ins.i64_extend_i32_u();
    ins.call(push);
    ins.end();
}

/// `wait_idle() -> i32`: 1 when a wake was queued, 0 when nothing can wake the run.
fn wait_idle(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const EVENT: u32 = 1;
    const HANDLE: u32 = 2;
    const STATUS: u32 = 3;
    let host_subtask = plan(b).host_subtask;
    let wait_set = plan(b).wait_set;
    let sleeper_on = plan(b).sleeper_on;
    let wake = b.rt("wait_wake");
    // The host subtask path is only reachable when the program imports the monotonic clock; a
    // TestClock-only program has the sleeper path alone and no `subtask.drop` import.
    if b.uses_std_host(StdHost::is_monotonic) {
        let set_wait = b.waitable_set_wait();
        let join = b.waitable_join();
        let drop_subtask = b.subtask_drop();
        let cancel = b.rt("wait_cancel");
        ins.i32_const(host_subtask as i32);
        ins.i32_load(mem(0, 4));
        ins.if_(BlockType::Empty);
        ins.i32_const(wait_set as i32);
        ins.i32_load(mem(0, 4));
        ins.i32_const(WAITBUF as i32);
        ins.call(set_wait);
        ins.local_set(EVENT);
        ins.local_get(EVENT);
        ins.i32_const(EVENT_SUBTASK);
        ins.i32_eq();
        ins.if_(BlockType::Empty);
        ins.i32_const(WAITBUF as i32);
        ins.i32_load(mem(0, 4));
        ins.local_set(HANDLE);
        ins.i32_const((WAITBUF + 4) as i32);
        ins.i32_load(mem(0, 4));
        ins.local_set(STATUS);
        ins.i32_const(host_subtask as i32);
        ins.i32_const(0);
        ins.i32_store(mem(0, 4));
        ins.local_get(STATUS);
        ins.i32_const(STATUS_RETURNED);
        ins.i32_eq();
        ins.local_get(STATUS);
        ins.i32_const(STATUS_RETURNED_CANCELLED);
        ins.i32_eq();
        ins.i32_or();
        ins.if_(BlockType::Empty);
        ins.local_get(HANDLE);
        ins.call(drop_subtask);
        ins.call(wake);
        ins.i32_const(1);
        ins.return_();
        ins.end();
        // A still-pending status is not an event we can use; join it again and leave the run to
        // stall rather than spinning.
        ins.local_get(HANDLE);
        ins.i32_const(wait_set as i32);
        ins.i32_load(mem(0, 4));
        ins.call(join);
        ins.i32_const(host_subtask as i32);
        ins.local_get(HANDLE);
        ins.i32_store(mem(0, 4));
        ins.i32_const(0);
        ins.return_();
        ins.else_();
        // A cancellation event: the task is going away, so let the interruption path unwind.
        ins.call(cancel);
        ins.i32_const(0);
        ins.return_();
        ins.end();
        ins.else_();
        wake_sleeper(sleeper_on, wake, ins);
        ins.end();
    } else {
        wake_sleeper(sleeper_on, wake, ins);
    }
    ins.i32_const(0);
}

/// Wake the TestClock sleeper if one waits: clear it and queue its wake.
fn wake_sleeper(sleeper_on: u32, wake: u32, ins: &mut InstructionSink) {
    ins.i32_const(sleeper_on as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.i32_const(sleeper_on as i32);
    ins.i32_const(0);
    ins.i32_store(mem(0, 4));
    ins.call(wake);
    ins.i32_const(1);
    ins.return_();
    ins.end();
}

/// Wake the continuation that waits for the event: the innermost operation's implementation
/// first. Its `resume` call queues the next caller, and so on outwards (spec 14.5 item 3).
fn wait_wake(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const CHAIN: u32 = 0;
    const P: u32 = 1;
    let sleep_op = sus_cell(b, |p| p.sleep_op);
    let wait_chain = plan(b).wait_chain;
    let push = b.rt("queue_push");
    ins.i32_const(sleep_op as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(P);
    ins.local_get(P);
    ins.if_(BlockType::Empty);
    ins.local_get(P);
    ins.i32_load(mem(super::sched::P_IMPL, 4));
    ins.local_set(CHAIN);
    ins.local_get(P);
    ins.i32_const(0);
    ins.i32_store(mem(super::sched::P_IMPL, 4));
    ins.else_();
    ins.i32_const(wait_chain as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(CHAIN);
    ins.i32_const(wait_chain as i32);
    ins.i32_const(0);
    ins.i32_store(mem(0, 4));
    ins.end();
    ins.local_get(CHAIN);
    ins.if_(BlockType::Empty);
    ins.local_get(CHAIN);
    ins.i32_const(0);
    ins.i64_const(0);
    ins.call(push);
    ins.end();
}

/// `wait_cancel()`: drop a host subtask (which cancels the timer) and forget a TestClock sleeper.
fn wait_cancel(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const HANDLE: u32 = 0;
    let host_subtask = plan(b).host_subtask;
    let sleeper_on = plan(b).sleeper_on;
    ins.i32_const(sleeper_on as i32);
    ins.i32_const(0);
    ins.i32_store(mem(0, 4));
    if b.uses_std_host(StdHost::is_monotonic) {
        let drop_subtask = b.subtask_drop();
        ins.i32_const(host_subtask as i32);
        ins.i32_load(mem(0, 4));
        ins.local_tee(HANDLE);
        ins.if_(BlockType::Empty);
        ins.i32_const(host_subtask as i32);
        ins.i32_const(0);
        ins.i32_store(mem(0, 4));
        // `subtask.drop` also removes the subtask from its waitable set (and so cancels the
        // timer).
        ins.local_get(HANDLE);
        ins.call(drop_subtask);
        ins.end();
    }
}
