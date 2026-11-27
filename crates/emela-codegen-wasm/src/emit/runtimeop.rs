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
        if op == RuntimeOp::Concat {
            self.concat(cx, ins, dst, args);
            return;
        }
        let name = match op {
            RuntimeOp::ShowString => "show_string",
            RuntimeOp::Concat => unreachable!("concat is emitted above"),
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
                if self.fiber_plan.is_some() && self.uses_fibers() {
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

    /// `concat(parts…)`: the byte lengths are read once to make one String, then each part is
    /// copied into it. The destination local holds the new String while the parts are copied;
    /// the offset of each part is the sum of the lengths before it, which is read again there
    /// (a few loads, and the alternative would be another local).
    fn concat(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        args: &[Operand],
    ) {
        // A pure value whose result is unused allocates nothing.
        let Some(dst) = dst else { return };
        debug_assert!(args.len() > 1, "a concat of fewer than two parts");
        for (i, a) in args.iter().enumerate() {
            self.operand(cx, ins, a);
            ins.i32_load(mem(HDR, 4));
            if i > 0 {
                ins.i32_add();
            }
        }
        let new = self.rt("str_new");
        ins.call(new);
        ins.local_set(cx.local(dst));
        let copy = self.rt("copy_range");
        for (i, a) in args.iter().enumerate() {
            // `(dst, dst offset, src, src offset, length)`; the offsets are in bytes from the
            // start of the string, and `copy_range` skips the header of each side.
            ins.local_get(cx.local(dst));
            if i == 0 {
                ins.i32_const(0);
            }
            for (j, b) in args.iter().take(i).enumerate() {
                self.operand(cx, ins, b);
                ins.i32_load(mem(HDR, 4));
                if j > 0 {
                    ins.i32_add();
                }
            }
            self.operand(cx, ins, a);
            ins.i32_const(0);
            self.operand(cx, ins, a);
            ins.i32_load(mem(HDR, 4));
            ins.call(copy);
        }
    }
}
