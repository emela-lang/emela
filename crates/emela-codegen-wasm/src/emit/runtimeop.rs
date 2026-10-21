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
            // The state-machine pass puts a fairness yield before a call that may suspend by
            // type. The target has no scheduler; a call that does not actually suspend (the
            // common case for a type that merely uses an effect with a suspend operation) runs
            // straight through, and an actual suspending external is a defect stub.
            RuntimeOp::Yield | RuntimeOp::YieldPerform => {
                return;
            }
            RuntimeOp::Hold | RuntimeOp::Unhold => {
                self.internal(format!("suspend runtime op {op:?} left in {}", cx.f.name));
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
