//! crate::contract::ir::Term::Break
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
    let Term::Break { label, value } = term else {
        unreachable!("terminal dispatch")
    };
    {
        let Some(target_index) = blocks.iter().rposition(|target| target.label == *label) else {
            return Err(invalid_flow_ir(
                &function.name,
                "Break target is not an enclosing emitted block",
            ));
        };
        let target_dst = blocks[target_index].dst;
        let exit = blocks[target_index].exit.clone();
        let protect_depth = blocks[target_index].protect_depth;
        match (target_dst, value) {
            (Some(dst), Some(value)) => {
                if function.vars[dst].ty == TypeId::ANY
                    && operand_type(function, value)
                        .is_some_and(|ty| layouts.error_enum(ty).is_some())
                {
                    emit_release_if_owned(layouts, function, dst, writer);
                    emit_error_assignment(layouts, function, value, &variable_name(dst), writer)?;
                } else {
                    emit_retain_operand(layouts, function, value, writer);
                    emit_release_if_owned(layouts, function, dst, writer);
                    let expression = operand_expression(value)
                        .map_err(|variant| unsupported(&function.name, variant))?;
                    writer.assign(&variable_name(dst), &expression);
                }
                if layouts.is_managed_value(function.vars[dst].ty) {
                    writer.assign(&owned_variable_name(dst), "INT32_C(1)");
                }
            }
            (Some(_), None) => {
                return Err(invalid_flow_ir(
                    &function.name,
                    "Break omits the enclosing block value",
                ));
            }
            (None, _) => {}
        }
        emit_pending_cleanups(
            layouts,
            function,
            protect_depth,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        )?;
        blocks[target_index].used = true;
        writer.line(&format!("goto {exit};"));
    };
    Ok(())
}

pub(in crate::compiler::emit) fn emit_function(
    _layouts: &TypeLayouts<'_>,
    function: &Function,
    term: &Term,
    _string_buffers: &StringBufferPlan,
    _writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    Err(unsupported(&function.name, term_name(term)))
}
