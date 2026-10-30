use super::super::*;

pub(in crate::compiler::plan) fn lower(
    context: &mut LoweringContext<'_>,
    id: StmtId,
    value: &Value,
    next: usize,
) -> Result<usize, Vec<Error>> {
    let Value::Fiber { on_fail, .. } = value else {
        unreachable!("value graph entry")
    };
    let graph = &mut *context.graph;
    let next = if let Some(edge) = on_fail {
        let failed = graph.failure(context.function, *edge)?;
        graph.add(Node::CheckError {
            normal: next,
            failed,
            slot: graph.error_depth,
        })
    } else {
        next
    };
    Ok(graph.add(Node::Statement(id, next)))
}

pub(in crate::compiler::plan) fn needs_heap() -> bool {
    true
}
