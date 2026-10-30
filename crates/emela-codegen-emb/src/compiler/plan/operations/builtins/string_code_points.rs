use super::super::*;

pub(in crate::compiler::plan) fn require_runtime(
    _args: &[Operand],
    requirements: &mut crate::contract::runtime::RuntimeRequirements,
) {
    requirements.require(crate::contract::runtime::RuntimeHelper::Utf8Decode);
}
