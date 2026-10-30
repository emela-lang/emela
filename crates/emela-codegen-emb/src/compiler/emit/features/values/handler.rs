//! crate::contract::ir::Value::Handler
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
        Value::Handler { name, effect, ops } => {
            emit_handler_value(layouts, function, *dst, *effect, name, ops, writer)?;
        }
        other => return Err(unsupported(&function.name, value_name(other))),
    };
    Ok(())
}

pub(in crate::compiler::emit) fn emit_handler_value(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    effect: EffectId,
    name: &str,
    operations: &[VarId],
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let Some(dst) = dst else {
        return Err(invalid_flow_ir(
            &function.name,
            "Handler has no destination",
        ));
    };
    if layouts.shapes.types[function.vars[dst].ty] != Type::Handler(effect) {
        return Err(invalid_flow_ir(
            &function.name,
            "Handler destination does not match its effect",
        ));
    }
    let Some(&layout_index) = layouts.shapes.handler_layout_ids.get(&effect) else {
        return Err(invalid_flow_ir(
            &function.name,
            "Handler has no runtime layout",
        ));
    };
    let layout = &layouts.shapes.handler_layouts[layout_index];
    if operations.len() != layout.operations.len()
        || operations
            .iter()
            .zip(&layout.operations)
            .any(|(&operation, &ty)| function.vars[operation].ty != ty)
    {
        return Err(invalid_flow_ir(
            &function.name,
            "Handler operation types do not match its runtime layout",
        ));
    }
    let handler_type = crate::semantics::abi::handler_type_name(effect);
    let destination = variable_name(dst);
    let kind = layouts.handler_kind(effect);
    writer.assign(
        &destination,
        &format!("emela_heap_allocate(sizeof({handler_type}), UINT32_C({kind}))"),
    );
    let target = format!("(({handler_type} *){destination})");
    writer.assign(&format!("{target}->active"), "INT32_C(1)");
    let message = format!("handler {name} was used after release");
    writer.assign(
        &format!("{target}->released_message"),
        &c_string_literal(message.as_bytes()),
    );
    writer.assign(
        &format!("{target}->released_message_length"),
        &format!("UINT16_C({})", message.len()),
    );
    for (index, &operation) in operations.iter().enumerate() {
        if layouts.shapes.core.effects[effect].ops[index].suspend {
            let message = format!(
                "resume called twice in operation `{}` of handler `{name}`",
                layouts.shapes.core.effects[effect].ops[index].name
            );
            writer.assign(
                &format!("{target}->resume_error_{index}"),
                &c_string_literal(message.as_bytes()),
            );
            writer.assign(
                &format!("{target}->resume_error_length_{index}"),
                &format!("UINT16_C({})", message.len()),
            );
        }
        if layouts.is_managed_value(function.vars[operation].ty) {
            emit_retain_operand(layouts, function, &Operand::Var(operation), writer);
        }
        writer.assign(&format!("{target}->op_{index}"), &variable_name(operation));
    }
    Ok(())
}
