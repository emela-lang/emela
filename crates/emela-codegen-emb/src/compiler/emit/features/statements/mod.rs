use super::*;
pub(in crate::compiler::emit) mod block;
pub(in crate::compiler::emit) mod drop;
pub(in crate::compiler::emit) mod dup;
pub(in crate::compiler::emit) mod if_;
pub(in crate::compiler::emit) mod let_;
pub(in crate::compiler::emit) mod loop_;
pub(in crate::compiler::emit) mod protect;
pub(in crate::compiler::emit) mod switch;
pub(in crate::compiler::emit) fn emit(
    context: &mut Context<'_, '_>,
    statement: &Stmt,
) -> Result<(), Vec<Error>> {
    match statement {
        Stmt::Let { .. } => let_::emit(context, statement),
        Stmt::If { .. } => if_::emit(context, statement),
        Stmt::Switch { .. } => switch::emit(context, statement),
        Stmt::Loop { .. } => loop_::emit(context, statement),
        Stmt::Block { .. } => block::emit(context, statement),
        Stmt::Protect { .. } => protect::emit(context, statement),
        Stmt::Dup(_) => dup::emit(context, statement),
        Stmt::Drop(_) => drop::emit(context, statement),
    }
}
