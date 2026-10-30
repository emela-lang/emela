//! crate::contract::ir::Value::Tag
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
        Value::Tag(base) => {
            emit_tag(layouts, function, *dst, *base, writer)?;
        }
        other => return Err(unsupported(&function.name, value_name(other))),
    };
    Ok(())
}

pub(in crate::compiler::emit) fn emit_tag(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    base: VarId,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let Some(dst) = dst else {
        return Ok(());
    };
    let base_ty = function.vars[base].ty;
    let Type::Enum(enum_id, _) = layouts.shapes.types[base_ty] else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: tag base is not an enum",
                function.name
            ),
        }]);
    };
    let expression = if layouts.enum_is_i32(enum_id) {
        variable_name(base)
    } else if layouts.enum_fields(base_ty).is_some() {
        format!("{}.tag", variable_name(base))
    } else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: tag base has no supported enum representation",
                function.name
            ),
        }]);
    };
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}
