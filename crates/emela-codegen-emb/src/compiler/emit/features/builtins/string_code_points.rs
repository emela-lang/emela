use super::super::*;
pub(in crate::compiler::emit) fn string_code_points(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::StringCodePoints;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        1,
        ResultType::Exact(Type::List(TypeId::INT)),
        |layouts, function, args| operand_is_string(layouts, function, &args[0]),
    )?;
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    {
        emit_string_code_points(layouts, function, dst, &expressions[0], writer)?;
        Ok(())
    }
}

pub(in crate::compiler::emit) fn emit_string_code_points(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: VarId,
    input: &str,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let list_ty = function.vars[dst].ty;
    if !matches!(layouts.shapes.types[list_ty], Type::List(TypeId::INT)) {
        return Err(invalid_flow_ir(
            &function.name,
            "StringCodePoints destination is not List[Int]",
        ));
    }

    let suffix = u32::from(dst.into_raw());
    let input_name = format!("emela_code_points_input_{suffix}");
    let offset_name = format!("emela_code_points_offset_{suffix}");
    let code_point_name = format!("emela_code_points_value_{suffix}");
    let width_name = format!("emela_code_points_width_{suffix}");
    let head_name = format!("emela_code_points_head_{suffix}");
    let last_name = format!("emela_code_points_last_{suffix}");
    let node_name = format!("emela_code_points_node_{suffix}");
    let list_node = layouts.list_node_name(list_ty);
    let list_kind = layouts.list_index(list_ty);

    writer.decl("EmelaString", &input_name, Some(input));
    writer.decl("uint32_t", &offset_name, Some("UINT32_C(0)"));
    writer.decl("uint32_t", &code_point_name, None);
    writer.decl("uint32_t", &width_name, None);
    writer.decl("void *", &head_name, Some("NULL"));
    writer.decl(&format!("{list_node} *"), &last_name, Some("NULL"));
    writer.decl(&format!("{list_node} *"), &node_name, None);

    writer.begin_block(&format!(
        "while ({offset_name} < (uint32_t){input_name}.length)"
    ));
    writer.begin_block(&format!(
        "if (!emela_emb_utf8_decode({input_name}, {offset_name}, &{code_point_name}, &{width_name}))"
    ));
    writer.line("emela_board_defect(EMELA_DEFECT_PANIC, \"invalid UTF-8 string\", UINT16_C(20));");
    writer.end_block();
    writer.assign(
        &node_name,
        &format!("({list_node} *)emela_heap_allocate(sizeof({list_node}), UINT32_C({list_kind}))"),
    );
    writer.assign(
        &format!("{node_name}->head"),
        &format!("(int32_t){code_point_name}"),
    );
    writer.assign(&format!("{node_name}->tail"), "NULL");
    writer.begin_block(&format!("if ({last_name} == NULL)"));
    writer.assign(&head_name, &node_name);
    writer.end_block();
    writer.begin_block("else");
    writer.assign(&format!("{last_name}->tail"), &node_name);
    writer.end_block();
    writer.assign(&last_name, &node_name);
    writer.line(&format!("{offset_name} += {width_name};"));
    writer.end_block();
    writer.assign(&variable_name(dst), &head_name);
    Ok(())
}
