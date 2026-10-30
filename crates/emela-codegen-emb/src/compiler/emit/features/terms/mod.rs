use super::*;
pub(in crate::compiler::emit) mod r#break;
pub(in crate::compiler::emit) mod defect;
pub(in crate::compiler::emit) mod fail;
pub(in crate::compiler::emit) mod recur;
pub(in crate::compiler::emit) mod return_;
pub(in crate::compiler::emit) mod unreachable;
pub(in crate::compiler::emit) mod unwind;
pub(in crate::compiler::emit) mod r#yield;
pub(in crate::compiler::emit) fn emit(
    context: &mut Context<'_, '_>,
    term: &Term,
    dst: Option<VarId>,
) -> Result<(), Vec<Error>> {
    match term {
        Term::Yield(..) => r#yield::emit(context, term, dst),
        Term::Return(..) => return_::emit(context, term, dst),
        Term::Fail(..) => fail::emit(context, term, dst),
        Term::Break { .. } => r#break::emit(context, term, dst),
        Term::Recur(..) => recur::emit(context, term, dst),
        Term::Defect(..) => defect::emit(context, term, dst),
        Term::Unwind => unwind::emit(context, term, dst),
        Term::Unreachable => unreachable::emit(context, term, dst),
    }
}

pub(in crate::compiler::emit) fn emit_function(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    term: &Term,
    string_buffers: &StringBufferPlan,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    match term {
        Term::Yield(..) => r#yield::emit_function(layouts, function, term, string_buffers, writer),
        Term::Return(..) => return_::emit_function(layouts, function, term, string_buffers, writer),
        Term::Fail(..) => fail::emit_function(layouts, function, term, string_buffers, writer),
        Term::Break { .. } => {
            r#break::emit_function(layouts, function, term, string_buffers, writer)
        }
        Term::Recur(..) => recur::emit_function(layouts, function, term, string_buffers, writer),
        Term::Defect(..) => defect::emit_function(layouts, function, term, string_buffers, writer),
        Term::Unwind => unwind::emit_function(layouts, function, term, string_buffers, writer),
        Term::Unreachable => {
            unreachable::emit_function(layouts, function, term, string_buffers, writer)
        }
    }
}
