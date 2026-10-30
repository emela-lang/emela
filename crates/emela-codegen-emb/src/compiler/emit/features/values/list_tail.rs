//! crate::contract::ir::Value::ListTail
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
        Value::ListTail(list) => {
            emit_list_tail(layouts, function, *dst, *list, writer)?;
        }
        other => return Err(unsupported(&function.name, value_name(other))),
    };
    Ok(())
}

pub(in crate::compiler::emit) fn emit_list_tail(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    list: VarId,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let Some(dst) = dst else {
        return Ok(());
    };
    let list_ty = function.vars[list].ty;
    if !matches!(layouts.shapes.types[list_ty], Type::List(_)) || function.vars[dst].ty != list_ty {
        return Err(invalid_flow_ir(
            &function.name,
            "ListTail operand and destination types differ",
        ));
    }
    let node_type = layouts.list_node_name(list_ty);
    writer.assign(
        &variable_name(dst),
        &format!("(({node_type} *){})->tail", variable_name(list)),
    );
    Ok(())
}
