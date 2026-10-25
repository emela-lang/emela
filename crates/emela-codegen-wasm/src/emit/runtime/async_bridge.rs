//! The bridge between the runtime's waits and the component model's asynchronous imports.
//!
//! `Std.Clock.sleep` on `SystemClock` is an `@external suspend fn` lowered to
//! `wasi:clocks/monotonic-clock.wait-for` (spec 8.7). The canonical ABI's asynchronous lower
//! returns the status of a subtask in a packed word (`status | handle << 4`): `RETURNED` means the
//! wait already elapsed, otherwise the subtask is joined to the waitable set and the scheduler
//! waits on it only when it has no continuation left to run. Waiting happens inside the
//! async-lifted `run` export, so the guest never spins while the host is idle. The wait itself is
//! an ordinary wait of the fiber scheduler (`runtime::fiber`): the sleeper is what `fb_idle` wakes
//! when the set reports its handle, and an interruption drops it, which drops the subtask (and so
//! cancels the timer).
//!
//! `TestClock` uses the same suspension machinery without a host timer: `when-stalled` records the
//! time the sleeper waits for, and the scheduler's idle hook wakes the earliest sleeper (spec 8.8).
//! The virtual time itself stays in TestClock's `State[Instant]`; this module only starts the
//! sleeps.

use super::*;

pub(super) const FUNCS: &[Sig] = &[
    ("sleep_start", &[ValType::I32], &[]),
    ("clock_when_stalled", &[ValType::I64], &[]),
];

pub(super) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        "sleep_start" => vec![(3, ValType::I32), (1, ValType::I64)],
        _ => Vec::new(),
    }
}

pub(super) fn used(b: &Builder<'_>) -> bool {
    b.uses_sleep()
}

pub(super) fn prepare(_b: &mut Builder<'_>) {}

/// The canonical ABI status codes of an asynchronous lower (low four bits of the packed word).
const STATUS_RETURNED: i32 = 2;

pub(super) fn emit(b: &mut Builder<'_>, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "sleep_start" => sleep_start(b, ins),
        "clock_when_stalled" => clock_when_stalled(b, ins),
        _ => return false,
    }
    true
}

/// `sleep_start(ms)`: start `wait-for(ms * 1_000_000)`. Zero and negative milliseconds are
/// clamped to zero, as in `clock.mjs` and `clock.c`. When the wait has not elapsed yet, the
/// subtask is joined to a waitable set and the calling continuation suspends on a host wait.
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
    let wait_set = super::fiber::plan(b).wait_set;
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
    ins.local_get(HANDLE);
    let sleep = b.rt("fb_sleep_start");
    ins.call(sleep);
    ins.else_();
    // START_CANCELLED (3) or RETURN_CANCELLED (4): the subtask will not be waited for, so its
    // handle is dropped instead of leaked.
    let drop_subtask = b.subtask_drop();
    ins.local_get(HANDLE);
    ins.call(drop_subtask);
    ins.end();
}

/// `clock_when_stalled(at)`: register a TestClock sleeper. The virtual time itself stays in
/// TestClock's `State[Instant]`; the scheduler wakes the earliest sleeper when nothing can go on.
fn clock_when_stalled(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    ins.local_get(0);
    let sleep = b.rt("fb_test_sleep");
    ins.call(sleep);
}
