//! Lowering of the `RuntimeOp` operations.

use super::*;

use emela_lir::RuntimeOp;

impl<'m> Builder<'m> {
    pub(super) fn runtime_op(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        op: RuntimeOp,
        args: &[Operand],
    ) {
        let name = match op {
            RuntimeOp::ShowString => "show_string",
            RuntimeOp::DebugPrint => "debug_print",
            RuntimeOp::Retire => "retire",
            RuntimeOp::Defect => "raise",
            RuntimeOp::DefectAt => "raise_at",
            RuntimeOp::DefectTake => "defect_take",
            RuntimeOp::DefectResume => "defect_resume",
            // A fairness yield hands the scheduler a turn. The WASM backend has no fibers yet,
            // so there is nobody to hand it to (and the state-machine pass still gives the value
            // its suspension edge).
            RuntimeOp::Yield | RuntimeOp::YieldPerform => {
                self.assign(cx, ins, dst, Repr::Void);
                return;
            }
            // `hold` / `unhold` keep a release that may suspend from being interrupted until it
            // ends (spec 11.4). Interruption is only delivered at the end of a run here, and no
            // fiber can interrupt another, so the release is never cut short and both are no-ops.
            RuntimeOp::Hold | RuntimeOp::Unhold => {
                self.assign(cx, ins, dst, Repr::Void);
                return;
            }
        };
        for a in args {
            self.operand(cx, ins, a);
        }
        let f = self.rt(name);
        ins.call(f);
        let ret = match op {
            RuntimeOp::ShowString | RuntimeOp::DefectTake => Repr::Ref,
            _ => Repr::Void,
        };
        self.assign(cx, ins, dst, ret);
    }
}
