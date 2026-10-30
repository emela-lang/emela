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
    let Term::Defect(value) = term else {
        unreachable!("graph dispatch")
    };
    Ok(graph.add(Node::Defect(value.clone())))
}

pub(in crate::compiler::plan) fn require_runtime(
    flow: &crate::contract::ir::Program,
    function: &Function,
    value: &Operand,
    requirements: &mut crate::contract::runtime::RuntimeRequirements,
) {
    requirements.defects = true;
    if matches ! (value , Operand :: Var (var) if matches ! (flow . core . ty (function . vars [* var] . ty) , Type :: String))
    {
        requirements.require(crate::contract::runtime::RuntimeHelper::DynamicDefect);
    }
}
