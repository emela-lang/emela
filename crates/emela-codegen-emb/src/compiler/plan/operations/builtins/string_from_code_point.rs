use super::super::*;

pub(in crate::compiler::plan) fn string_behavior(args: &[Operand]) -> StringBehavior {
    StringBehavior {
        output: if matches!(args, [Operand::Var(_)]) {
            StringOutput::Buffer
        } else {
            StringOutput::None
        },
        result: BufferResult::OptionalString,
        ..StringBehavior::default()
    }
}

pub(in crate::compiler::plan) fn string_bound(
    _args: &[Operand],
    bounds: &StringBounds<'_>,
) -> Option<usize> {
    Some(4.min(usize::from(bounds.capacity)))
}

pub(in crate::compiler::plan) fn require_runtime(
    args: &[Operand],
    requirements: &mut crate::contract::runtime::RuntimeRequirements,
) {
    requirements.defects |= matches!(args, [Operand::Var(_)]);
}
