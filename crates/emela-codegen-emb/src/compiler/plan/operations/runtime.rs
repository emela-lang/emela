use super::*;

use crate::semantics::strings::show_string;

pub(in crate::compiler::plan) fn string_behavior(
    op: RuntimeOp,
    args: &[Operand],
) -> StringBehavior {
    match op {
        RuntimeOp::ShowString => StringBehavior {
            output: if args.len() == 1 && !matches!(args[0], Operand::Const(Const::String(_))) {
                StringOutput::Buffer
            } else {
                StringOutput::None
            },
            input_use: InputUse::ReadOnly,
            ..StringBehavior::default()
        },
        RuntimeOp::DebugPrint => StringBehavior {
            input_use: InputUse::ReadOnly,
            ..StringBehavior::default()
        },
        RuntimeOp::Retire
        | RuntimeOp::Defect
        | RuntimeOp::DefectAt
        | RuntimeOp::DefectTake
        | RuntimeOp::DefectResume
        | RuntimeOp::Hold
        | RuntimeOp::Unhold
        | RuntimeOp::Yield
        | RuntimeOp::YieldPerform => StringBehavior::default(),
    }
}

pub(in crate::compiler::plan) fn string_bound(
    op: RuntimeOp,
    args: &[Operand],
    bounds: &StringBounds<'_>,
) -> Option<usize> {
    match op {
        RuntimeOp::ShowString => {
            let [arg] = args else { return None };
            Some(match arg {
                Operand::Const(Const::String(value)) => show_string(value).len(),
                _ => bounds
                    .operand(arg)
                    .saturating_mul(6)
                    .saturating_add(2)
                    .min(usize::from(bounds.capacity)),
            })
        }
        RuntimeOp::DebugPrint
        | RuntimeOp::Retire
        | RuntimeOp::Defect
        | RuntimeOp::DefectAt
        | RuntimeOp::DefectTake
        | RuntimeOp::DefectResume
        | RuntimeOp::Hold
        | RuntimeOp::Unhold
        | RuntimeOp::Yield
        | RuntimeOp::YieldPerform => None,
    }
}

pub(in crate::compiler::plan) fn require_runtime(
    op: RuntimeOp,
    args: &[Operand],
    requirements: &mut crate::contract::runtime::RuntimeRequirements,
) {
    match op {
        RuntimeOp::ShowString => {
            if !matches!(args, [Operand::Const(Const::String(_))]) {
                requirements.require(crate::contract::runtime::RuntimeHelper::StringShow);
                requirements.defects = true;
            }
        }
        RuntimeOp::DebugPrint => requirements.debug = true,
        RuntimeOp::Retire
        | RuntimeOp::Defect
        | RuntimeOp::DefectAt
        | RuntimeOp::DefectTake
        | RuntimeOp::DefectResume
        | RuntimeOp::Hold
        | RuntimeOp::Unhold
        | RuntimeOp::Yield
        | RuntimeOp::YieldPerform => {}
    }
}

pub(in crate::compiler::plan) fn needs_heap(op: RuntimeOp) -> bool {
    match op {
        RuntimeOp::Retire => true,
        RuntimeOp::ShowString
        | RuntimeOp::DebugPrint
        | RuntimeOp::Defect
        | RuntimeOp::DefectAt
        | RuntimeOp::DefectTake
        | RuntimeOp::DefectResume
        | RuntimeOp::Hold
        | RuntimeOp::Unhold
        | RuntimeOp::Yield
        | RuntimeOp::YieldPerform => false,
    }
}
