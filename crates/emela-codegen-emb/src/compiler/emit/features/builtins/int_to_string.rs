use super::super::*;
pub(in crate::compiler::emit) fn int_to_string(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::IntToString;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        1,
        ResultType::Exact(Type::String),
        |layouts, function, args| {
            args.iter()
                .all(|operand| operand_is_int(layouts, function, operand))
        },
    )?;
    let function = context.function;
    let writer = &mut *context.writer;
    let string_buffers = context.string_buffers;
    let expression = match &args[0] {
        Operand::Const(Const::Int(value)) => constant_expression(&Const::String(value.to_string()))
            .map_err(|variant| unsupported(&function.name, variant))?
            .expect("String constant has an expression"),
        _ => {
            let Some((buffer, capacity)) = string_buffers.buffer(dst) else {
                return Err(invalid_flow_ir(
                    &function.name,
                    "dynamic Int.to_string has no planned output buffer",
                ));
            };
            format!(
                "emela_emb_int_to_string((int64_t)({}), {buffer}, {capacity})",
                expressions[0]
            )
        }
    };
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}
