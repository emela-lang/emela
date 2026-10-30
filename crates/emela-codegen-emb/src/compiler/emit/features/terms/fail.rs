//! crate::contract::ir::Term::Fail
use super::super::*;
pub(in crate::compiler::emit) fn emit(
    context: &mut Context<'_, '_>,
    term: &Term,
    _dst: Option<VarId>,
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    let names = context.names;
    let string_buffers = context.string_buffers;
    let loops = &mut *context.loops;
    let blocks = &mut *context.blocks;
    let protects = &mut *context.protects;
    let next_temp = &mut *context.next_temp;
    let next_label = &mut *context.next_label;
    let Term::Fail(error) = term else {
        unreachable!("terminal dispatch")
    };
    {
        if !function.fails {
            return Err(invalid_flow_ir(
                &function.name,
                "Fail appears in a function without a failure channel",
            ));
        }
        emit_error_assignment(layouts, function, error, "*emela_error_out", writer)?;
        emit_pending_cleanups(
            layouts,
            function,
            0,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        )?;
        emit_failure_return(layouts, function, writer);
    };
    Ok(())
}

pub(in crate::compiler::emit) fn emit_function(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    term: &Term,
    _string_buffers: &StringBufferPlan,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    match term {
        Term::Fail(error) => {
            if !function.fails {
                return Err(invalid_flow_ir(
                    &function.name,
                    "Fail appears in a function without a failure channel",
                ));
            }
            emit_error_assignment(layouts, function, error, "*emela_error_out", writer)?;
            emit_failure_return(layouts, function, writer);
        }
        _ => unreachable!("function terminal"),
    };
    Ok(())
}

pub(in crate::compiler::emit) fn emit_suspend(
    context: &mut SuspendContext<'_, '_>,
    node: &Node,
) -> Result<(), Vec<Error>> {
    let writer = &mut *context.writer;
    let Node::Fail(slot) = node else {
        unreachable!("frame state dispatch")
    };
    {
        writer . line ("if (frame->error_out == NULL) emela_board_defect(EMELA_DEFECT_UNHANDLED_ERROR, \"unhandled error\", UINT16_C(15));") ;
        writer.line(&format!(
            "*frame->error_out = frame->pending_error[{slot}];"
        ));
        writer.line(&format!(
            "frame->pending_error[{slot}] = (EmelaError){{0}};"
        ));
        writer.line("emela_frame_return(fiber); return;");
    };
    Ok(())
}

pub(in crate::compiler::emit) fn emit_set_error(
    context: &mut SuspendContext<'_, '_>,
    node: &Node,
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    let Node::SetError(error, next, slot) = node else {
        unreachable!("frame state dispatch")
    };
    {
        writer.line(&format!(
            "emela_error_clear(&frame->pending_error[{slot}]);"
        ));
        emit_error_assignment(
            layouts,
            function,
            error,
            &format!("frame->pending_error[{slot}]"),
            writer,
        )?;
        writer.line(&format!("base->pc = {next}; continue;"));
    };
    Ok(())
}
