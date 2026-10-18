//! The `Fiber` value (spec chapter 11).
//!
//! Every concurrency operation (`Value::Fiber`) is a call of a runtime function in
//! `runtime::fiber`/`runtime::sched`. The runtime moves values of any type, so it is told their
//! representation (a [`repr_code`]); the results that are a word are unpacked into the
//! destination's representation here.

use emela_core::{FiberOp, Type, TypeId};

use super::*;

impl<'m> Builder<'m> {
    /// The `Value::Fiber` case: a call of the runtime (spec chapter 11).
    pub(super) fn fiber(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        op: FiberOp,
        args: &[Operand],
    ) {
        let arg_ty = |cx: &FnCtx, i: usize| match args.get(i) {
            Some(Operand::Var(v)) => Some(cx.f.vars[*v].ty),
            _ => None,
        };
        let dst_repr = dst.map_or(Repr::Void, |d| cx.f.vars[d].repr);
        let dst_ty = dst.map(|d| cx.f.vars[d].ty);
        match op {
            FiberOp::Scope { .. } => {
                let repr = arg_ty(cx, 0).map_or(Repr::Void, |t| self.inner_repr(t));
                self.operand(cx, ins, &args[0]);
                ins.i32_const(repr_code(repr) as i32);
                ins.call(self.rt("fiber_scope"));
                self.unpack_word(cx, ins, dst, repr);
            }
            FiberOp::Fork => {
                self.operand(cx, ins, &args[0]);
                self.operand(cx, ins, &args[1]);
                ins.call(self.rt("fiber_fork"));
                self.assign(cx, ins, dst, Repr::Ref);
            }
            FiberOp::Deferred => {
                self.operand(cx, ins, &args[0]);
                ins.call(self.rt("fiber_deferred"));
                self.assign(cx, ins, dst, Repr::Ref);
            }
            FiberOp::Join => {
                self.operand(cx, ins, &args[0]);
                ins.i32_const(repr_code(dst_repr) as i32);
                ins.call(self.rt("fiber_join"));
                self.unpack_word(cx, ins, dst, dst_repr);
            }
            FiberOp::Await { .. } => {
                let repr = dst_ty.map_or(Repr::Void, |t| self.inner_repr(t));
                self.operand(cx, ins, &args[0]);
                ins.i32_const(repr_code(repr) as i32);
                ins.call(self.rt("fiber_await"));
                self.assign(cx, ins, dst, Repr::Ref);
            }
            FiberOp::Interrupt => {
                self.operand(cx, ins, &args[0]);
                ins.call(self.rt("fiber_interrupt"));
            }
            FiberOp::Attempt { .. } => {
                let repr = arg_ty(cx, 0).map_or(Repr::Void, |t| self.inner_repr(t));
                self.operand(cx, ins, &args[0]);
                ins.i32_const(repr_code(repr) as i32);
                ins.call(self.rt("fiber_attempt"));
                self.assign(cx, ins, dst, Repr::Ref);
            }
            FiberOp::Complete => {
                let repr = args
                    .get(1)
                    .map_or(Repr::Void, |o| Self::operand_repr(cx.f, o));
                self.operand(cx, ins, &args[0]);
                match args.get(1) {
                    Some(o) if repr != Repr::Void => self.word(cx, ins, o),
                    _ => {
                        ins.i64_const(0);
                    }
                }
                ins.i32_const(repr_code(repr) as i32);
                ins.call(self.rt("deferred_complete"));
                self.assign(cx, ins, dst, Repr::I32);
            }
            FiberOp::DeferredAwait => {
                self.operand(cx, ins, &args[0]);
                ins.i32_const(repr_code(dst_repr) as i32);
                ins.call(self.rt("deferred_await"));
                self.unpack_word(cx, ins, dst, dst_repr);
            }
            FiberOp::Poll => {
                let (some, none, field) =
                    dst_ty
                        .and_then(|t| self.option_parts(t))
                        .unwrap_or((0, 0, Repr::Void));
                self.operand(cx, ins, &args[0]);
                ins.i32_const(some as i32);
                ins.i32_const(none as i32);
                ins.i32_const(repr_code(field) as i32);
                ins.call(self.rt("deferred_poll"));
                self.assign(cx, ins, dst, Repr::Ref);
            }
            FiberOp::CauseKind { .. } => {
                self.operand(cx, ins, &args[0]);
                ins.call(self.rt("cause_kind"));
                self.assign(cx, ins, dst, Repr::I32);
            }
            FiberOp::CauseToString => {
                self.operand(cx, ins, &args[0]);
                ins.call(self.rt("cause_to_string"));
                self.assign(cx, ins, dst, Repr::Ref);
            }
            FiberOp::CauseRaise => {
                self.operand(cx, ins, &args[0]);
                ins.call(self.rt("cause_raise"));
            }
        }
    }

