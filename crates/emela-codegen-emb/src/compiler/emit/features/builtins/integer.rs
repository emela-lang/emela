use super::super::*;
pub(in crate::compiler::emit) fn int_bit_and(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::IntBitAnd;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        2,
        ResultType::Exact(Type::Int),
        |layouts, function, args| {
            args.iter()
                .all(|operand| operand_is_int(layouts, function, operand))
        },
    )?;
    let writer = &mut *context.writer;
    let expression = format!("({} & {})", expressions[0], expressions[1]);
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

pub(in crate::compiler::emit) fn int_bit_or(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::IntBitOr;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        2,
        ResultType::Exact(Type::Int),
        |layouts, function, args| {
            args.iter()
                .all(|operand| operand_is_int(layouts, function, operand))
        },
    )?;
    let writer = &mut *context.writer;
    let expression = format!("({} | {})", expressions[0], expressions[1]);
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

pub(in crate::compiler::emit) fn int_bit_xor(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::IntBitXor;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        2,
        ResultType::Exact(Type::Int),
        |layouts, function, args| {
            args.iter()
                .all(|operand| operand_is_int(layouts, function, operand))
        },
    )?;
    let writer = &mut *context.writer;
    let expression = format!("({} ^ {})", expressions[0], expressions[1]);
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

pub(in crate::compiler::emit) fn int_bit_not(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::IntBitNot;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        1,
        ResultType::Exact(Type::Int),
        |layouts, function, args| {
            args.iter()
                .all(|operand| operand_is_int(layouts, function, operand))
        },
    )?;
    let writer = &mut *context.writer;
    let expression = format!("(~{})", expressions[0]);
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

pub(in crate::compiler::emit) fn int_shift_left(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::IntShiftLeft;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        2,
        ResultType::Exact(Type::Int),
        |layouts, function, args| {
            args.iter()
                .all(|operand| operand_is_int(layouts, function, operand))
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
        "emela_i32_shift_left({}, {})",
        expressions[0], expressions[1]
    );
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

pub(in crate::compiler::emit) fn int_shift_right(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::IntShiftRight;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        2,
        ResultType::Exact(Type::Int),
        |layouts, function, args| {
            args.iter()
                .all(|operand| operand_is_int(layouts, function, operand))
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
        "emela_i32_shift_right({}, {})",
        expressions[0], expressions[1]
    );
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

pub(in crate::compiler::emit) fn int_shift_right_unsigned(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::IntShiftRightUnsigned;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        2,
        ResultType::Exact(Type::Int),
        |layouts, function, args| {
            args.iter()
                .all(|operand| operand_is_int(layouts, function, operand))
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
        "emela_i32_shift_right_unsigned({}, {})",
        expressions[0], expressions[1]
    );
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

pub(in crate::compiler::emit) fn emit_i32_shift_helpers(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED int32_t emela_i32_shift_left(int32_t value, int32_t by)",
    );
    writer.begin_block("if (by >= INT32_C(32))");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.line("return emela_i32_from_bits((uint32_t)value << (uint32_t)by);");
    writer.end_block();
    writer.blank();

    writer.begin_block(
        "static inline EMELA_UNUSED int32_t emela_i32_shift_right(int32_t value, int32_t by)",
    );
    writer.begin_block("if (by >= INT32_C(32))");
    writer.line("return value < INT32_C(0) ? -INT32_C(1) : INT32_C(0);");
    writer.end_block();
    writer.line("uint32_t shifted = (uint32_t)value >> (uint32_t)by;");
    writer.begin_block("if (by > INT32_C(0) && value < INT32_C(0))");
    writer.line("shifted |= UINT32_MAX << (UINT32_C(32) - (uint32_t)by);");
    writer.end_block();
    writer.line("return emela_i32_from_bits(shifted);");
    writer.end_block();
    writer.blank();

    writer.begin_block(
        "static inline EMELA_UNUSED int32_t emela_i32_shift_right_unsigned(int32_t value, int32_t by)",
    );
    writer.begin_block("if (by >= INT32_C(32))");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.line("return emela_i32_from_bits((uint32_t)value >> (uint32_t)by);");
    writer.end_block();
    writer.blank();
}
