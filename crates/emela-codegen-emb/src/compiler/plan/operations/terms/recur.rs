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
    let Term::Recur(values) = term else {
        unreachable!("graph dispatch")
    };
    Ok({
        let target = graph
            .loops
            .last()
            .ok_or_else(|| invalid_flow_ir(&function.name, "suspend Recur without enclosing Loop"))?
            .clone();
        let assign = graph.add(Node::Assign(
            target.vars.into_iter().zip(values.clone()).collect(),
            target.entry,
        ));
        graph.leave(target.depth, assign)
    })
}
