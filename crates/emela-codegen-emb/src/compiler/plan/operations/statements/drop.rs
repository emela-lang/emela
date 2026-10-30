use super::super::*;

pub(in crate::compiler::plan) fn lower(
    context: &mut LoweringContext<'_>,
    _id: StmtId,
    _statement: &Stmt,
    next: usize,
) -> Result<usize, Vec<Error>> {
    let _graph = &mut *context.graph;
    let function = context.function;
    let _entry = next;
    Err(unsupported(
        &function.name,
        "unsupported Stmt at Flow stage",
    ))
}
