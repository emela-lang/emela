use super::super::*;

pub(in crate::compiler::plan) fn require_runtime(
    op: Builtin,
    requirements: &mut crate::contract::runtime::RuntimeRequirements,
) {
    match op {
        Builtin::IntShiftLeft | Builtin::IntShiftRight | Builtin::IntShiftRightUnsigned => {
            requirements.require(crate::contract::runtime::RuntimeHelper::I32Shift);
            requirements.require(crate::contract::runtime::RuntimeHelper::NegativeShift);
            requirements.defects = true;
        }
        _ => {}
    }
}
