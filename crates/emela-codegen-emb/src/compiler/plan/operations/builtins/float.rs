use super::super::*;

pub(in crate::compiler::plan) fn require_runtime(
    op: Builtin,
    requirements: &mut crate::contract::runtime::RuntimeRequirements,
) {
    if matches!(
        op,
        Builtin::FloatFloor | Builtin::FloatCeil | Builtin::FloatRound | Builtin::FloatTruncate
    ) {
        requirements.require(crate::contract::runtime::RuntimeHelper::FloatToInt);
        requirements.defects = true;
    }
}
