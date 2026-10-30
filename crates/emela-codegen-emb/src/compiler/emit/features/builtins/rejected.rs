use super::super::*;
pub(in crate::compiler::emit) fn show_nested(
    context: &mut BuiltinContext<'_, '_>,
    _dst: Option<VarId>,
    _args: &[Operand],
) -> Result<(), Vec<Error>> {
    Err(invalid_flow_ir(
        &context.function.name,
        "unlowered builtin ShowNested at stage Flow",
    ))
}

pub(in crate::compiler::emit) fn panic(
    context: &mut BuiltinContext<'_, '_>,
    _dst: Option<VarId>,
    _args: &[Operand],
) -> Result<(), Vec<Error>> {
    Err(invalid_flow_ir(
        &context.function.name,
        "unlowered builtin Panic at stage Flow",
    ))
}

pub(in crate::compiler::emit) fn todo(
    context: &mut BuiltinContext<'_, '_>,
    _dst: Option<VarId>,
    _args: &[Operand],
) -> Result<(), Vec<Error>> {
    Err(invalid_flow_ir(
        &context.function.name,
        "unlowered builtin Todo at stage Flow",
    ))
}

pub(in crate::compiler::emit) fn dbg(
    context: &mut BuiltinContext<'_, '_>,
    _dst: Option<VarId>,
    _args: &[Operand],
) -> Result<(), Vec<Error>> {
    Err(invalid_flow_ir(
        &context.function.name,
        "unlowered builtin Dbg at stage Flow",
    ))
}

pub(in crate::compiler::emit) fn inspect(
    context: &mut BuiltinContext<'_, '_>,
    _dst: Option<VarId>,
    _args: &[Operand],
) -> Result<(), Vec<Error>> {
    Err(invalid_flow_ir(
        &context.function.name,
        "unlowered builtin Inspect at stage Flow",
    ))
}

pub(in crate::compiler::emit) fn show_string(
    context: &mut BuiltinContext<'_, '_>,
    _dst: Option<VarId>,
    _args: &[Operand],
) -> Result<(), Vec<Error>> {
    Err(invalid_flow_ir(
        &context.function.name,
        "unlowered builtin ShowString at stage Flow",
    ))
}

pub(in crate::compiler::emit) fn debug_print(
    context: &mut BuiltinContext<'_, '_>,
    _dst: Option<VarId>,
    _args: &[Operand],
) -> Result<(), Vec<Error>> {
    Err(invalid_flow_ir(
        &context.function.name,
        "unlowered builtin DebugPrint at stage Flow",
    ))
}

pub(in crate::compiler::emit) fn assert_fail(
    context: &mut BuiltinContext<'_, '_>,
    _dst: Option<VarId>,
    _args: &[Operand],
) -> Result<(), Vec<Error>> {
    Err(invalid_flow_ir(
        &context.function.name,
        "unlowered builtin AssertFail at stage Flow",
    ))
}
