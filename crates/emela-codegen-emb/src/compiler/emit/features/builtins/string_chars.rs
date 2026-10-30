use super::super::*;
pub(in crate::compiler::emit) fn string_chars(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::StringChars;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        1,
        ResultType::Exact(Type::List(TypeId::STRING)),
        |layouts, function, args| operand_is_string(layouts, function, &args[0]),
    )?;
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    {
        emit_string_chars(layouts, function, dst, &expressions[0], writer)?;
        Ok(())
    }
}

pub(in crate::compiler::emit) fn emit_string_chars(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: VarId,
    input: &str,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let list_ty = function.vars[dst].ty;
    if !matches!(layouts.shapes.types[list_ty], Type::List(TypeId::STRING)) {
        return Err(invalid_flow_ir(
            &function.name,
            "StringChars destination is not List[String]",
        ));
    }

    let suffix = u32::from(dst.into_raw());
    let input_name = format!("emela_string_chars_input_{suffix}");
    let copy_index_name = format!("emela_string_chars_copy_index_{suffix}");
    let bytes_name = format!("emela_string_chars_bytes_{suffix}");
    let owner_name = format!("emela_string_chars_owner_{suffix}");
    let owner_created_name = format!("emela_string_chars_owner_created_{suffix}");
    let head_name = format!("emela_string_chars_head_{suffix}");
    let last_name = format!("emela_string_chars_last_{suffix}");
    let node_name = format!("emela_string_chars_node_{suffix}");
    let offset_name = format!("emela_string_chars_offset_{suffix}");
    let next_name = format!("emela_string_chars_next_{suffix}");
    let length_name = format!("emela_string_chars_length_{suffix}");
    let list_node = layouts.list_node_name(list_ty);
    let list_kind = layouts.list_index(list_ty);
    let string_owner_kind = layouts.string_owner_kind();

    writer.decl("EmelaString", &input_name, Some(input));
    writer.decl(
        "const char *",
        &bytes_name,
        Some(&format!("{input_name}.data")),
    );
    writer.decl("void *", &owner_name, Some(&format!("{input_name}.owner")));
    writer.decl("int", &owner_created_name, Some("INT32_C(0)"));
    writer.begin_block(&format!(
        "if ({owner_name} == NULL && {input_name}.length != UINT16_C(0))"
    ));
    writer.assign(
        &owner_name,
        &format!("emela_heap_allocate((size_t){input_name}.length, UINT32_C({string_owner_kind}))"),
    );
    writer.assign(&bytes_name, &format!("(const char *){owner_name}"));
    writer.decl("uint32_t", &copy_index_name, Some("UINT32_C(0)"));
    writer.begin_block(&format!(
        "while ({copy_index_name} < (uint32_t){input_name}.length)"
    ));
    writer.line(&format!(
        "((char *){owner_name})[{copy_index_name}] = {input_name}.data[{copy_index_name}];"
    ));
    writer.line(&format!("++{copy_index_name};"));
    writer.end_block();
    writer.assign(&owner_created_name, "INT32_C(1)");
    writer.end_block();
    writer.begin_block(&format!("if ({bytes_name} == NULL)"));
    writer.assign(&bytes_name, "\"\"");
    writer.end_block();

    writer.decl("void *", &head_name, Some("NULL"));
    writer.decl(&format!("{list_node} *"), &last_name, Some("NULL"));
    writer.decl(&format!("{list_node} *"), &node_name, None);
    writer.decl("uint32_t", &offset_name, Some("UINT32_C(0)"));
    writer.decl("uint32_t", &next_name, None);
    writer.decl("uint32_t", &length_name, None);
    writer.begin_block(&format!(
        "while ({offset_name} < (uint32_t){input_name}.length)"
    ));
    writer.begin_block(&format!(
        "if (!emela_emb_grapheme_next({input_name}, {offset_name}, &{next_name}))"
    ));
    writer.line("emela_board_defect(EMELA_DEFECT_PANIC, \"invalid UTF-8 string\", UINT16_C(20));");
    writer.end_block();
    writer.assign(&length_name, &format!("{next_name} - {offset_name}"));
    emit_string_split_push(
        writer,
        &node_name,
        &list_node,
        list_kind,
        &bytes_name,
        &offset_name,
        &length_name,
        &owner_name,
        &last_name,
        &head_name,
    );
    writer.assign(&offset_name, &next_name);
    writer.end_block();
    writer.begin_block(&format!("if ({owner_created_name} != INT32_C(0))"));
    writer.line(&format!("emela_heap_release({owner_name});"));
    writer.end_block();
    writer.assign(&variable_name(dst), &head_name);
    Ok(())
}
