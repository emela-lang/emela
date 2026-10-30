//! crate::contract::ir::Term::Yield
use super::super::*;
pub(in crate::compiler::emit) fn emit(
    context: &mut Context<'_, '_>,
    term: &Term,
    dst: Option<VarId>,
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    let Term::Yield(value) = term else {
        unreachable!("terminal dispatch")
    };
    match (dst, value) {
        (Some(dst), Some(value)) => {
            emit_retain_operand(layouts, function, value, writer);
            let expression = operand_expression(value)
                .map_err(|variant| unsupported(&function.name, variant))?;
            writer.assign(&variable_name(dst), &expression);
            if layouts.is_managed_value(function.vars[dst].ty) {
                writer.assign(&owned_variable_name(dst), "INT32_C(1)");
            }
        }
        (Some(_), None) => {
            return Err(invalid_flow_ir(
                &function.name,
                "compound statement destination has a valueless arm",
            ));
        }
        (None, _) => {}
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
