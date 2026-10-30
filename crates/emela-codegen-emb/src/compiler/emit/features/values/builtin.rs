//! crate::contract::ir::Value::Builtin
use super::super::*;
pub(in crate::compiler::emit) fn emit(
    context: &mut Context<'_, '_>,
    dst: &Option<VarId>,
    value: &Value,
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    let string_buffers = context.string_buffers;
    match value {
        Value::Builtin { op, args } => {
            emit_builtin(layouts, function, *dst, *op, args, string_buffers, writer)?;
        }
        other => return Err(unsupported(&function.name, value_name(other))),
    };
    Ok(())
}
