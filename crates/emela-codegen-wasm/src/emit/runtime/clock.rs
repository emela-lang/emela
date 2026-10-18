//! The `Std.Clock` WASM hosts (spec 8.1; the platform layer of 15.2).
//!
//! `now_milliseconds` reads WASI's REALTIME clock (`clock_time_get` with clock id 0), so it is the
//! Unix-epoch milliseconds JS' `Date.now()` and the C host return. It is **not** the driver's
//! `now_ms` (which reads its own clock); `SystemClock.now` needs the epoch, not a duration.
//! `sleep_milliseconds` and `when_stalled` suspend through the driver's timers the same way the
//! `suspend fn` external ABI does (spec 12.6): they register the task with `after` and return, and
//! the driver completes it when the timer is due. The corpus' `suspend fn sleep(ms) -> Int` host
//! (`clock_sleep`) records `ms` in the task and registers a timer too, so the driver resumes it
//! with `ms` (the value `timers_due` reads back from the task).
//!
//! This fragment is registered only when a clock internal host is requested
//! (`emit::external::INTERNAL_HOSTS`), so programs without one keep the exact bytes they had.

use super::*;

/// The clock fragment (see `runtime::FUNCS`).
pub(super) const FUNCS: &[Sig] = &[
    ("clock_now_milliseconds", &[], &[ValType::I64]),
    (
        "clock_sleep_milliseconds",
        &[ValType::I32, ValType::I32],
        &[],
    ),
    ("clock_when_stalled", &[ValType::I64, ValType::I32], &[]),
    ("clock_sleep", &[ValType::I32, ValType::I32], &[]),
];

pub(super) fn body(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "clock_now_milliseconds" => now_milliseconds(b, ins),
        "clock_sleep_milliseconds" => sleep_milliseconds(b, ins),
        "clock_when_stalled" => when_stalled(b, ins),
        "clock_sleep" => sleep(b, ins),
        _ => return false,
    }
    true
}

pub(super) fn locals(name: &str) -> Option<Vec<(u32, ValType)>> {
    match name {
        // `when_stalled`: the `at - now` delta (the parameters are locals 0 and 1).
        "clock_when_stalled" => Some(vec![(1, ValType::I64)]),
        _ => None,
    }
}

/// `clock_now_milliseconds() -> i64`: milliseconds since the Unix epoch (WASI REALTIME clock, the
/// same as JS' `Date.now()` and the C `emela_clock_now_milliseconds`).
fn now_milliseconds(b: &mut Builder, ins: &mut InstructionSink) {
    let buf = b.alloc_static(8);
    let clock = b.clock_time_get_fn;
    ins.i32_const(0); // CLOCK_REALTIME
    ins.i64_const(1_000_000);
    ins.i32_const(buf as i32);
    ins.call(clock);
    ins.drop();
    ins.i32_const(buf as i32);
    ins.i64_load(mem(0, 8));
    ins.i64_const(1_000_000);
    ins.i64_div_s();
}

/// `clock_sleep_milliseconds(ms, task)`: completes `task` with `()` after `ms`, a negative `ms`
/// being 0 (spec 8.8). The driver's timer wakes the sleeping fiber.
fn sleep_milliseconds(b: &mut Builder, ins: &mut InstructionSink) {
    let after = rt(b, "after");
    ins.local_get(0);
    ins.i32_const(0);
    ins.i32_lt_s();
    ins.if_(BlockType::Result(ValType::I32));
    ins.i32_const(0);
    ins.else_();
    ins.local_get(0);
    ins.end();
    ins.local_get(1);
    ins.call(after);
}

/// `clock_when_stalled(at, task)`: completes `task` with `()` after `at - now` milliseconds
/// (`at` is Unix-epoch milliseconds; T-Clock's `when_stalled`, spec 8.8). A time already passed is
/// 0, and the delta is clamped to the `i32` range `after` takes.
fn when_stalled(b: &mut Builder, ins: &mut InstructionSink) {
    // One extra i64 local (index 2, after the `at` and `task` parameters).
    const DELTA: u32 = 2;
    let buf = b.alloc_static(8);
    let clock = b.clock_time_get_fn;
    let after = rt(b, "after");
    // `at - now`, where now is the REALTIME clock as in `now_milliseconds`.
    ins.local_get(0);
    ins.i32_const(0); // CLOCK_REALTIME
    ins.i64_const(1_000_000);
    ins.i32_const(buf as i32);
    ins.call(clock);
    ins.drop();
    ins.i32_const(buf as i32);
    ins.i64_load(mem(0, 8));
    ins.i64_const(1_000_000);
    ins.i64_div_s();
    ins.i64_sub();
    ins.local_set(DELTA);
    // Clamp to [0, i32::MAX].
    ins.local_get(DELTA);
    ins.i64_const(0);
    ins.i64_lt_s();
    ins.if_(BlockType::Result(ValType::I32));
    ins.i32_const(0);
    ins.else_();
    ins.local_get(DELTA);
    ins.i64_const(2147483647);
    ins.i64_gt_s();
    ins.if_(BlockType::Result(ValType::I32));
    ins.i32_const(2147483647);
    ins.else_();
    ins.local_get(DELTA);
    ins.i32_wrap_i64();
    ins.end();
    ins.end();
    ins.local_get(1);
    ins.call(after);
}

/// `clock_sleep(ms, task)`: suspends the caller for `ms` and resumes it with `ms`, the `suspend fn`
/// external `sleep` of the test corpus (JS' `sleep.mjs` and C's `emela_test_sleep` resolve with the
/// milliseconds slept). The value is put in the task before the timer is registered;
/// `timers_due` completes the task and wakes the fiber with it.
fn sleep(b: &mut Builder, ins: &mut InstructionSink) {
    let after = rt(b, "after");
    ins.local_get(1); // the task
    ins.local_get(0); // ms
    ins.i64_extend_i32_s();
    ins.i64_store(mem(sched::T_VALUE, 8));
    // A negative `ms` is 0 (spec 8.8).
    ins.local_get(0);
    ins.i32_const(0);
    ins.i32_lt_s();
    ins.if_(BlockType::Result(ValType::I32));
    ins.i32_const(0);
    ins.else_();
    ins.local_get(0);
    ins.end();
    ins.local_get(1);
    ins.call(after);
}
