use super::super::*;
pub(in crate::compiler::emit) fn string_length(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::StringLength;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        1,
        ResultType::Exact(Type::Int),
        |layouts, function, args| operand_is_string(layouts, function, &args[0]),
    )?;
    let writer = &mut *context.writer;
    {
        emit_string_length(dst, &expressions[0], writer);
        Ok(())
    }
}

pub(in crate::compiler::emit) fn emit_string_length(dst: VarId, input: &str, writer: &mut Writer) {
    let suffix = u32::from(dst.into_raw());
    let input_name = format!("emela_string_length_input_{suffix}");
    let offset_name = format!("emela_string_length_offset_{suffix}");
    let next_name = format!("emela_string_length_next_{suffix}");
    let count_name = format!("emela_string_length_count_{suffix}");

    writer.decl("EmelaString", &input_name, Some(input));
    writer.decl("uint32_t", &offset_name, Some("UINT32_C(0)"));
    writer.decl("uint32_t", &next_name, None);
    writer.decl("int32_t", &count_name, Some("INT32_C(0)"));
    writer.begin_block(&format!(
        "while ({offset_name} < (uint32_t){input_name}.length)"
    ));
    writer.begin_block(&format!(
        "if (!emela_emb_grapheme_next({input_name}, {offset_name}, &{next_name}))"
    ));
    writer.line("emela_board_defect(EMELA_DEFECT_PANIC, \"invalid UTF-8 string\", UINT16_C(20));");
    writer.end_block();
    writer.assign(&offset_name, &next_name);
    writer.line(&format!("++{count_name};"));
    writer.end_block();
    writer.assign(&variable_name(dst), &count_name);
}