    /// The representation of the first type argument of an enum (`Exit[A, E]`, `Option[A]`) or
    /// the value of a function type.
    pub(super) fn inner_repr(&self, ty: TypeId) -> Repr {
        match self.core().ty(ty) {
            Type::Enum(_, args) => args.first().map_or(Repr::Void, |&a| self.repr(a)),
            Type::Fn(ft) => self.repr(ft.ret),
            _ => Repr::Ref,
        }
    }

    /// Pushes a value of `repr` as a word (`i64`) on the stack.
    fn word(&mut self, cx: &mut FnCtx, ins: &mut InstructionSink, o: &Operand) {
        self.operand(cx, ins, o);
        match Self::operand_repr(cx.f, o) {
            Repr::I64 => {}
            Repr::F64 => {
                ins.i64_reinterpret_f64();
            }
            _ => {
                ins.i64_extend_i32_u();
            }
        }
    }

    /// Unpacks the `i64` word on the stack into the destination of representation `repr`.
    fn unpack_word(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        repr: Repr,
    ) {
        match repr {
            Repr::Void => {
                ins.drop();
            }
            Repr::I32 | Repr::Ref => {
                if let Some(d) = dst {
                    ins.i32_wrap_i64();
                    ins.local_set(cx.local(d));
                } else {
                    ins.drop();
                }
            }
            Repr::I64 => {
                if let Some(d) = dst {
                    ins.local_set(cx.local(d));
                } else {
                    ins.drop();
                }
            }
            Repr::F64 => {
                if let Some(d) = dst {
                    ins.f64_reinterpret_i64();
                    ins.local_set(cx.local(d));
                } else {
                    ins.drop();
                }
            }
        }
    }
}

impl<'m> Builder<'m> {
    /// A call of a `suspend fn` external bound to an internal host (spec 12.6): make a task, call
    /// the host with the declared arguments and the task, then `task_end` gives the value (or
    /// suspends the caller, which the driver resumes). The host's runtime function takes the task
    /// (`i32`) last and returns nothing.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn suspend_external_call(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        g: FnId,
        module: &str,
        name: &str,
        args: &[Operand],
    ) {
        let ret = self.repr(self.m.functions[g].ret);
        let display = check::display_name(&self.m.functions[g].name).to_owned();
        let name_lit = self.string_literal(display.as_bytes());
        let task = cx.scratch(Repr::I32, 0);
        ins.i32_const(name_lit as i32);
        let tn = self.rt("task_new");
        ins.call(tn);
        ins.local_set(task);
        for a in args {
            self.operand(cx, ins, a);
        }
        ins.local_get(task);
        let host = match self.internal_host(module, name) {
            Some(i) => i,
            None => {
                self.internal(format!("suspend external `{display}` has no runtime host"));
                return;
            }
        };
        ins.call(host);
        ins.local_get(task);
        let te = self.rt("task_end");
        ins.call(te);
        self.unpack_word(cx, ins, dst, ret);
    }
}
