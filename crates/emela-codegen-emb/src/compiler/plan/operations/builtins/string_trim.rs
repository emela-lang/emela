use super::super::*;

pub(in crate::compiler::plan) fn string_bound(
    args: &[Operand],
    bounds: &StringBounds<'_>,
) -> Option<usize> {
    match args {
        [arg] => Some(bounds.operand(arg)),
        _ => None,
    }
}

pub(in crate::compiler::plan) fn result_ownership() -> ResultOwnership {
    ResultOwnership::Borrowed
}

pub(in crate::compiler::plan) fn require_runtime(
    _args: &[Operand],
    requirements: &mut crate::contract::runtime::RuntimeRequirements,
) {
    requirements.require(crate::contract::runtime::RuntimeHelper::StringTrim);
}
