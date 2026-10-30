//! crate::contract::ir::Value::ListIsEmpty
use super::super::*;
pub(in crate::compiler::emit) fn emit(
    context: &mut Context<'_, '_>,
    dst: &Option<VarId>,
    value: &Value,
) -> Result<(), Vec<Error>> {
    let function = context.function;
    let writer = &mut *context.writer;
    match value {
        Value::ListIsEmpty(list) => {
            let Some(dst) = dst else {
                return Ok(());
            };
            writer.assign(
                &variable_name(*dst),
                &format!(
                    "({} == NULL) ? INT32_C(1) : INT32_C(0)",
                    variable_name(*list)
                ),
            );
        }
        other => return Err(unsupported(&function.name, value_name(other))),
    };
    Ok(())
}
