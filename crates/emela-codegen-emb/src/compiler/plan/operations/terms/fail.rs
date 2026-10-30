use super::super::*;

pub(in crate::compiler::plan) fn lower(
    context: &mut LoweringContext<'_>,
    term: &Term,
    _dst: Option<VarId>,
    next: usize,
) -> Result<usize, Vec<Error>> {
    let graph = &mut *context.graph;
    let function = context.function;
    let _entry = next;
    let Term::Fail(error) = term else {
        unreachable!("graph dispatch")
    };
    Ok({
        let fail = graph.failure(function, OnFail::Propagate)?;
        graph.add(Node::SetError(error.clone(), fail, graph.error_depth))
    })
}
