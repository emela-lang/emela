use crate::contract::ir::{Const, Function, Operand, TypeId};
pub(crate) fn operand_type(function: &Function, operand: &Operand) -> Option<TypeId> {
    match operand {
        Operand::Var(variable) => Some(function.vars[*variable].ty),
        Operand::Const(Const::Int(_)) => Some(TypeId::INT),
        Operand::Const(Const::Bool(_)) => Some(TypeId::BOOL),
        Operand::Const(Const::Int64(_)) => Some(TypeId::INT64),
        Operand::Const(Const::Float(_)) => Some(TypeId::FLOAT),
        Operand::Const(Const::String(_)) => Some(TypeId::STRING),
        Operand::Const(Const::Unit) => Some(TypeId::UNIT),
    }
}
