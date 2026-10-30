//! crate::contract::ir::Term::Unwind
use super::super::*;
pub(in crate::compiler::emit) fn emit(
    context: &mut Context<'_, '_>,
    term: &Term,
    _dst: Option<VarId>,
) -> Result<(), Vec<Error>> {
    let function = context.function;
    Err(unsupported(&function.name, term_name(term)))
}

pub(in crate::compiler::emit) fn emit_function(
    _layouts: &TypeLayouts<'_>,
    function: &Function,
    term: &Term,
    _string_buffers: &StringBufferPlan,
    _writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    Err(unsupported(&function.name, term_name(term)))
}
