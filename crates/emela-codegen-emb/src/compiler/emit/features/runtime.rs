use super::*;
pub(in crate::compiler::emit) fn emit_show_string(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    let string_buffers = context.string_buffers;
    if args.len() != 1 || !operand_is_string(layouts, function, &args[0]) {
        return Err(invalid_flow_ir(
            &function.name,
            "RuntimeOp::ShowString must have one String argument",
        ));
    }
    let Some(dst) = dst else {
        return Err(invalid_flow_ir(
            &function.name,
            "RuntimeOp::ShowString has no String destination",
        ));
    };
    if !matches!(layouts.shapes.types[function.vars[dst].ty], Type::String) {
        return Err(invalid_flow_ir(
            &function.name,
            "RuntimeOp::ShowString destination is not String",
        ));
    }
    let expression = match &args[0] {
        Operand::Const(Const::String(value)) => {
            constant_expression(&Const::String(show_string(value)))
                .map_err(|variant| unsupported(&function.name, variant))?
                .ok_or_else(|| {
                    invalid_flow_ir(&function.name, "RuntimeOp::ShowString has no C expression")
                })?
        }
        operand => {
            let input = operand_expression(operand)
                .map_err(|variant| unsupported(&function.name, variant))?;
            let Some((buffer, capacity)) = string_buffers.buffer(dst) else {
                return Err(invalid_flow_ir(
                    &function.name,
                    "dynamic RuntimeOp::ShowString has no planned output buffer",
                ));
            };
            format!("emela_emb_string_show({input}, {buffer}, {capacity})")
        }
    };
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}
pub(in crate::compiler::emit) fn debug_print(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    {
        if args.len() != 1 || !operand_is_string(layouts, function, &args[0]) {
            return Err(invalid_flow_ir(
                &function.name,
                "RuntimeOp::DebugPrint must have one String argument",
            ));
        }
        if dst.is_some() {
            return Err(invalid_flow_ir(
                &function.name,
                "RuntimeOp::DebugPrint has a result destination",
            ));
        }
        let value =
            operand_expression(&args[0]).map_err(|variant| unsupported(&function.name, variant))?;
        writer.line(&format!(
            "emela_board_debug(({value}).data, ({value}).length);"
        ));
        Ok(())
    }
}
pub(in crate::compiler::emit) fn retire(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    {
        if dst.is_some() || args.len() != 2 {
            return Err(invalid_flow_ir(
                &function.name,
                "RuntimeOp::Retire must have two arguments and no destination",
            ));
        }
        let Operand::Var(handler) = args[0] else {
            return Err(invalid_flow_ir(
                &function.name,
                "RuntimeOp::Retire handler is not a variable",
            ));
        };
        let Type::Handler(effect) = layouts.shapes.types[function.vars[handler].ty] else {
            return Err(invalid_flow_ir(
                &function.name,
                "RuntimeOp::Retire argument is not a Handler value",
            ));
        };
        if operand_type(function, &args[1]) != Some(TypeId::STRING) {
            return Err(invalid_flow_ir(
                &function.name,
                "RuntimeOp::Retire name is not a String",
            ));
        }
        let name =
            operand_expression(&args[1]).map_err(|variant| unsupported(&function.name, variant))?;
        if !layouts.shapes.handler_layout_ids.contains_key(&effect) {
            return Err(unsupported(
                &function.name,
                "RuntimeOp::Retire without a runtime handler layout",
            ));
        }
        let expression = variable_name(handler);
        let pointer = format!(
            "(({} *){expression})",
            crate::semantics::abi::handler_type_name(effect)
        );
        writer.begin_block(&format!("if ({expression} == NULL)"));
        writer.line(
            "emela_board_defect(EMELA_DEFECT_PANIC, \"invalid handler release\", UINT16_C(24));",
        );
        writer.end_block();
        writer.assign(&format!("{pointer}->active"), "INT32_C(0)");
        writer.line(&format!("(void)({name});"));
        Ok(())
    }
}
pub(in crate::compiler::emit) fn defect(
    context: &mut BuiltinContext<'_, '_>,
    _dst: Option<VarId>,
    _args: &[Operand],
) -> Result<(), Vec<Error>> {
    let function = context.function;
    Err(unsupported(&function.name, "RuntimeOp::Defect"))
}
pub(in crate::compiler::emit) fn defect_take(
    context: &mut BuiltinContext<'_, '_>,
    _dst: Option<VarId>,
    _args: &[Operand],
) -> Result<(), Vec<Error>> {
    let function = context.function;
    Err(unsupported(&function.name, "RuntimeOp::DefectTake"))
}
pub(in crate::compiler::emit) fn defect_resume(
    context: &mut BuiltinContext<'_, '_>,
    _dst: Option<VarId>,
    _args: &[Operand],
) -> Result<(), Vec<Error>> {
    let function = context.function;
    Err(unsupported(&function.name, "RuntimeOp::DefectResume"))
}
pub(in crate::compiler::emit) fn hold(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    let op = RuntimeOp::Hold;
    {
        if dst.is_some() || !args.is_empty() {
            return Err(invalid_flow_ir(
                &function.name,
                "Hold/Unhold arity or result",
            ));
        }
        if layouts.runtime.has_unwind {
            let operation = if op == RuntimeOp::Hold {
                "hold"
            } else {
                "unhold"
            };
            writer.line(&format!(
                "if (emela_current_fiber != NULL) emela_interrupt_{operation}(emela_current_fiber);"
            ));
        }
        Ok(())
    }
}
pub(in crate::compiler::emit) fn unhold(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    let op = RuntimeOp::Unhold;
    {
        if dst.is_some() || !args.is_empty() {
            return Err(invalid_flow_ir(
                &function.name,
                "Hold/Unhold arity or result",
            ));
        }
        if layouts.runtime.has_unwind {
            let operation = if op == RuntimeOp::Hold {
                "hold"
            } else {
                "unhold"
            };
            writer.line(&format!(
                "if (emela_current_fiber != NULL) emela_interrupt_{operation}(emela_current_fiber);"
            ));
        }
        Ok(())
    }
}
pub(in crate::compiler::emit) fn yield_operation(
    context: &mut BuiltinContext<'_, '_>,
    _dst: Option<VarId>,
    _args: &[Operand],
) -> Result<(), Vec<Error>> {
    let function = context.function;
    Err(unsupported(&function.name, "RuntimeOp::Yield"))
}
pub(in crate::compiler::emit) fn yield_perform(
    context: &mut BuiltinContext<'_, '_>,
    _dst: Option<VarId>,
    _args: &[Operand],
) -> Result<(), Vec<Error>> {
    let function = context.function;
    Err(unsupported(&function.name, "RuntimeOp::YieldPerform"))
}
pub(in crate::compiler::emit) fn emit(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    op: RuntimeOp,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    match op {
        RuntimeOp::ShowString => emit_show_string(context, dst, args),
        RuntimeOp::DebugPrint => debug_print(context, dst, args),
        RuntimeOp::Retire => retire(context, dst, args),
        RuntimeOp::DefectAt => defect_at(context, dst, args),
        RuntimeOp::Defect => defect(context, dst, args),
        RuntimeOp::DefectTake => defect_take(context, dst, args),
        RuntimeOp::DefectResume => defect_resume(context, dst, args),
        RuntimeOp::Hold => hold(context, dst, args),
        RuntimeOp::Unhold => unhold(context, dst, args),
        RuntimeOp::Yield => yield_operation(context, dst, args),
        RuntimeOp::YieldPerform => yield_perform(context, dst, args),
    }
}

