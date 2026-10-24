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
            RuntimeOp::DefectTake => "defect_take",
            RuntimeOp::DefectResume => "defect_resume",
            // The state-machine pass puts these around code that may suspend by type: fairness
            // yields before a call, and the hold of a release that must not be interrupted. The
            // check rejects every reachable suspension point and the target has no scheduler or
            // fibers, so the code runs straight through and there is nothing to yield to or
            // interrupt.
            RuntimeOp::Yield | RuntimeOp::YieldPerform | RuntimeOp::Hold | RuntimeOp::Unhold => {
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
