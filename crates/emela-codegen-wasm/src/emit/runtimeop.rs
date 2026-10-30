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
            // A fairness yield hands the scheduler a turn (spec 11.1). Without fibers nobody can
            // take it: the state-machine pass gives the value its suspension edge only then, so
            // the yield is a no-op here as well.
            RuntimeOp::Yield | RuntimeOp::YieldPerform => {
                if self.fiber_plan.is_some() && runtime::fiber::used(self) {
                    let perform = i32::from(matches!(op, RuntimeOp::YieldPerform));
                    ins.i32_const(perform);
                    let f = self.rt("fb_yield");
                    ins.call(f);
                }
                self.assign(cx, ins, dst, Repr::Void);
                return;
            }
            // `hold` / `unhold`: a release that may suspend is not interrupted until it ends
            // (spec 11.4).
            RuntimeOp::Hold | RuntimeOp::Unhold => {
                if self.fiber_plan.is_some() {
                    let name = if matches!(op, RuntimeOp::Hold) {
                        "fb_hold"
                    } else {
                        "fb_unhold"
                    };
                    let f = self.rt(name);
                    ins.call(f);
                }
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
