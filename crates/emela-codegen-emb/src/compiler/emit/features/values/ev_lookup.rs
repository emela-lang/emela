//! crate::contract::ir::Value::EvLookup
use super::super::*;
pub(in crate::compiler::emit) fn emit(
    context: &mut Context<'_, '_>,
    dst: &Option<VarId>,
    value: &Value,
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    match value {
        Value::EvLookup { ev, effect } => {
            emit_evidence_lookup(layouts, function, *dst, *ev, *effect, writer)?;
        }
        other => return Err(unsupported(&function.name, value_name(other))),
    };
    Ok(())
}

pub(in crate::compiler::emit) fn emit_evidence_lookup(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    evidence: VarId,
    effect: EffectId,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let Some(dst) = dst else {
        return Err(invalid_flow_ir(
            &function.name,
            "EvLookup has no destination",
        ));
    };
    if layouts.shapes.types[function.vars[dst].ty] != Type::Handler(effect)
        || !matches!(
            layouts.shapes.types[function.vars[evidence].ty],
            Type::Evidence
        )
    {
        return Err(invalid_flow_ir(
            &function.name,
            "EvLookup has inconsistent evidence and handler types",
        ));
    }
    let cursor = format!("emela_evidence_cursor_{}", u32::from(dst.into_raw()));
    writer.decl(
        "emela_evidence_node *",
        &cursor,
        Some(&variable_name(evidence)),
    );
    writer.begin_block(&format!("while ({cursor} != NULL)"));
    writer.begin_block(&format!(
        "if ({cursor}->effect == UINT32_C({}))",
        u32::from(effect.into_raw())
    ));
    writer.assign(&variable_name(dst), &format!("{cursor}->handler"));
    writer.line(&format!("goto emela_evidence_lookup_done_{cursor};"));
    writer.end_block();
    writer.assign(&cursor, &format!("({cursor}->parent)"));
    writer.end_block();
    writer.line(
        "emela_board_defect(EMELA_DEFECT_PANIC, \"effect handler not found\", UINT16_C(23));",
    );
    writer.line(&format!("emela_evidence_lookup_done_{cursor}: ;"));
    Ok(())
}
