use super::super::*;
pub(in crate::compiler::emit) fn string_trim(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::StringTrim;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        1,
        ResultType::Exact(Type::String),
        |layouts, function, args| {
            args.iter()
                .all(|operand| operand_is_string(layouts, function, operand))
        },
    )?;
    let writer = &mut *context.writer;
    let expression = format!("emela_emb_string_trim({})", expressions[0]);
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

pub(in crate::compiler::emit) fn emit_string_trim_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED int32_t emela_emb_is_white_space(uint32_t code_point)",
    );
    writer.line(
        "return (code_point >= UINT32_C(0x0009) && code_point <= UINT32_C(0x000d)) || code_point == UINT32_C(0x0020) || code_point == UINT32_C(0x0085) || code_point == UINT32_C(0x00a0) || code_point == UINT32_C(0x1680) || (code_point >= UINT32_C(0x2000) && code_point <= UINT32_C(0x200a)) || code_point == UINT32_C(0x2028) || code_point == UINT32_C(0x2029) || code_point == UINT32_C(0x202f) || code_point == UINT32_C(0x205f) || code_point == UINT32_C(0x3000);",
    );
    writer.end_block();
    writer.blank();

    writer.begin_block(
        "static inline EMELA_UNUSED EmelaString emela_emb_string_trim(EmelaString value)",
    );
    writer.begin_block("if (value.length == UINT16_C(0))");
    writer.line("return value;");
    writer.end_block();
    writer.line("uint32_t start = UINT32_C(0);");
    writer.line("uint32_t end = (uint32_t)value.length;");
    writer.line("uint32_t code_point;");
    writer.line("uint32_t width;");
    writer.begin_block("while (start < end)");
    writer.begin_block("if (!emela_emb_utf8_decode(value, start, &code_point, &width))");
    writer.line("break;");
    writer.end_block();
    writer.begin_block("if (!emela_emb_is_white_space(code_point))");
    writer.line("break;");
    writer.end_block();
    writer.line("start += width;");
    writer.end_block();
    writer.begin_block("while (end > start)");
    writer.line("uint32_t begin = end - UINT32_C(1);");
    writer.begin_block(
        "while (begin > start && ((uint8_t)value.data[begin] & UINT8_C(0xc0)) == UINT8_C(0x80))",
    );
    writer.line("--begin;");
    writer.end_block();
    writer.begin_block(
        "if (!emela_emb_utf8_decode(value, begin, &code_point, &width) || begin + width != end)",
    );
    writer.line("break;");
    writer.end_block();
    writer.begin_block("if (!emela_emb_is_white_space(code_point))");
    writer.line("break;");
    writer.end_block();
    writer.line("end = begin;");
    writer.end_block();
    writer.line(
        "return (EmelaString){ .data = value.data + start, .length = (uint16_t)(end - start), .owner = value.owner };",
    );
    writer.end_block();
    writer.blank();
}
