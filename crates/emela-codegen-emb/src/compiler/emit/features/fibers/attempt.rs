//! crate::contract::ir::FiberOp::Attempt
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
    cause::emit_attempt_call(layouts, function, dst, args, writer)
}

pub(in crate::compiler::emit) fn emit_suspend(
    context: &mut SuspendContext<'_, '_>,
    statement: &Stmt,
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    let next = &context.next;
    if let Stmt::Let {
        dst,
        value:
            Value::Fiber {
                op: crate::contract::ir::FiberOp::Attempt { suspends: true, .. },
                args,
                ..
            },
        ..
    } = statement
    {
        prepare_result(layouts, function, *dst, writer);
        writer.line(&format!("base->pc = {next};"));
        cause::emit_attempt_call(layouts, function, *dst, args, writer)?;
        writer.line("return;");
    } else {
        unreachable!("suspend operation entry")
    };
    Ok(())
}
