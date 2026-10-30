//! crate::contract::ir::Value::ListHead
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
        Value::ListHead(list) => {
            emit_list_head(layouts, function, *dst, *list, writer)?;
        }
        other => return Err(unsupported(&function.name, value_name(other))),
    };
    Ok(())
}

pub(in crate::compiler::emit) fn emit_list_head(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    list: VarId,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let list_ty = function.vars[list].ty;
    let Type::List(element_ty) = layouts.shapes.types[list_ty].clone() else {
        return Err(invalid_flow_ir(
            &function.name,
            "ListHead operand is not a List",
        ));
    };
    let has_repr = layouts.type_repr(element_ty) != Repr::Void;
    if dst.is_some() && !has_repr {
        return Err(invalid_flow_ir(
            &function.name,
            "ListHead destination does not match its element representation",
        ));
    }
    if let Some(dst) = dst {
        let node_type = layouts.list_node_name(list_ty);
        writer.assign(
            &variable_name(dst),
            &format!("(({node_type} *){})->head", variable_name(list)),
        );
    }
    Ok(())
}
