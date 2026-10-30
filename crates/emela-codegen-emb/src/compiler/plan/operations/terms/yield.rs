use super::super::*;

pub(in crate::compiler::plan) fn lower(
    context: &mut LoweringContext<'_>,
    term: &Term,
    dst: Option<VarId>,
    next: usize,
) -> Result<usize, Vec<Error>> {
    let graph = &mut *context.graph;
    let _function = context.function;
    let _entry = next;
    let Term::Yield(value) = term else {
        unreachable!("graph dispatch")
    };
    Ok(graph.assign(dst, value, next))
}
