use super::super::*;
pub(in crate::compiler::emit) fn string_join(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::StringJoin;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        2,
        ResultType::Exact(Type::String),
        |layouts, function, args| {
            operand_is_string_list(layouts, function, &args[0])
                && operand_is_string(layouts, function, &args[1])
        },
    )?;
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    let string_buffers = context.string_buffers;
    {
        let Operand::Var(list) = args[0] else {
            return Err(invalid_flow_ir(
                &function.name,
                "StringJoin first argument is not a List variable",
            ));
        };
        emit_string_join(
            layouts,
            function,
            dst,
            function.vars[list].ty,
            &expressions[0],
            &expressions[1],
            string_buffers,
            writer,
        )?;
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::compiler::emit) fn emit_string_join(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: VarId,
    list_ty: TypeId,
    parts: &str,
    separator: &str,
    string_buffers: &StringBufferPlan,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    if !matches!(layouts.shapes.types[list_ty], Type::List(TypeId::STRING)) {
        return Err(invalid_flow_ir(
            &function.name,
            "StringJoin first argument is not List[String]",
        ));
    }
    let Some((buffer, capacity)) = string_buffers.buffer(dst) else {
        return Err(invalid_flow_ir(
            &function.name,
            "StringJoin has no planned output buffer",
        ));
    };
    let suffix = u32::from(dst.into_raw());
    let list_node = layouts.list_node_name(list_ty);
    let cursor_name = format!("emela_string_join_cursor_{suffix}");
    let length_name = format!("emela_string_join_length_{suffix}");
    let first_name = format!("emela_string_join_first_{suffix}");

    writer.decl(
        &format!("const {list_node} *"),
        &cursor_name,
        Some(&format!("(const {list_node} *)({parts})")),
    );
    writer.decl("uint32_t", &length_name, Some("UINT32_C(0)"));
    writer.decl("int", &first_name, Some("INT32_C(1)"));
    writer.begin_block(&format!("while ({cursor_name} != NULL)"));
    writer.begin_block(&format!("if ({first_name} == INT32_C(0))"));
    writer.line(&format!(
        "emela_emb_string_join_put({buffer}, {capacity}, &{length_name}, {separator});"
    ));
    writer.end_block();
    writer.line(&format!(
        "emela_emb_string_join_put({buffer}, {capacity}, &{length_name}, {cursor_name}->head);"
    ));
    writer.assign(&first_name, "INT32_C(0)");
    writer.assign(
        &cursor_name,
        &format!("(const {list_node} *){cursor_name}->tail"),
    );
    writer.end_block();
    writer.assign(
        &variable_name(dst),
        &format!("(EmelaString){{ .data = {buffer}, .length = (uint16_t){length_name} }}"),
    );
    Ok(())
}

pub(in crate::compiler::emit) fn emit_string_join_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED void emela_emb_string_join_put(char *buffer, uint16_t capacity, uint32_t *length, EmelaString value)",
    );
    writer.begin_block("if ((uint32_t)value.length > (uint32_t)capacity - *length)");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"string capacity exceeded\", UINT16_C(24));",
    );
    writer.end_block();
    writer.line("for (uint32_t index = UINT32_C(0); index < (uint32_t)value.length; ++index)");
    writer.begin_block("");
    writer.line("buffer[*length + index] = value.data[index];");
    writer.end_block();
    writer.line("*length += (uint32_t)value.length;");
    writer.end_block();
    writer.blank();
}
