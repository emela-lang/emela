use super::*;

pub(in crate::compiler::emit) fn emit_assignment(
    context: &mut SuspendContext<'_, '_>,
    node: &Node,
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    let Node::Assign(values, next) = node else {
        unreachable!("frame state dispatch")
    };
    {
        for (index, (dst, value)) in values.iter().enumerate() {
            if function.vars[*dst].ty == TypeId::ANY
                && operand_type(function, value).is_some_and(|ty| layouts.error_enum(ty).is_some())
            {
                writer.decl("EmelaAny", &format!("next_{index}"), Some("{0}"));
                emit_error_assignment(layouts, function, value, &format!("next_{index}"), writer)?;
                continue;
            }
            writer.decl(
                &layouts.c_type(function.vars[*dst].ty),
                &format!("next_{index}"),
                Some(&operand_expression(value).map_err(|why| unsupported(&function.name, why))?),
            );
            retain_frame_value(
                layouts,
                function.vars[*dst].ty,
                &format!("next_{index}"),
                writer,
            );
        }
        for (index, (dst, _)) in values.iter().enumerate() {
            emit_release_if_owned(layouts, function, *dst, writer);
            writer.assign(&variable_name(*dst), &format!("next_{index}"));
            if layouts.is_managed_value(function.vars[*dst].ty) {
                writer.assign(&owned_variable_name(*dst), "INT32_C(1)");
            }
        }
        writer.line(&format!("frame->base.pc = {next}; continue;"));
    };
    Ok(())
}
