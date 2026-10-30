//! crate::contract::ir::Value::AsError
use super::super::*;
pub(in crate::compiler::emit) fn emit(
    context: &mut Context<'_, '_>,
    dst: &Option<VarId>,
    value: &Value,
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    match value {
        Value::AsError { error, enum_id } => {
            if !matches!(layouts.shapes.types[function.vars[*error].ty], Type::Any) {
                return Err(invalid_flow_ir(
                    &function.name,
                    "AsError operand is not an EmelaError value",
                ));
            }
            let Some(dst) = dst else {
                return Ok(());
            };
            if layouts.error_enum(function.vars[*dst].ty) != Some(*enum_id) {
                return Err(invalid_flow_ir(
                    &function.name,
                    "AsError destination does not match its error enum",
                ));
            }
            writer.assign(
                &variable_name(*dst),
                &format!(
                    "{}.payload.e{}",
                    variable_name(*error),
                    enum_index(*enum_id)
                ),
            );
        }
        other => return Err(unsupported(&function.name, value_name(other))),
    };
    Ok(())
}
