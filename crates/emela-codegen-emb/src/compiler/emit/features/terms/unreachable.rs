//! crate::contract::ir::Term::Unreachable
use super::super::*;
pub(in crate::compiler::emit) fn emit(
    context: &mut Context<'_, '_>,
    term: &Term,
    _dst: Option<VarId>,
) -> Result<(), Vec<Error>> {
    let writer = &mut *context.writer;
    let Term::Unreachable = term else {
        unreachable!("terminal dispatch")
    };
    writer.line("emela_board_defect(EMELA_DEFECT_ASSERT, \"unreachable code\", UINT16_C(16));");
    Ok(())
}

pub(in crate::compiler::emit) fn emit_function(
    _layouts: &TypeLayouts<'_>,
    _function: &Function,
    term: &Term,
    _string_buffers: &StringBufferPlan,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    match term {
        Term::Unreachable => writer
            .line("emela_board_defect(EMELA_DEFECT_ASSERT, \"unreachable code\", UINT16_C(16));"),
        _ => unreachable!("function terminal"),
    };
    Ok(())
}

pub(in crate::compiler::emit) fn emit_suspend(
    context: &mut SuspendContext<'_, '_>,
    node: &Node,
) -> Result<(), Vec<Error>> {
    let writer = &mut *context.writer;
    let Node::Unreachable = node else {
        unreachable!("frame state dispatch")
    };
    writer.line("emela_scheduler_invalid();");
    Ok(())
}
