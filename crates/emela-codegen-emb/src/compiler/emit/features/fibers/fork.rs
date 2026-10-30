//! crate::contract::ir::FiberOp::Fork
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
    let [scope, Operand::Var(body)] = args.as_slice() else {
        return Err(unsupported(
            &function.name,
            "Fiber::Fork (phase 7 follow-up)",
        ));
    };
    let expression = format!(
        "emela_fork_{}({}, {})",
        function.vars[*body].ty.index(),
        operand_expression(scope).map_err(|w| unsupported(&function.name, w))?,
        variable_name(*body)
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
