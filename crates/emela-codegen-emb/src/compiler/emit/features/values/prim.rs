//! crate::contract::ir::Value::Prim
use super::super::*;
pub(in crate::compiler::emit) fn emit(
    context: &mut Context<'_, '_>,
    dst: &Option<VarId>,
    value: &Value,
) -> Result<(), Vec<Error>> {
    let function = context.function;
    let writer = &mut *context.writer;
    match value {
        Value::Prim { op, ty, args } => {
            let expression = primitive_expression(*op, *ty, args)
                .map_err(|variant| unsupported(&function.name, variant))?;
            if let Some(dst) = dst {
                writer.assign(&variable_name(*dst), &expression);
            } else {
                writer.line(&format!("(void)({expression});"));
            }
        }
        other => return Err(unsupported(&function.name, value_name(other))),
    };
    Ok(())
}
