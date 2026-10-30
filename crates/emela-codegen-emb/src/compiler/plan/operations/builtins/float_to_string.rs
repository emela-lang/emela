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
        Operand::Const(Const::Float(value)) => {
            crate::semantics::numeric::format_float(*value).len()
        }
        _ => crate::semantics::numeric::MAX_FLOAT_OUTPUT_BYTES.min(usize::from(bounds.capacity)),
    })
}

pub(in crate::compiler::plan) fn require_runtime(
    args: &[Operand],
    requirements: &mut crate::contract::runtime::RuntimeRequirements,
) {
    if matches!(args, [Operand::Var(_)]) {
        requirements.require(crate::contract::runtime::RuntimeHelper::FloatToString);
        requirements.defects = true;
    }
}
