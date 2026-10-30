//! crate::contract::ir::Value::Coerce
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
        Value::Coerce(source) => {
            emit_coerce(layouts, function, *dst, *source, writer)?;
        }
        other => return Err(unsupported(&function.name, value_name(other))),
    };
    Ok(())
}

pub(in crate::compiler::emit) fn emit_coerce(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    source: VarId,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let Some(dst) = dst else {
        return Ok(());
    };
    let source_ty = function.vars[source].ty;
    let destination_ty = function.vars[dst].ty;
    let source_name = variable_name(source);
    let destination_name = variable_name(dst);
    let source_is_any = matches!(layouts.shapes.types[source_ty], Type::Any);
    let destination_is_any = matches!(layouts.shapes.types[destination_ty], Type::Any);

    if (destination_is_any && layouts.is_runtime_handle(source_ty))
        || (source_is_any && layouts.is_runtime_handle(destination_ty))
    {
        if destination_is_any {
            writer.assign(
                &destination_name,
                &format!("(EmelaAny){{.payload.any_box = {source_name}}}"),
            );
        } else {
            let kind = if layouts.is_named_external(destination_ty, "Std.Fiber.Fiber") {
                layouts.child_kind()
            } else if layouts.is_cause(destination_ty) {
                layouts.cause_kind()
            } else {
                layouts.deferred_kind()
            };
            writer.line(&format!("if ({source_name}.tag != 0 || {source_name}.payload.any_box == NULL || emela_heap_header({source_name}.payload.any_box)->kind != UINT32_C({kind})) emela_scheduler_invalid();"));
            writer.assign(&destination_name, &format!("{source_name}.payload.any_box"));
        }
        layouts.emit_retain_value_expression(destination_ty, &destination_name, writer);
        return Ok(());
    }

    if matches!(layouts.shapes.types[source_ty], Type::Fn(_))
        && matches!(layouts.shapes.types[destination_ty], Type::Fn(_))
    {
        if let Some(&adapter) = layouts
            .shapes
            .function_adapter_ids
            .get(&(source_ty, destination_ty))
        {
            let environment = format!("emela_fn_coerce_env_for_var_{}", u32::from(dst.into_raw()));
            let kind = layouts.function_adapter_kind(adapter);
            writer.decl(
                &format!("emela_fn_coerce_env_{adapter} *"),
                &environment,
                Some(&format!(
                    "(emela_fn_coerce_env_{adapter} *)emela_heap_allocate(sizeof(emela_fn_coerce_env_{adapter}), UINT32_C({kind}))"
                )),
            );
            if layouts.is_managed_value(source_ty) {
                layouts.emit_retain_value_expression(source_ty, &source_name, writer);
            }
            writer.assign(&format!("{environment}->source"), &source_name);
            writer.assign(
                &format!("{destination_name}.call"),
                &function_coerce_adapter_name(adapter),
            );
            writer.assign(&format!("{destination_name}.env"), &environment);
        } else {
            writer.assign(
                &format!("{destination_name}.call"),
                &format!("{source_name}.call"),
            );
            writer.assign(
                &format!("{destination_name}.env"),
                &format!("{source_name}.env"),
            );
            if layouts.is_managed_value(source_ty) {
                layouts.emit_retain_value_expression(source_ty, &source_name, writer);
            }
        }
        return Ok(());
    }

    if destination_is_any && source_is_any {
        writer.assign(&destination_name, &source_name);
        layouts.emit_retain_value_expression(destination_ty, &destination_name, writer);
        return Ok(());
    }
    if destination_is_any {
        let Some(&box_type) = layouts.shapes.any_box_ids.get(&source_ty) else {
            return Err(unsupported(
                &function.name,
                "boxing an Any value without a concrete runtime layout",
            ));
        };
        let box_name = format!("emela_any_box_for_var_{}", u32::from(dst.into_raw()));
        let kind = layouts.any_box_kind();
        writer.decl(
            "emela_any_box *",
            &box_name,
            Some(&format!(
                "(emela_any_box *)emela_heap_allocate(sizeof(emela_any_box), UINT32_C({kind}))"
            )),
        );
        writer.assign(
            &format!("{box_name}->type"),
            &format!("UINT32_C({box_type})"),
        );
        layouts.emit_owned_value_store(
            source_ty,
            &source_name,
            &format!("{box_name}->payload.v{box_type}"),
            writer,
        );
        writer.assign(&format!("{destination_name}.tag"), "UINT32_C(0)");
        writer.assign(&format!("{destination_name}.payload.any_box"), &box_name);
        return Ok(());
    }
    if source_is_any {
        let Some(&box_type) = layouts.shapes.any_box_ids.get(&destination_ty) else {
            return Err(unsupported(
                &function.name,
                "unboxing an Any value without a concrete runtime layout",
            ));
        };
        let box_name = format!("((emela_any_box *){source_name}.payload.any_box)");
        writer.begin_block(&format!(
            "if ({source_name}.tag != UINT32_C(0) || {source_name}.payload.any_box == NULL)"
        ));
        writer.line("emela_board_defect(EMELA_DEFECT_PANIC, \"invalid Any unbox\", UINT16_C(16));");
        writer.end_block();
        writer.begin_block(&format!("if ({box_name}->type != UINT32_C({box_type}))"));
        writer.line("emela_board_defect(EMELA_DEFECT_PANIC, \"Any type mismatch\", UINT16_C(17));");
        writer.end_block();
        writer.assign(
            &destination_name,
            &format!("{box_name}->payload.v{box_type}"),
        );
        if layouts.is_managed_value(destination_ty) {
            layouts.emit_retain_value_expression(destination_ty, &destination_name, writer);
        }
        return Ok(());
    }

    if layouts.c_type(source_ty) == layouts.c_type(destination_ty) {
        writer.assign(&destination_name, &source_name);
        if layouts.is_managed_value(destination_ty) {
            layouts.emit_retain_value_expression(destination_ty, &destination_name, writer);
        }
        return Ok(());
    }
    Err(unsupported(
        &function.name,
        "function-value coercion requiring a runtime adapter",
    ))
}
