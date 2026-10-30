//! crate::contract::ir::Stmt::Block
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
        Stmt::Block { dst, label, body } => {
            let exit = format!("emela_block_exit_{}", *next_label);
            *next_label += 1;
            writer.begin_block("");
            blocks.push(BlockTarget {
                label: *label,
                dst: *dst,
                exit: exit.clone(),
                used: false,
                protect_depth: protects.len(),
            });
            let result = emit_compound_sequence(
                layouts,
                function,
                *body,
                *dst,
                writer,
                names,
                string_buffers,
                loops,
                blocks,
                protects,
                next_temp,
                next_label,
            );
            let block = blocks.pop().expect("block target stack underflow");
            writer.end_block();
            if block.used {
                writer.line(&format!("{exit}: ;"));
            }
            result
        }
        other => Err(unsupported(&function.name, statement_name(other))),
    }
}
