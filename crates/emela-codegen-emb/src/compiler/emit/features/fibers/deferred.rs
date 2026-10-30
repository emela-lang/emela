//! crate::contract::ir::FiberOp::Deferred
use super::super::*;
pub(in crate::compiler::emit) fn emit(
    context: &mut Context<'_, '_>,
    dst: &Option<VarId>,
    value: &Value,
) -> Result<(), Vec<Error>> {
    let function = context.function;
    let writer = &mut *context.writer;
    let Value::Fiber { args, .. } = value else {
        unreachable!("Fiber entry")
    };
    let [scope] = args.as_slice() else {
        return Err(unsupported(
            &function.name,
            "Fiber::Deferred (phase 7 follow-up)",
        ));
    };
    let expression = format!(
        "emela_deferred_create({})",
        operand_expression(scope).map_err(|w| unsupported(&function.name, w))?
    );
    if let Some(dst) = dst {
        writer.assign(&variable_name(*dst), &expression)
    } else {
        writer.begin_block("");
        writer.line(&format!("EmelaAny discarded = {expression};"));
        writer.line("emela_heap_release(discarded.payload.any_box);");
        writer.end_block();
    }
    Ok(())
}
