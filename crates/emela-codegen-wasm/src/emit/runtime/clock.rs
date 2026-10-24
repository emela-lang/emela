//! `Std.Clock.now` on WASI 0.3: `wasi:clocks/system-clock.now`.
//!
//! The host returns `instant { seconds: s64, nanoseconds: u32 }` through a pointer. The result is
//! Unix epoch milliseconds, `seconds * 1000 + nanoseconds / 1_000_000` (the division truncates).
//! `Std.Clock.sleep` and the TestClock's idle hook are in `runtime::async_bridge`.

use super::*;

pub(super) const FUNCS: &[Sig] = &[("clock_now", &[], &[ValType::I64])];

pub(super) fn locals(_name: &str) -> Vec<(u32, ValType)> {
    Vec::new()
}

pub(super) fn used(b: &Builder) -> bool {
    b.uses_clock()
}

pub(super) fn emit(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    if name != "clock_now" {
        return false;
    }
    ins.i32_const(HOSTBUF as i32);
    ins.call(b.system_clock_now());
    ins.i32_const(HOSTBUF as i32);
    ins.i64_load(mem(0, 8));
    ins.i64_const(1000);
    ins.i64_mul();
    ins.i32_const((HOSTBUF + 8) as i32);
    ins.i32_load(mem(0, 4));
    ins.i64_extend_i32_u();
    ins.i64_const(1_000_000);
    ins.i64_div_u();
    ins.i64_add();
    true
}
