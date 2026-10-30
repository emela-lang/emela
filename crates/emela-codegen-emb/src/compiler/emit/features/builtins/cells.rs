use super::super::builtin_utilities as utilities;
use super::super::*;

pub(in crate::compiler::emit) fn cell_new(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Emission {
    utilities::cell_arity(context, args, 1)?;
    let BuiltinContext {
        layouts,
        function,
        writer,
        ..
    } = context;
    let content = operand_type(function, &args[0]).expect("Cell argument has a type");
    let cell_ty =
        if let Some(cell) = dst {
            let ty = function.vars[cell].ty;
            if !matches!(layouts.shapes.types[ty], Type::Cell(field) if field == content) {
                return Err(invalid_flow_ir(
                    &function.name,
                    "CellNew destination or initial value has the wrong type",
                ));
            }
            ty
        } else {
            *layouts.shapes.cell_types
            .iter()
            .find(|&&ty| matches!(layouts.shapes.types[ty], Type::Cell(field) if field == content))
            .expect("CellNew layout was collected")
        };
    let target = dst
        .map(variable_name)
        .unwrap_or_else(|| "emela_discarded_cell".into());
    if dst.is_none() {
        writer.begin_block("");
        writer.decl("void *", &target, None);
    }
    let storage_ty = layouts.cell_content_c_type(content);
    writer.assign(
        &target,
        &format!(
            "emela_heap_allocate(sizeof({storage_ty}), UINT32_C({}))",
            layouts.cell_kind(cell_ty)
        ),
    );
    if layouts.type_repr(content) != Repr::Void {
        let source = operand_expression(&args[0]).map_err(|v| unsupported(&function.name, v))?;
        let storage = format!("(*(({storage_ty} *){target}))");
        layouts.emit_owned_value_store(content, &source, &storage, writer);
    }
    if dst.is_none() {
        writer.line(&format!("emela_heap_release({target});"));
        writer.end_block();
    }
    Ok(())
}

pub(in crate::compiler::emit) fn cell_get(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Emission {
    utilities::cell_arity(context, args, 1)?;
    let storage = utilities::cell_storage(context, &args[0])?;
    match dst {
        Some(result) if context.function.vars[result].ty == storage.content => {
            context.layouts.emit_owned_value_store(
                storage.content,
                &storage.expression,
                &variable_name(result),
                context.writer,
            );
        }
        None => {}
        _ => {
            return Err(invalid_flow_ir(
                &context.function.name,
                "CellGet destination does not match the cell content type",
            ));
        }
    }
    Ok(())
}

pub(in crate::compiler::emit) fn cell_set(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Emission {
    utilities::cell_arity(context, args, 2)?;
    let storage = utilities::cell_storage(context, &args[0])?;
    let BuiltinContext {
        layouts,
        function,
        writer,
        ..
    } = context;
    if dst.is_some() || operand_type(function, &args[1]) != Some(storage.content) {
        return Err(invalid_flow_ir(
            &function.name,
            "CellSet destination or value type is invalid",
        ));
    }
    if layouts.type_repr(storage.content) != Repr::Void {
        let source = operand_expression(&args[1]).map_err(|v| unsupported(&function.name, v))?;
        writer.begin_block("");
        writer.decl(&storage.c_type, "emela_cell_new_value", Some(&source));
        layouts.emit_own_value_in_place(storage.content, "emela_cell_new_value", writer);
        if layouts.is_managed_value(storage.content) {
            writer.decl(
                &storage.c_type,
                "emela_cell_old_value",
                Some(&storage.expression),
            );
        }
        // Publish before release, so a collector sees the new edge on self-assignment.
        writer.assign(&storage.expression, "emela_cell_new_value");
        layouts.emit_release_value_expression(storage.content, "emela_cell_old_value", writer);
        writer.end_block();
    }
    Ok(())
}
