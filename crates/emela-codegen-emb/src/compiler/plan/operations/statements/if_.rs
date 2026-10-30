use super::super::*;

pub(in crate::compiler::plan) fn lower(
    context: &mut LoweringContext<'_>,
    _id: StmtId,
    statement: &Stmt,
    next: usize,
) -> Result<usize, Vec<Error>> {
    let graph = &mut *context.graph;
    let function = context.function;
    let entry = next;
    let Stmt::If {
        dst,
        cond,
        then,
        else_,
    } = statement
    else {
        unreachable!("graph dispatch")
    };
    Ok({
        let yes = graph.sequence(function, *then, *dst, entry)?;
        let no = graph.sequence(function, *else_, *dst, entry)?;
        graph.add(Node::Branch(cond.clone(), yes, no))
    })
}
