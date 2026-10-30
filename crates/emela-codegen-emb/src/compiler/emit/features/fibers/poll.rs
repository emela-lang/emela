//! crate::contract::ir::FiberOp::Poll
use super::super::*;
pub(in crate::compiler::emit) fn emit(
    context: &mut Context<'_, '_>,
    dst: &Option<VarId>,
    value: &Value,
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    let Value::Fiber { args, .. } = value else {
        unreachable!("Fiber entry")
    };
    let dst = *dst;
    deferred::emit_poll(layouts, function, dst, args, writer)
}
