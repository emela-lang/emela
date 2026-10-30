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
    let Term::Break { label, value } = term else {
        unreachable!("graph dispatch")
    };
    Ok({
        let &(dst, next, depth) = graph.blocks.get(label).ok_or_else(|| {
            invalid_flow_ir(&function.name, "suspend Break without enclosing Block")
        })?;
        let assign = graph.assign(dst, value, next);
        graph.leave(depth, assign)
    })
}
