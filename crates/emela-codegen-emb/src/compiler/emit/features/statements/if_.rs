//! crate::contract::ir::Stmt::If
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
        Stmt::If {
            dst,
            cond,
            then,
            else_,
        } => {
            let condition =
                operand_expression(cond).map_err(|variant| unsupported(&function.name, variant))?;
            writer.begin_block(&format!("if (({condition}) != INT32_C(0))"));
            emit_compound_sequence(
                layouts,
                function,
                *then,
                *dst,
                writer,
                names,
                string_buffers,
                loops,
                blocks,
                protects,
                next_temp,
                next_label,
            )?;
            writer.end_block();
            writer.begin_block("else");
            emit_compound_sequence(
                layouts,
                function,
                *else_,
                *dst,
                writer,
                names,
                string_buffers,
                loops,
                blocks,
                protects,
                next_temp,
                next_label,
            )?;
            writer.end_block();
            Ok(())
        }
        other => Err(unsupported(&function.name, statement_name(other))),
    }
}

pub(in crate::compiler::emit) fn emit_suspend(
    context: &mut SuspendContext<'_, '_>,
    node: &Node,
) -> Result<(), Vec<Error>> {
    let function = context.function;
    let writer = &mut *context.writer;
    let Node::Branch(cond, yes, no) = node else {
        unreachable!("frame state dispatch")
    };
    {
        writer.line(&format!(
            "frame->base.pc = {} ? {yes} : {no}; continue;",
            operand_expression(cond).map_err(|why| unsupported(&function.name, why))?
        ));
    };
    Ok(())
}
