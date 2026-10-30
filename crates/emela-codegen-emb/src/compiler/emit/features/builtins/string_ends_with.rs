use super::super::*;
pub(in crate::compiler::emit) fn string_ends_with(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::StringEndsWith;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        2,
        ResultType::Exact(Type::Bool),
        |layouts, function, args| {
            args.iter()
                .all(|operand| operand_is_string(layouts, function, operand))
        },
    )?;
    let writer = &mut *context.writer;
    let expression = format!(
        "emela_emb_string_ends_with({}, {})",
        expressions[0], expressions[1]
    );
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

pub(in crate::compiler::emit) fn emit_string_ends_with_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED int32_t emela_emb_string_ends_with(EmelaString value, EmelaString suffix)",
    );
    writer.begin_block("if (suffix.length > value.length)");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.line("uint32_t offset = (uint32_t)value.length - suffix.length;");
    writer.line("return emela_emb_string_matches_at(value, suffix, offset);");
    writer.end_block();
    writer.blank();
}
