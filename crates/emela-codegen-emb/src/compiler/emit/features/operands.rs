use super::*;
pub(in crate::compiler::emit) fn var_value(operand: &Operand) -> Result<String, &'static str> {
    let Operand::Var(variable) = operand else {
        unreachable!("variant dispatch")
    };
    Ok(variable_name(*variable))
}
pub(in crate::compiler::emit) fn const_value(operand: &Operand) -> Result<String, &'static str> {
    let Operand::Const(value) = operand else {
        unreachable!("variant dispatch")
    };
    constant_expression(value)?.ok_or("Unit return operand")
}
pub(in crate::compiler::emit) fn dispatch(operand: &Operand) -> Result<String, &'static str> {
    match operand {
        Operand::Var(..) => var_value(operand),
        Operand::Const(..) => const_value(operand),
    }
}
