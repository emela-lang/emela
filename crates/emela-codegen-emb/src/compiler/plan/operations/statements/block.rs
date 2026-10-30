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
    let Stmt::Block { dst, label, body } = statement else {
        unreachable!("graph dispatch")
    };
    Ok({
        graph.blocks.insert(*label, (*dst, entry, graph.depth));
        let body = graph.sequence(function, *body, *dst, entry)?;
        graph.blocks.remove(label);
        body
    })
}