pub(in crate::compiler::emit) fn emit_string_show_helpers(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED void emela_emb_string_show_put(char *buffer, uint16_t capacity, uint32_t *length, uint8_t byte)",
    );
    writer.begin_block("if (*length >= (uint32_t)capacity)");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"string capacity exceeded\", UINT16_C(24));",
    );
    writer.end_block();
    writer.line("buffer[*length] = (char)byte;");
    writer.line("++*length;");
    writer.end_block();
    writer.blank();

    writer.begin_block(
        "static inline EMELA_UNUSED EmelaString emela_emb_string_show(EmelaString value, char *buffer, uint16_t capacity)",
    );
    writer.line("uint32_t length = UINT32_C(0);");
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, UINT8_C(0x22));");
    writer.begin_block("for (uint32_t i = UINT32_C(0); i < (uint32_t)value.length; ++i)");
    writer.line("const uint8_t byte = (uint8_t)value.data[i];");
    writer.begin_block("if (byte == UINT8_C(0x22) || byte == UINT8_C(0x5c))");
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, UINT8_C(0x5c));");
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, byte);");
    writer.end_block();
    writer.begin_block(
        "else if (byte == UINT8_C(0x23) && i + UINT32_C(1) < (uint32_t)value.length && value.data[i + UINT32_C(1)] == '{')",
    );
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, UINT8_C(0x5c));");
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, byte);");
    writer.end_block();
    writer.begin_block(
        "else if (byte == UINT8_C(0x0a) || byte == UINT8_C(0x09) || byte == UINT8_C(0x0d))",
    );
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, UINT8_C(0x5c));");
    writer.line("const uint8_t escaped = byte == UINT8_C(0x0a) ? UINT8_C(0x6e) : (byte == UINT8_C(0x09) ? UINT8_C(0x74) : UINT8_C(0x72));");
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, escaped);");
    writer.end_block();
    writer.begin_block("else if (byte < UINT8_C(0x20) || byte == UINT8_C(0x7f))");
    writer.line("static const char digits[] = \"0123456789abcdef\";");
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, UINT8_C(0x5c));");
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, UINT8_C(0x75));");
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, UINT8_C(0x7b));");
    writer.begin_block("if (byte >= UINT8_C(0x10))");
    writer
        .line("emela_emb_string_show_put(buffer, capacity, &length, (uint8_t)digits[byte >> 4]);");
    writer.end_block();
    writer.line(
        "emela_emb_string_show_put(buffer, capacity, &length, (uint8_t)digits[byte & UINT8_C(0x0f)]);",
    );
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, UINT8_C(0x7d));");
    writer.end_block();
    writer.begin_block("else");
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, byte);");
    writer.end_block();
    writer.end_block();
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, UINT8_C(0x22));");
    writer.line("return (EmelaString){ .data = buffer, .length = (uint16_t)length };");
    writer.end_block();
    writer.blank();
}

use crate::semantics::strings::show_string;

pub(in crate::compiler::emit) fn defect_at(
    context: &mut BuiltinContext<'_, '_>,
    _dst: Option<VarId>,
    _args: &[Operand],
) -> Result<(), Vec<Error>> {
    Err(unsupported(&context.function.name, "RuntimeOp::DefectAt"))
}
