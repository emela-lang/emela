use super::super::*;
pub(in crate::compiler::emit) fn float_parse(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::FloatParse;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        1,
        ResultType::Option(TypeId::FLOAT),
        |layouts, function, args| operand_is_string(layouts, function, &args[0]),
    )?;
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    {
        let ty = function.vars[dst].ty;
        let none = option_variant_expression(layouts, function, ty, "None", None)?;
        if let Some(Operand::Const(Const::String(value))) = args.first() {
            let expression = match crate::semantics::numeric::parse_float(value) {
                Some(parsed) => option_variant_expression(
                    layouts,
                    function,
                    ty,
                    "Some",
                    Some(&float_literal(parsed)),
                )?,
                None => none,
            };
            writer.assign(&variable_name(dst), &expression);
        } else {
            let parsed_name = format!("{}_parsed", variable_name(dst));
            let some =
                option_variant_expression(layouts, function, ty, "Some", Some(&parsed_name))?;
            writer.decl("double", &parsed_name, None);
            writer.begin_block(&format!(
                "if (emela_emb_float_parse({}, &{parsed_name}))",
                expressions[0]
            ));
            writer.assign(&variable_name(dst), &some);
            writer.end_block();
            writer.begin_block("else");
            writer.assign(&variable_name(dst), &none);
            writer.end_block();
        }
        Ok(())
    }
}
