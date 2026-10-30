use super::super::*;
pub(in crate::compiler::emit) fn int_parse(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::IntParse;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        1,
        ResultType::Option(TypeId::INT),
        |layouts, function, args| {
            args.iter()
                .all(|operand| operand_is_string(layouts, function, operand))
        },
    )?;
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    {
        let ty = function.vars[dst].ty;
        let none = option_variant_expression(layouts, function, ty, "None", None)?;
        let parsed_name = format!("{}_parsed", variable_name(dst));
        let some = option_variant_expression(layouts, function, ty, "Some", Some(&parsed_name))?;
        if let Operand::Const(Const::String(value)) = &args[0] {
            let expression = match value.parse::<i32>() {
                Ok(parsed) => option_variant_expression(
                    layouts,
                    function,
                    ty,
                    "Some",
                    Some(&int32_literal(parsed)),
                )?,
                Err(_) => none,
            };
            writer.assign(&variable_name(dst), &expression);
        } else {
            writer.decl("int32_t", &parsed_name, None);
            writer.begin_block(&format!(
                "if (emela_emb_int_parse({}, &{parsed_name}))",
                expressions[0]
            ));
            writer.assign(&variable_name(dst), &some);
            writer.end_block();
            writer.begin_block("else");
            writer.assign(&variable_name(dst), &none);
            writer.end_block();
        }
        Ok(())
    }
}

pub(in crate::compiler::emit) fn emit_int_parse_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED int emela_emb_int_parse(EmelaString value, int32_t *result)",
    );
    writer.begin_block("if (value.length == UINT16_C(0))");
    writer.line("return 0;");
    writer.end_block();
    writer.line("uint32_t index = UINT32_C(0);");
    writer.line("int negative = 0;");
    writer.line("const uint8_t first = (uint8_t)value.data[index];");
    writer.begin_block("if (first == UINT8_C(0x2b) || first == UINT8_C(0x2d))");
    writer.line("negative = first == UINT8_C(0x2d);");
    writer.line("++index;");
    writer.end_block();
    writer.begin_block("if (index >= (uint32_t)value.length)");
    writer.line("return 0;");
    writer.end_block();
    writer.line("uint64_t limit = negative ? UINT64_C(2147483648) : UINT64_C(2147483647);");
    writer.line("uint64_t magnitude = UINT64_C(0);");
    writer.begin_block("for (; index < (uint32_t)value.length; ++index)");
    writer.line("const uint8_t byte = (uint8_t)value.data[index];");
    writer.begin_block("if (byte < UINT8_C(0x30) || byte > UINT8_C(0x39))");
    writer.line("return 0;");
    writer.end_block();
    writer.line("const uint64_t digit = (uint64_t)(byte - UINT8_C(0x30));");
    writer.begin_block("if (magnitude > (limit - digit) / UINT64_C(10))");
    writer.line("return 0;");
    writer.end_block();
    writer.line("magnitude = magnitude * UINT64_C(10) + digit;");
    writer.end_block();
    writer.begin_block("if (negative && magnitude == UINT64_C(2147483648))");
    writer.line("*result = INT32_MIN;");
    writer.end_block();
    writer.line("else if (negative)");
    writer.begin_block("");
    writer.line("*result = -(int32_t)magnitude;");
    writer.end_block();
    writer.line("else");
    writer.begin_block("");
    writer.line("*result = (int32_t)magnitude;");
    writer.end_block();
    writer.line("return 1;");
    writer.end_block();
    writer.blank();
}
