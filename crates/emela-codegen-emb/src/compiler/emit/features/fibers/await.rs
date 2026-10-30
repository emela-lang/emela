//! crate::contract::ir::FiberOp::Await
use super::super::*;
pub(in crate::compiler::emit) fn emit(
    context: &mut Context<'_, '_>,
    _dst: &Option<VarId>,
    _value: &Value,
) -> Result<(), Vec<Error>> {
    let function = context.function;
    Err(unsupported(
        &function.name,
        "Fiber::Await (phase 7 follow-up)",
    ))
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
                op:
                    op @ (crate::contract::ir::FiberOp::Join
                    | crate::contract::ir::FiberOp::Await { .. }
                    | crate::contract::ir::FiberOp::Interrupt),
                args,
                on_fail: None,
            },
        ..
    } = statement
    {
        prepare_result(layouts, function, *dst, writer);
        writer.line(&format!("frame->base.pc = {next};"));
        child::emit_wait(layouts, function, *dst, *op, args, writer)?;
        writer.line("return;");
    } else {
        unreachable!("suspend operation entry")
    };
    Ok(())
}
