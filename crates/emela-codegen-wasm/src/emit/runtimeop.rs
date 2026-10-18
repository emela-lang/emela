//! The `runtime` operations (the Heap IR's `Value::Runtime`): showing strings, `dbg`,
//! retiring handler operations, the defect globals and the suspend operations (`hold` / `unhold`
//! / `yield` / `yield_perform`).

use super::*;

impl<'m> Builder<'m> {
    pub(super) fn runtime_op(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        op: RuntimeOp,
        args: &[Operand],
    ) {
        match op {
            // The suspend operations of the state machines: a fairness yield, and a `release`
            // that may suspend not being interrupted (spec 11.1, 11.4).
            RuntimeOp::Hold => {
                let f = self.rt("hold");
                ins.call(f);
            }
            RuntimeOp::Unhold => {
                let f = self.rt("unhold");
                ins.call(f);
            }
            RuntimeOp::Yield | RuntimeOp::YieldPerform => {
                ins.i32_const(i32::from(op == RuntimeOp::YieldPerform));
                let f = self.rt("yield");
                ins.call(f);
            }
            _ => {
                let name = match op {
                    RuntimeOp::ShowString => "show_string",
                    RuntimeOp::DebugPrint => "debug_print",
                    RuntimeOp::Retire => "retire",
                    RuntimeOp::Defect => "raise",
                    RuntimeOp::DefectTake => "defect_take",
                    RuntimeOp::DefectResume => "defect_resume",
                    RuntimeOp::Hold
                    | RuntimeOp::Unhold
                    | RuntimeOp::Yield
                    | RuntimeOp::YieldPerform => unreachable!(),
                };
                for a in args {
                    self.operand(cx, ins, a);
                }
                let f = self.rt(name);
                ins.call(f);
            }
        }
        let ret = match op {
            RuntimeOp::ShowString | RuntimeOp::DefectTake => Repr::Ref,
            _ => Repr::Void,
        };
        self.assign(cx, ins, dst, ret);
    }
}
