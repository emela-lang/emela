use super::super::*;
pub(in crate::compiler::emit) fn string_starts_with(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::StringStartsWith;
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
        "emela_emb_string_starts_with({}, {})",
        expressions[0], expressions[1]
    );
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

pub(in crate::compiler::emit) fn emit_string_starts_with_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED int32_t emela_emb_string_starts_with(EmelaString value, EmelaString prefix)",
    );
    writer.line("return emela_emb_string_matches_at(value, prefix, UINT32_C(0));");
    writer.end_block();
    writer.blank();
}
