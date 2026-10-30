//! crate::contract::ir::Value::Const
use super::super::*;
pub(in crate::compiler::emit) fn emit(
    context: &mut Context<'_, '_>,
    dst: &Option<VarId>,
    value: &Value,
) -> Result<(), Vec<Error>> {
    let function = context.function;
    let writer = &mut *context.writer;
    match value {
        Value::Const(value) => {
            let expression = constant_expression(value)
                .map_err(|kind| unsupported(&function.name, &format!("{kind} in Value::Const")))?;
            match (dst, expression) {
                (Some(dst), Some(expression)) => {
                    writer.assign(&variable_name(*dst), &expression);
                }
                (None, None) => {}
                (Some(_), None) => {
                    return Err(vec![Error {
                        kind: ErrorKind::InvalidFlowIr,
                        message: format!(
                            "invalid Flow IR in function `{}`: Unit constant has a destination",
                            function.name
                        ),
                    }]);
                }
                (None, Some(_)) => {}
            }
        }
        other => return Err(unsupported(&function.name, value_name(other))),
    };
    Ok(())
}
