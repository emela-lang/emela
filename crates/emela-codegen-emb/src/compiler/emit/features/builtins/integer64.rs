use super::super::*;
pub(in crate::compiler::emit) fn int64_from_int(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::Int64FromInt;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        1,
        ResultType::Exact(Type::Int64),
        |layouts, function, args| operand_is_int(layouts, function, &args[0]),
    )?;
    let writer = &mut *context.writer;
    let expression = format!("((int64_t)({}))", expressions[0]);
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

pub(in crate::compiler::emit) fn int64_to_int(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::Int64ToInt;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        1,
        ResultType::Exact(Type::Int),
        |layouts, function, args| operand_is_int64(layouts, function, &args[0]),
    )?;
    let writer = &mut *context.writer;
    let expression = { format!("emela_i32_from_bits((uint32_t)({}))", expressions[0]) };
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

pub(in crate::compiler::emit) fn int64_bit_and(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::Int64BitAnd;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        2,
        ResultType::Exact(Type::Int64),
        |layouts, function, args| {
            args.iter()
                .all(|operand| operand_is_int64(layouts, function, operand))
        },
    )?;
    let writer = &mut *context.writer;
    let expression = format!("({} & {})", expressions[0], expressions[1]);
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

pub(in crate::compiler::emit) fn int64_bit_or(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::Int64BitOr;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        2,
        ResultType::Exact(Type::Int64),
        |layouts, function, args| {
            args.iter()
                .all(|operand| operand_is_int64(layouts, function, operand))
        },
    )?;
    let writer = &mut *context.writer;
    let expression = format!("({} | {})", expressions[0], expressions[1]);
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

pub(in crate::compiler::emit) fn int64_bit_xor(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::Int64BitXor;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        2,
        ResultType::Exact(Type::Int64),
        |layouts, function, args| {
            args.iter()
                .all(|operand| operand_is_int64(layouts, function, operand))
        },
    )?;
    let writer = &mut *context.writer;
    let expression = format!("({} ^ {})", expressions[0], expressions[1]);
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

pub(in crate::compiler::emit) fn int64_bit_not(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::Int64BitNot;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        1,
        ResultType::Exact(Type::Int64),
        |layouts, function, args| {
            args.iter()
                .all(|operand| operand_is_int64(layouts, function, operand))
        },
    )?;
    let writer = &mut *context.writer;
    let expression = format!("(~{})", expressions[0]);
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

pub(in crate::compiler::emit) fn int64_shift_left(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::Int64ShiftLeft;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        2,
        ResultType::Exact(Type::Int64),
        |layouts, function, args| {
            operand_is_int64(layouts, function, &args[0])
                && operand_is_int(layouts, function, &args[1])
        },
    )?;
    let writer = &mut *context.writer;
    {
        writer.begin_block(&format!("if ({} < INT32_C(0))", expressions[1]));
        writer.line(&format!(
            "emela_emb_negative_shift_defect({});",
            expressions[1]
        ));
        writer.end_block();
    }
    let expression = format!(
        "emela_i64_shift_left({}, {})",
        expressions[0], expressions[1]
    );
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

pub(in crate::compiler::emit) fn int64_shift_right(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::Int64ShiftRight;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        2,
        ResultType::Exact(Type::Int64),
        |layouts, function, args| {
            operand_is_int64(layouts, function, &args[0])
                && operand_is_int(layouts, function, &args[1])
        },
    )?;
    let writer = &mut *context.writer;
    {
        writer.begin_block(&format!("if ({} < INT32_C(0))", expressions[1]));
        writer.line(&format!(
            "emela_emb_negative_shift_defect({});",
            expressions[1]
        ));
        writer.end_block();
    }
    let expression = format!(
        "emela_i64_shift_right({}, {})",
        expressions[0], expressions[1]
    );
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

pub(in crate::compiler::emit) fn int64_shift_right_unsigned(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::Int64ShiftRightUnsigned;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        2,
        ResultType::Exact(Type::Int64),
        |layouts, function, args| {
            operand_is_int64(layouts, function, &args[0])
                && operand_is_int(layouts, function, &args[1])
        },
    )?;
    let writer = &mut *context.writer;
    {
        writer.begin_block(&format!("if ({} < INT32_C(0))", expressions[1]));
        writer.line(&format!(
            "emela_emb_negative_shift_defect({});",
            expressions[1]
        ));
        writer.end_block();
    }
    let expression = format!(
        "emela_i64_shift_right_unsigned({}, {})",
        expressions[0], expressions[1]
    );
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

pub(in crate::compiler::emit) fn emit_i64_shift_helpers(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED int64_t emela_i64_shift_left(int64_t value, int32_t by)",
    );
    writer.begin_block("if (by >= INT32_C(64))");
    writer.line("return INT64_C(0);");
    writer.end_block();
    writer.line("return emela_i64_from_bits((uint64_t)value << (uint32_t)by);");
    writer.end_block();
    writer.blank();

    writer.begin_block(
        "static inline EMELA_UNUSED int64_t emela_i64_shift_right(int64_t value, int32_t by)",
    );
    writer.begin_block("if (by >= INT32_C(64))");
    writer.line("return value < INT64_C(0) ? -INT64_C(1) : INT64_C(0);");
    writer.end_block();
    writer.line("uint64_t shifted = (uint64_t)value >> (uint32_t)by;");
    writer.begin_block("if (by > INT32_C(0) && value < INT64_C(0))");
    writer.line("shifted |= UINT64_MAX << (UINT32_C(64) - (uint32_t)by);");
    writer.end_block();
    writer.line("return emela_i64_from_bits(shifted);");
    writer.end_block();
    writer.blank();

    writer.begin_block(
        "static inline EMELA_UNUSED int64_t emela_i64_shift_right_unsigned(int64_t value, int32_t by)",
    );
    writer.begin_block("if (by >= INT32_C(64))");
    writer.line("return INT64_C(0);");
    writer.end_block();
    writer.line("return emela_i64_from_bits((uint64_t)value >> (uint32_t)by);");
    writer.end_block();
    writer.blank();
}
