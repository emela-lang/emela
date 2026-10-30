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
    let Term::Unreachable = term else {
        unreachable!("graph dispatch")
    };
    Ok(graph.add(Node::Unreachable))
}

pub(in crate::compiler::plan) fn require_runtime(
    requirements: &mut crate::contract::runtime::RuntimeRequirements,
) {
    requirements.defects = true;
}
