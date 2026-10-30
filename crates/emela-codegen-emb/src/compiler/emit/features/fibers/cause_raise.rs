//! crate::contract::ir::FiberOp::CauseRaise
use super::super::*;
pub(in crate::compiler::emit) fn emit(
    context: &mut Context<'_, '_>,
    _dst: &Option<VarId>,
    value: &Value,
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
    match value {
        Value::Fiber {
            op: crate::contract::ir::FiberOp::CauseRaise,
            args,
            on_fail: Some(edge),
        } => {
            let [cause] = args.as_slice() else {
                return Err(invalid_flow_ir(&function.name, "Cause.raise arity"));
            };
            let cause =
                operand_expression(cause).map_err(|why| unsupported(&function.name, why))?;
            writer.begin_block("");
            writer.line(&format!(
                "emela_cause *raised = emela_cause_checked({cause});"
            ));
            writer.begin_block("if (raised->kind == 0)");
            writer
                .line("EmelaError raised_error = raised->error; emela_error_retain(raised_error);");
            emit_call_failure_edge(
                layouts,
                function,
                *edge,
                "raised_error",
                writer,
                names,
                string_buffers,
                loops,
                blocks,
                protects,
                next_temp,
                next_label,
            )?;
            writer.end_block();
            writer.begin_block("else if (raised->kind == 2)");
            if layouts.runtime.has_unwind {
                writer.line("if (emela_current_fiber == NULL) emela_board_defect(EMELA_DEFECT_PANIC, \"fiber was interrupted\", UINT16_C(21));");
                writer.line("emela_force_interrupt(emela_current_fiber);");
                if !function.suspends {
                    emit_sync_interrupt_exit(
                        layouts,
                        function,
                        writer,
                        names,
                        string_buffers,
                        loops,
                        blocks,
                        protects,
                        next_temp,
                        next_label,
                    )?;
                }
            } else {
                writer.line("emela_board_defect(EMELA_DEFECT_PANIC, \"fiber was interrupted\", UINT16_C(21));");
            }
            writer.end_block();
            writer.begin_block("else");
            writer
                .line("emela_board_defect(EMELA_DEFECT_ASSERT, \"invalid Cause\", UINT16_C(13));");
            writer.end_block();
            writer.end_block();
        }
        other => return Err(unsupported(&function.name, value_name(other))),
    };
    Ok(())
}
