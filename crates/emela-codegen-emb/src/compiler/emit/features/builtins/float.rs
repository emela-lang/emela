use super::super::*;
pub(in crate::compiler::emit) fn float_from_int(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::FloatFromInt;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        1,
        ResultType::Exact(Type::Float),
        |layouts, function, args| {
            args.iter()
                .all(|operand| operand_is_int(layouts, function, operand))
        },
    )?;
    let writer = &mut *context.writer;
    let expression = format!("((double)({}))", expressions[0]);
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

pub(in crate::compiler::emit) fn float_floor(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::FloatFloor;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        1,
        ResultType::Exact(Type::Int),
        |layouts, function, args| operand_is_float(layouts, function, &args[0]),
    )?;
    let writer = &mut *context.writer;
    let expression = format!("emela_emb_float_to_int({}, UINT8_C(0))", expressions[0]);
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

pub(in crate::compiler::emit) fn float_ceil(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::FloatCeil;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        1,
        ResultType::Exact(Type::Int),
        |layouts, function, args| operand_is_float(layouts, function, &args[0]),
    )?;
    let writer = &mut *context.writer;
    let expression = format!("emela_emb_float_to_int({}, UINT8_C(1))", expressions[0]);
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

pub(in crate::compiler::emit) fn float_round(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::FloatRound;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        1,
        ResultType::Exact(Type::Int),
        |layouts, function, args| operand_is_float(layouts, function, &args[0]),
    )?;
    let writer = &mut *context.writer;
    let expression = format!("emela_emb_float_to_int({}, UINT8_C(2))", expressions[0]);
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

pub(in crate::compiler::emit) fn float_truncate(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::FloatTruncate;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        1,
        ResultType::Exact(Type::Int),
        |layouts, function, args| operand_is_float(layouts, function, &args[0]),
    )?;
    let writer = &mut *context.writer;
    let expression = format!("emela_emb_float_to_int({}, UINT8_C(3))", expressions[0]);
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

pub(in crate::compiler::emit) fn emit_float_to_int_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED int32_t emela_emb_float_to_int(double value, uint8_t operation)",
    );
    writer.begin_block("if (!(value > -2147483649.0 && value < 2147483648.0))");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_FLOAT_TO_INT, \"cannot convert Float to Int\", UINT16_C(27));",
    );
    writer.end_block();
    writer.line("int64_t rounded = (int64_t)value;");
    writer.begin_block("if (operation == UINT8_C(0) && (double)rounded > value)");
    writer.line("--rounded;");
    writer.end_block();
    writer.begin_block("else if (operation == UINT8_C(1) && (double)rounded < value)");
    writer.line("++rounded;");
    writer.end_block();
    writer.begin_block("else if (operation == UINT8_C(2))");
    writer.begin_block("if (value >= 0.0 && value - (double)rounded >= 0.5)");
    writer.line("++rounded;");
    writer.end_block();
    writer.begin_block("else if (value < 0.0 && (double)rounded - value >= 0.5)");
    writer.line("--rounded;");
    writer.end_block();
    writer.end_block();
    writer.begin_block("if (rounded < INT32_MIN || rounded > INT32_MAX)");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_FLOAT_TO_INT, \"cannot convert Float to Int\", UINT16_C(27));",
    );
    writer.end_block();
    writer.line("return (int32_t)rounded;");
    writer.end_block();
    writer.blank();
}
