use super::super::*;
pub(in crate::compiler::emit) fn string_split(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::StringSplit;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        2,
        ResultType::Exact(Type::List(TypeId::STRING)),
        |layouts, function, args| {
            args.iter()
                .all(|operand| operand_is_string(layouts, function, operand))
        },
    )?;
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    {
        let separator_may_be_empty = !matches ! (args . get (1) , Some (Operand :: Const (Const :: String (separator))) if ! separator . is_empty ());
        emit_string_split(
            layouts,
            function,
            dst,
            &expressions[0],
            &expressions[1],
            separator_may_be_empty,
            writer,
        )?;
        Ok(())
    }
}

pub(in crate::compiler::emit) fn emit_string_split(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: VarId,
    input: &str,
    separator: &str,
    separator_may_be_empty: bool,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let list_ty = function.vars[dst].ty;
    if !matches!(layouts.shapes.types[list_ty], Type::List(TypeId::STRING)) {
        return Err(invalid_flow_ir(
            &function.name,
            "StringSplit destination is not List[String]",
        ));
    }

    let suffix = u32::from(dst.into_raw());
    let input_name = format!("emela_string_split_input_{suffix}");
    let separator_name = format!("emela_string_split_separator_{suffix}");
    let bytes_name = format!("emela_string_split_bytes_{suffix}");
    let owner_name = format!("emela_string_split_owner_{suffix}");
    let owner_created_name = format!("emela_string_split_owner_created_{suffix}");
    let head_name = format!("emela_string_split_head_{suffix}");
    let last_name = format!("emela_string_split_last_{suffix}");
    let node_name = format!("emela_string_split_node_{suffix}");
    let start_name = format!("emela_string_split_start_{suffix}");
    let scan_name = format!("emela_string_split_scan_{suffix}");
    let matched_name = format!("emela_string_split_matched_{suffix}");
    let separator_index_name = format!("emela_string_split_separator_index_{suffix}");
    let list_node = layouts.list_node_name(list_ty);
    let list_kind = layouts.list_index(list_ty);
    let string_owner_kind = layouts.string_owner_kind();
    writer.decl("EmelaString", &input_name, Some(input));
    writer.decl("EmelaString", &separator_name, Some(separator));

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
    let copy_index_name = format!("emela_string_split_copy_index_{suffix}");
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
    if separator_may_be_empty {
        writer.begin_block(&format!("if ({separator_name}.length == UINT16_C(0))"));
        let cluster_offset_name = format!("emela_string_split_cluster_offset_{suffix}");
        let cluster_next_name = format!("emela_string_split_cluster_next_{suffix}");
        let cluster_length_name = format!("emela_string_split_cluster_length_{suffix}");
        writer.decl("uint32_t", &cluster_offset_name, Some("UINT32_C(0)"));
        writer.decl("uint32_t", &cluster_next_name, None);
        writer.decl("uint32_t", &cluster_length_name, None);
        writer.begin_block(&format!(
            "while ({cluster_offset_name} < (uint32_t){input_name}.length)"
        ));
        writer.begin_block(&format!(
            "if (!emela_emb_grapheme_next({input_name}, {cluster_offset_name}, &{cluster_next_name}))"
        ));
        writer.line(
            "emela_board_defect(EMELA_DEFECT_PANIC, \"invalid UTF-8 string\", UINT16_C(20));",
        );
        writer.end_block();
        writer.assign(
            &cluster_length_name,
            &format!("{cluster_next_name} - {cluster_offset_name}"),
        );
        emit_string_split_push(
            writer,
            &node_name,
            &list_node,
            list_kind,
            &bytes_name,
            &cluster_offset_name,
            &cluster_length_name,
            &owner_name,
            &last_name,
            &head_name,
        );
        writer.assign(&cluster_offset_name, &cluster_next_name);
        writer.end_block();
        writer.end_block();
        writer.begin_block("else");
    }
    writer.decl("uint32_t", &start_name, Some("UINT32_C(0)"));
    writer.decl("uint32_t", &scan_name, Some("UINT32_C(0)"));
    writer.decl("int", &matched_name, None);
    writer.decl("uint32_t", &separator_index_name, None);
    writer.begin_block(&format!(
        "while ({scan_name} <= (uint32_t){input_name}.length && (uint32_t){separator_name}.length <= (uint32_t){input_name}.length - {scan_name})"
    ));
    writer.assign(&matched_name, "INT32_C(1)");
    writer.assign(&separator_index_name, "UINT32_C(0)");
    writer.begin_block(&format!(
        "while ({separator_index_name} < (uint32_t){separator_name}.length)"
    ));
    writer.begin_block(&format!(
        "if ({bytes_name}[{scan_name} + {separator_index_name}] != {separator_name}.data[{separator_index_name}])"
    ));
    writer.assign(&matched_name, "INT32_C(0)");
    writer.line("break;");
    writer.end_block();
    writer.line(&format!("++{separator_index_name};"));
    writer.end_block();
    writer.begin_block(&format!("if ({matched_name} != INT32_C(0))"));
    emit_string_split_push(
        writer,
        &node_name,
        &list_node,
        list_kind,
        &bytes_name,
        &start_name,
        &format!("{scan_name} - {start_name}"),
        &owner_name,
        &last_name,
        &head_name,
    );
    writer.line(&format!(
        "{scan_name} += (uint32_t){separator_name}.length;"
    ));
    writer.assign(&start_name, &scan_name);
    writer.end_block();
    writer.begin_block("else");
    writer.line(&format!("++{scan_name};"));
    writer.end_block();
    writer.end_block();
    emit_string_split_push(
        writer,
        &node_name,
        &list_node,
        list_kind,
        &bytes_name,
        &start_name,
        &format!("(uint32_t){input_name}.length - {start_name}"),
        &owner_name,
        &last_name,
        &head_name,
    );
    if separator_may_be_empty {
        writer.end_block();
    }
    writer.begin_block(&format!("if ({owner_created_name} != INT32_C(0))"));
    writer.line(&format!("emela_heap_release({owner_name});"));
    writer.end_block();
    writer.assign(&variable_name(dst), &head_name);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(in crate::compiler::emit) fn emit_string_split_push(
    writer: &mut Writer,
    node_name: &str,
    list_node: &str,
    list_kind: u32,
    bytes: &str,
    start: &str,
    length: &str,
    owner: &str,
    last: &str,
    head: &str,
) {
    writer.assign(
        node_name,
        &format!("({list_node} *)emela_heap_allocate(sizeof({list_node}), UINT32_C({list_kind}))"),
    );
    writer.assign(
        &format!("{node_name}->head"),
        &format!(
            "(EmelaString){{ .data = {bytes} + ({start}), .length = (uint16_t)({length}), .owner = {owner} }}"
        ),
    );
    writer.line(&format!("emela_heap_retain({owner});"));
    writer.assign(&format!("{node_name}->tail"), "NULL");
    writer.begin_block(&format!("if ({last} == NULL)"));
    writer.assign(head, node_name);
    writer.end_block();
    writer.begin_block("else");
    writer.assign(&format!("{last}->tail"), node_name);
    writer.end_block();
    writer.assign(last, node_name);
}
