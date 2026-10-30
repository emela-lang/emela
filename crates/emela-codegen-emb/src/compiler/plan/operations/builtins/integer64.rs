use super::super::*;

pub(in crate::compiler::plan) fn require_runtime(
    op: Builtin,
    requirements: &mut crate::contract::runtime::RuntimeRequirements,
) {
    match op {
        Builtin::Int64ShiftLeft | Builtin::Int64ShiftRight | Builtin::Int64ShiftRightUnsigned => {
            requirements.require(crate::contract::runtime::RuntimeHelper::I64Shift);
            requirements.require(crate::contract::runtime::RuntimeHelper::NegativeShift);
            requirements.defects = true;
        }
        Builtin::Int64ToInt => {
            requirements.require(crate::contract::runtime::RuntimeHelper::I32FromBits)
        }
        _ => {}
    }
}
