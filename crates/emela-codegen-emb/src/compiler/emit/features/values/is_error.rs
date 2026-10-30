//! crate::contract::ir::Value::IsError
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
        Value::IsError { error, enum_id } => {
            if !matches!(layouts.shapes.types[function.vars[*error].ty], Type::Any) {
                return Err(invalid_flow_ir(
                    &function.name,
                    "IsError operand is not an EmelaError value",
                ));
            }
            let Some(dst) = dst else {
                return Ok(());
            };
            if !layouts.shapes.core.enums[*enum_id].error {
                return Err(invalid_flow_ir(
                    &function.name,
                    "IsError target is not an error enum",
                ));
            }
            writer.assign(
                &variable_name(*dst),
                &format!(
                    "({}).tag == UINT32_C({}) ? INT32_C(1) : INT32_C(0)",
                    variable_name(*error),
                    enum_index(*enum_id) + 1
                ),
            );
        }
        other => return Err(unsupported(&function.name, value_name(other))),
    };
    Ok(())
}
