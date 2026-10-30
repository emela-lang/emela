//! crate::contract::ir::Value::Closure
use super::super::*;
pub(in crate::compiler::emit) fn emit(
    context: &mut Context<'_, '_>,
    _dst: &Option<VarId>,
    value: &Value,
) -> Result<(), Vec<Error>> {
    let function = context.function;
    Err(unsupported(&function.name, value_name(value)))
}
