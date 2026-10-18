//! The `Fiber` value (spec chapter 11).
//!
//! **Declared only.** `check::unsupported` rejects fibers on the Flow IR, before the Heap IR
//! passes, so a `Value::Fiber` never reaches the emitter today. This module is the seam: when
//! `runtime::fiber` and the scheduler ABI in `runtime::sched` are filled in, the operations are
//! lowered here (fork the run queue, resume through `loop $dispatch`, join the `Deferred`).

use super::*;

impl<'m> Builder<'m> {
    /// The `Value::Fiber` case: unreachable while fibers are rejected by the check pass.
    pub(super) fn fiber(
        &mut self,
        cx: &mut FnCtx,
        _ins: &mut InstructionSink,
        _dst: Option<VarId>,
    ) {
        self.internal(format!("fiber left in {}", cx.f.name));
    }
}
