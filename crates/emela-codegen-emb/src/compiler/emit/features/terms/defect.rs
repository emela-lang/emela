//! crate::contract::ir::Term::Defect
use super::super::*;
pub(in crate::compiler::emit) fn emit(
    context: &mut Context<'_, '_>,
    term: &Term,
    _dst: Option<VarId>,
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    let Term::Defect(message) = term else {
        unreachable!("terminal dispatch")
    };
    emit_defect(layouts, function, message, writer)?;
    Ok(())
}

pub(in crate::compiler::emit) fn emit_defect(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    message: &Operand,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    match message {
        Operand::Const(Const::String(message)) => {
            let length = u16::try_from(message.len()).map_err(|_| {
                unsupported(
                    &function.name,
                    "a defect message longer than the embedded C ABI supports",
                )
            })?;
            writer.line(&format!(
                "emela_board_defect(EMELA_DEFECT_{}, {}, UINT16_C({length}));",
                defect_kind_for_message(message),
                c_string_literal(message.as_bytes())
            ));
            Ok(())
        }
        Operand::Var(variable)
            if matches!(
                layouts.shapes.types[function.vars[*variable].ty],
                Type::String
            ) =>
        {
            let value = variable_name(*variable);
            writer.line(&format!(
                "emela_board_defect(emela_emb_defect_kind({value}), {value}.data, {value}.length);"
            ));
            Ok(())
        }
        _ => Err(invalid_flow_ir(
            &function.name,
            "Defect message is not a String value",
        )),
    }
}

pub(in crate::compiler::emit) fn emit_function(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    term: &Term,
    _string_buffers: &StringBufferPlan,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    match term {
        Term::Defect(message) => emit_defect(layouts, function, message, writer)?,
        _ => unreachable!("function terminal"),
    };
    Ok(())
}

pub(in crate::compiler::emit) fn emit_suspend(
    context: &mut SuspendContext<'_, '_>,
    node: &Node,
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    let strings = context.strings;
    let Node::Defect(value) = node else {
        unreachable!("frame state dispatch")
    };
    {
        emit_return(
            layouts,
            function,
            &Term::Defect(value.clone()),
            strings,
            writer,
        )?;
    };
    Ok(())
}

pub(in crate::compiler::emit) fn emit_dynamic_defect_kind_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED int emela_emb_string_matches(EmelaString value, const char *expected, uint16_t length, int exact)",
    );
    writer.begin_block("if (value.length < length || (exact && value.length != length))");
    writer.line("return 0;");
    writer.end_block();
    writer.begin_block("for (uint16_t i = 0; i < length; ++i)");
    writer.begin_block("if (value.data[i] != expected[i])");
    writer.line("return 0;");
    writer.end_block();
    writer.end_block();
    writer.line("return 1;");
    writer.end_block();
    writer.blank();
    writer.begin_block(
        "static inline EMELA_UNUSED EmelaDefectKind emela_emb_defect_kind(EmelaString message)",
    );
    for (message, kind, exact) in [
        ("not yet implemented", "TODO", "1"),
        ("unreachable: no match arm matched", "NO_MATCH", "1"),
        ("assertion failed", "ASSERT", "0"),
    ] {
        writer.begin_block(&format!(
            "if (emela_emb_string_matches(message, {}, UINT16_C({}), {exact}))",
            c_string_literal(message.as_bytes()),
            message.len(),
        ));
        writer.line(&format!("return EMELA_DEFECT_{kind};"));
        writer.end_block();
    }
    writer.line("return EMELA_DEFECT_PANIC;");
    writer.end_block();
    writer.blank();
}

fn defect_kind_for_message(message: &str) -> &'static str {
    if message == "not yet implemented" {
        "TODO"
    } else if message.starts_with("unreachable: no match arm matched") {
        "NO_MATCH"
    } else if message.starts_with("assertion failed") {
        "ASSERT"
    } else {
        "PANIC"
    }
}
