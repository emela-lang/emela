use super::*;

pub(in crate::compiler::plan) fn lower(
    context: &mut LoweringContext<'_>,
    term: &Term,
    dst: Option<VarId>,
    next: usize,
) -> Result<usize, Vec<Error>> {
    match term {
        Term::Yield(..) => r#yield::lower(context, term, dst, next),
        Term::Return(..) => return_::lower(context, term, dst, next),
        Term::Fail(..) => fail::lower(context, term, dst, next),
        Term::Break { .. } => r#break::lower(context, term, dst, next),
        Term::Recur(..) => recur::lower(context, term, dst, next),
        Term::Defect(..) => defect::lower(context, term, dst, next),
        Term::Unwind => unwind::lower(context, term, dst, next),
        Term::Unreachable => unreachable::lower(context, term, dst, next),
    }
}

pub(in crate::compiler::plan) fn require_runtime(
    flow: &crate::contract::ir::Program,
    function: &Function,
    term: &Term,
    requirements: &mut crate::contract::runtime::RuntimeRequirements,
) {
    match term {
        Term::Defect(value) => defect::require_runtime(flow, function, value, requirements),
        Term::Unreachable => unreachable::require_runtime(requirements),
        Term::Yield(_)
        | Term::Return(_)
        | Term::Fail(_)
        | Term::Break { .. }
        | Term::Recur(_)
        | Term::Unwind => {}
    }
}

pub(in crate::compiler::plan) mod r#break;
pub(in crate::compiler::plan) mod defect;
pub(in crate::compiler::plan) mod fail;
pub(in crate::compiler::plan) mod recur;
pub(in crate::compiler::plan) mod return_;
pub(in crate::compiler::plan) mod unreachable;
pub(in crate::compiler::plan) mod unwind;
pub(in crate::compiler::plan) mod r#yield;
