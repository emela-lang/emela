use super::*;
pub(in crate::compiler::emit) mod attempt;
pub(in crate::compiler::emit) mod r#await;
pub(in crate::compiler::emit) mod cause_kind;
pub(in crate::compiler::emit) mod cause_raise;
pub(in crate::compiler::emit) mod cause_to_string;
pub(in crate::compiler::emit) mod complete;
pub(in crate::compiler::emit) mod deferred;
pub(in crate::compiler::emit) mod deferred_await;
pub(in crate::compiler::emit) mod fork;
pub(in crate::compiler::emit) mod interrupt;
pub(in crate::compiler::emit) mod join;
pub(in crate::compiler::emit) mod poll;
pub(in crate::compiler::emit) mod scope;
pub(in crate::compiler::emit) fn emit(
    context: &mut Context<'_, '_>,
    dst: &Option<VarId>,
    value: &Value,
) -> Result<(), Vec<Error>> {
    let Value::Fiber { op, .. } = value else {
        unreachable!("Fiber dispatch")
    };
    match op {
        crate::contract::ir::FiberOp::Scope { .. } => scope::emit(context, dst, value),
        crate::contract::ir::FiberOp::Fork => fork::emit(context, dst, value),
        crate::contract::ir::FiberOp::Deferred => deferred::emit(context, dst, value),
        crate::contract::ir::FiberOp::Join => join::emit(context, dst, value),
        crate::contract::ir::FiberOp::Await { .. } => r#await::emit(context, dst, value),
        crate::contract::ir::FiberOp::Interrupt => interrupt::emit(context, dst, value),
        crate::contract::ir::FiberOp::Attempt { .. } => attempt::emit(context, dst, value),
        crate::contract::ir::FiberOp::Complete => complete::emit(context, dst, value),
        crate::contract::ir::FiberOp::DeferredAwait => deferred_await::emit(context, dst, value),
        crate::contract::ir::FiberOp::Poll => poll::emit(context, dst, value),
        crate::contract::ir::FiberOp::CauseKind { .. } => cause_kind::emit(context, dst, value),
        crate::contract::ir::FiberOp::CauseToString => cause_to_string::emit(context, dst, value),
        crate::contract::ir::FiberOp::CauseRaise => cause_raise::emit(context, dst, value),
    }
}

pub(in crate::compiler::emit) fn emit_waiting(
    context: &mut SuspendContext<'_, '_>,
    statement: &Stmt,
    op: crate::contract::ir::FiberOp,
) -> Result<(), Vec<Error>> {
    match op {
        crate::contract::ir::FiberOp::Join => join::emit_suspend(context, statement),
        crate::contract::ir::FiberOp::Await { .. } => r#await::emit_suspend(context, statement),
        crate::contract::ir::FiberOp::Interrupt => interrupt::emit_suspend(context, statement),
        _ => unreachable!("waiting Fiber selector"),
    }
}
