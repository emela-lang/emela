//! crate::contract::ir::Value::List
use super::super::*;
pub(in crate::compiler::emit) fn emit(
    context: &mut Context<'_, '_>,
    dst: &Option<VarId>,
    value: &Value,
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    let next_temp = &mut *context.next_temp;
    match value {
        Value::List { elems, tail } => emit_list_value(
            layouts,
            function,
            *dst,
            elems,
            tail.as_ref(),
            writer,
            next_temp,
        )?,
        other => return Err(unsupported(&function.name, value_name(other))),
    };
    Ok(())
}

pub(in crate::compiler::emit) fn emit_list_value(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    elems: &[Operand],
    tail: Option<&Operand>,
    writer: &mut Writer,
    next_temp: &mut usize,
) -> Result<(), Vec<Error>> {
    let Some(dst) = dst else {
        return Err(invalid_flow_ir(
            &function.name,
            "Value::List has no destination",
        ));
    };
    let list_ty = function.vars[dst].ty;
    let Type::List(element_ty) = layouts.shapes.types[list_ty].clone() else {
        return Err(invalid_flow_ir(
            &function.name,
            "Value::List destination is not a List",
        ));
    };
    if elems.is_empty() {
        let expression = match tail {
            Some(tail) => {
                operand_expression(tail).map_err(|variant| unsupported(&function.name, variant))?
            }
            None => "NULL".to_owned(),
        };
        writer.assign(&variable_name(dst), &expression);
        return Ok(());
    }

    let list_name = layouts.list_node_name(list_ty);
    let list_kind = layouts.list_index(list_ty);
    let temp = format!("emela_list_build_{}", *next_temp);
    *next_temp += 1;
    let node_temp = format!("emela_list_node_tmp_{}", *next_temp);
    *next_temp += 1;
    let initial_tail = match tail {
        Some(tail) => {
            operand_expression(tail).map_err(|variant| unsupported(&function.name, variant))?
        }
        None => "NULL".to_owned(),
    };
    writer.decl("void *", &temp, Some(&initial_tail));
    writer.decl("void *", &node_temp, None);
    if tail.is_some() {
        writer.line(&format!("emela_heap_retain({temp});"));
    }
    for elem in elems.iter().rev() {
        writer.assign(
            &node_temp,
            &format!(
                "({list_name} *)emela_heap_allocate(sizeof({list_name}), UINT32_C({list_kind}))"
            ),
        );
        if layouts.type_repr(element_ty) == Repr::Void {
            writer.assign(
                &format!("(({list_name} *){node_temp})->_unit"),
                "UINT8_C(0)",
            );
        } else {
            let expression =
                operand_expression(elem).map_err(|variant| unsupported(&function.name, variant))?;
            layouts.emit_owned_value_store(
                element_ty,
                &expression,
                &format!("(({list_name} *){node_temp})->head"),
                writer,
            );
        }
        writer.assign(&format!("(({list_name} *){node_temp})->tail"), &temp);
        writer.assign(&temp, &node_temp);
    }
    writer.assign(&variable_name(dst), &temp);
    Ok(())
}
