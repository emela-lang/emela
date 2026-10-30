use super::super::*;

pub(in crate::compiler::plan) fn string_behavior(args: &[Operand]) -> StringBehavior {
    StringBehavior {
        output: if matches!(args, [Operand::Var(_)]) {
            StringOutput::Buffer
        } else {
            StringOutput::None
        },
        ..StringBehavior::default()
    }
}

pub(in crate::compiler::plan) fn string_bound(
    args: &[Operand],
    bounds: &StringBounds<'_>,
) -> Option<usize> {
    let [arg] = args else { return None };
    Some(match arg {
        Operand::Const(Const::Int(value)) => value.to_string().len(),
        Operand::Const(Const::Int64(value)) => value.to_string().len(),
        _ => 11.min(usize::from(bounds.capacity)),
    })
}

pub(in crate::compiler::plan) fn require_runtime(
    args: &[Operand],
    requirements: &mut crate::contract::runtime::RuntimeRequirements,
) {
    if matches!(args, [Operand::Var(_)]) {
        requirements.require(crate::contract::runtime::RuntimeHelper::IntToString);
        requirements.defects = true;
    }
}
