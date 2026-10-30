use super::*;

pub(in crate::compiler::plan) fn lower(
    context: &mut LoweringContext<'_>,
    id: StmtId,
    statement: &Stmt,
    next: usize,
) -> Result<usize, Vec<Error>> {
    match statement {
        Stmt::Let { .. } => let_::lower(context, id, statement, next),
        Stmt::If { .. } => if_::lower(context, id, statement, next),
        Stmt::Switch { .. } => switch::lower(context, id, statement, next),
        Stmt::Loop { .. } => loop_::lower(context, id, statement, next),
        Stmt::Block { .. } => block::lower(context, id, statement, next),
        Stmt::Protect { .. } => protect::lower(context, id, statement, next),
        Stmt::Dup(..) => dup::lower(context, id, statement, next),
        Stmt::Drop(..) => drop::lower(context, id, statement, next),
    }
}

pub(in crate::compiler::plan) fn require_runtime(
    statement: &Stmt,
    active: bool,
    requirements: &mut crate::contract::runtime::RuntimeRequirements,
) {
    match statement {
        Stmt::Let { value, .. } => values::require_runtime(value, active, requirements),
        Stmt::Switch { .. } => switch::require_runtime(statement, requirements),
        Stmt::If { .. }
        | Stmt::Loop { .. }
        | Stmt::Block { .. }
        | Stmt::Protect { .. }
        | Stmt::Dup(_)
        | Stmt::Drop(_) => {}
    }
}

pub(in crate::compiler::plan) mod block;
pub(in crate::compiler::plan) mod drop;
pub(in crate::compiler::plan) mod dup;
pub(in crate::compiler::plan) mod if_;
pub(in crate::compiler::plan) mod let_;
pub(in crate::compiler::plan) mod loop_;
pub(in crate::compiler::plan) mod protect;
pub(in crate::compiler::plan) mod switch;
