use super::*;

pub(in crate::compiler::plan) fn require_runtime(
    op: PrimOp,
    ty: PrimTy,
    active: bool,
    requirements: &mut crate::contract::runtime::RuntimeRequirements,
) {
    use crate::contract::runtime::RuntimeHelper as Helper;
    match op {
        PrimOp::Add | PrimOp::Sub | PrimOp::Mul | PrimOp::Neg => match ty {
            PrimTy::I32 => requirements.require(Helper::I32FromBits),
            PrimTy::I64 => requirements.require(Helper::I64FromBits),
            PrimTy::F64 | PrimTy::String => {}
        },
        PrimOp::Div | PrimOp::Rem => match ty {
            PrimTy::I32 => {
                requirements.require(Helper::I32Division);
                requirements.defects = true;
            }
            PrimTy::I64 => {
                requirements.require(Helper::I64Division);
                requirements.defects = true;
            }
            PrimTy::F64 | PrimTy::String => {}
        },
        PrimOp::Eq | PrimOp::Ne | PrimOp::Lt | PrimOp::Le | PrimOp::Gt | PrimOp::Ge => match ty {
            PrimTy::String => {
                if active {
                    requirements.require(Helper::StringComparison);
                }
            }
            PrimTy::I32 | PrimTy::I64 | PrimTy::F64 => {}
        },
        PrimOp::Not => {}
    }
}
