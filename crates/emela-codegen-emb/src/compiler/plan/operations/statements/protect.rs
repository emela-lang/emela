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
    let Stmt::Protect { dst, body, cleanup } = statement else {
        unreachable!("graph dispatch")
    };
    Ok({
        let depth = graph.depth;
        graph.depth += 1;
        graph.max_depth = graph.max_depth.max(graph.depth);
        let end = graph.add(Node::CleanupEnd);
        graph.error_depth += 1;
        let cleanup = graph.sequence(function, *cleanup, None, end)?;
        graph.error_depth -= 1;
        let leave = graph.add(Node::Leave {
            keep: depth,
            next: entry,
        });
        let body = graph.sequence(function, *body, *dst, leave)?;
        graph.depth = depth;
        graph.add(Node::Protect { cleanup, body })
    })
}
