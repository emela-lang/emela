use super::super::*;

pub(in crate::compiler::plan) fn require_runtime(
    args: &[Operand],
    requirements: &mut crate::contract::runtime::RuntimeRequirements,
) {
    if matches!(args, [Operand::Var(_)]) {
        requirements.require(crate::contract::runtime::RuntimeHelper::IntParse);
    }
}
