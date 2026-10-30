//! crate::contract::ir::Stmt::Protect
use super::super::*;
pub(in crate::compiler::emit) fn emit(
    context: &mut Context<'_, '_>,
    statement: &Stmt,
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    let names = context.names;
    let string_buffers = context.string_buffers;
    let loops = &mut *context.loops;
    let blocks = &mut *context.blocks;
    let protects = &mut *context.protects;
    let next_temp = &mut *context.next_temp;
    let next_label = &mut *context.next_label;
    match statement {
        Stmt::Protect { dst, body, cleanup } => emit_protect(
            layouts,
            function,
            *dst,
            *body,
            *cleanup,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        ),
        other => Err(unsupported(&function.name, statement_name(other))),
    }
}

pub(in crate::compiler::emit) fn emit_enter(
    writer: &mut Writer,
    cleanup: usize,
    body: usize,
    max_depth: usize,
) {
    {
        writer.line(&format!(
            "if (frame->guard_depth >= {}) emela_scheduler_invalid();",
            max_depth
        ));
        writer.line(&format!(
            "frame->guards[frame->guard_depth++] = (emela_protection){{.cleanup = {cleanup}}};"
        ));
        writer.line(&format!("base->pc = {body}; continue;"));
    }
}
pub(in crate::compiler::emit) fn emit_leave(writer: &mut Writer, keep: usize, next: usize) {
    {
        writer . line (& format ! ("emela_protection_leave(fiber, frame->guards, frame->guard_depth, {keep}, {next}, &base->pc);")) ;
        writer.line("continue;");
    }
}
pub(in crate::compiler::emit) fn emit_finish(writer: &mut Writer) {
    {
        writer . line ("if (!emela_protection_finish(fiber, frame->guards, &frame->guard_depth, &base->pc)) return;") ;
        writer.line("continue;");
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::compiler::emit) fn emit_protect(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    body: crate::contract::ir::SeqId,
    cleanup: crate::contract::ir::SeqId,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    let body_yields = matches!(function.seqs[body].term, Term::Yield(_));
    writer.begin_block("");
    protects.push(ProtectTarget { cleanup });
    let body_result = emit_compound_sequence(
        layouts,
        function,
        body,
        dst,
        writer,
        names,
        string_buffers,
        loops,
        blocks,
        protects,
        next_temp,
        next_label,
    );
    protects.pop();
    let cleanup_result = if body_result.is_ok() && body_yields {
        emit_compound_sequence(
            layouts,
            function,
            cleanup,
            None,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        )
    } else {
        Ok(())
    };
    writer.end_block();
    body_result?;
    cleanup_result
}
