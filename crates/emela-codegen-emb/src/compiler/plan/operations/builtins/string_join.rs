use super::super::*;

pub(in crate::compiler::plan) fn string_behavior(args: &[Operand]) -> StringBehavior {
    StringBehavior {
        output: if args.len() == 2 {
            StringOutput::Buffer
        } else {
            StringOutput::None
        },
        input_use: InputUse::ReadOnly,
        ..StringBehavior::default()
    }
}

pub(in crate::compiler::plan) fn string_bound(
    _args: &[Operand],
    bounds: &StringBounds<'_>,
) -> Option<usize> {
    Some(usize::from(bounds.capacity))
}

pub(in crate::compiler::plan) fn require_runtime(
    _args: &[Operand],
    requirements: &mut crate::contract::runtime::RuntimeRequirements,
) {
    requirements.require(crate::contract::runtime::RuntimeHelper::StringJoin);
    requirements.defects = true;
}
