use super::super::*;

pub(in crate::compiler::plan) fn lower(
    context: &mut LoweringContext<'_>,
    term: &Term,
    _dst: Option<VarId>,
    next: usize,
) -> Result<usize, Vec<Error>> {
    let graph = &mut *context.graph;
    let _function = context.function;
    let _entry = next;
    let Term::Return(value) = term else {
        unreachable!("graph dispatch")
    };
    Ok({
        let returned = graph.add(Node::Return(value.clone()));
        graph.leave(0, returned)
    })
}
