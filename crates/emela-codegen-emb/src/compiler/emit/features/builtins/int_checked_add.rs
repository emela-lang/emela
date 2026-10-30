use super::super::*;
pub(in crate::compiler::emit) fn int_checked_add(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::IntCheckedAdd;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        2,
        ResultType::Option(TypeId::INT),
        |layouts, function, args| {
            args.iter()
                .all(|operand| operand_is_int(layouts, function, operand))
        },
    )?;
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    {
        let ty = function.vars[dst].ty;
        let sum_name = format!("{}_checked_sum", variable_name(dst));
        writer.decl(
            "int64_t",
            &sum_name,
            Some(&format!(
                "((int64_t)({}) + (int64_t)({}))",
                expressions[0], expressions[1]
            )),
        );
        let none = option_variant_expression(layouts, function, ty, "None", None)?;
        let some = option_variant_expression(
            layouts,
            function,
            ty,
            "Some",
            Some(&format!("(int32_t){sum_name}")),
        )?;
        writer.begin_block(&format!(
            "if ({sum_name} < (int64_t)INT32_MIN || {sum_name} > (int64_t)INT32_MAX)"
        ));
        writer.assign(&variable_name(dst), &none);
        writer.end_block();
        writer.begin_block("else");
        writer.assign(&variable_name(dst), &some);
        writer.end_block();
        Ok(())
    }
}
