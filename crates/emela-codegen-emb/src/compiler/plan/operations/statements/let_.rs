use super::super::*;

pub(in crate::compiler::plan) fn lower(
    context: &mut LoweringContext<'_>,
    id: StmtId,
    statement: &Stmt,
    next: usize,
) -> Result<usize, Vec<Error>> {
    let Stmt::Let { value, .. } = statement else {
        unreachable!("Let entry")
    };
    match value {
        Value::Call { .. } => values::call::lower(context, id, value, next),
        Value::Fiber { .. } => values::fiber::lower(context, id, value, next),
        _ => Ok(context.graph.add(Node::Statement(id, next))),
    }
}
