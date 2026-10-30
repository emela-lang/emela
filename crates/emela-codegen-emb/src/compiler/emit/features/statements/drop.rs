//! crate::contract::ir::Stmt::Drop
use super::super::*;
pub(in crate::compiler::emit) fn emit(
    context: &mut Context<'_, '_>,
    statement: &Stmt,
) -> Result<(), Vec<Error>> {
    let function = context.function;
    Err(unsupported(&function.name, statement_name(statement)))
}
