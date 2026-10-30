use super::super::*;
pub(in crate::compiler::emit) fn string_from_code_point(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::StringFromCodePoint;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        1,
        ResultType::Option(TypeId::STRING),
        |layouts, function, args| {
            args.iter()
                .all(|operand| operand_is_int(layouts, function, operand))
        },
    )?;
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    let string_buffers = context.string_buffers;
    {
        let ty = function.vars[dst].ty;
        let none = option_variant_expression(layouts, function, ty, "None", None)?;
        if let Operand::Const(Const::Int(value)) = &args[0] {
            let expression = match u32::try_from(*value).ok().and_then(char::from_u32) {
                Some(character) => {
                    let string = constant_expression(&Const::String(character.to_string()))
                        .map_err(|variant| unsupported(&function.name, variant))?
                        .ok_or_else(|| {
                            invalid_flow_ir(
                                &function.name,
                                "String.from_code_point constant has no C expression",
                            )
                        })?;
                    option_variant_expression(layouts, function, ty, "Some", Some(&string))?
                }
                None => none,
            };
            writer.assign(&variable_name(dst), &expression);
        } else {
            let Some((buffer, capacity)) = string_buffers.buffer(dst) else {
                return Err(invalid_flow_ir(
                    &function.name,
                    "dynamic String.from_code_point has no planned output buffer",
                ));
            };
            let codepoint = &expressions[0];
            let value_name = format!("{}_code_point", variable_name(dst));
            let length_name = format!("{}_byte_length", variable_name(dst));
            let string_value =
                format!("(EmelaString){{ .data = {buffer}, .length = (uint16_t){length_name} }}");
            let some =
                option_variant_expression(layouts, function, ty, "Some", Some(&string_value))?;
            writer . begin_block (& format ! ("if ({codepoint} < INT32_C(0) || {codepoint} > INT32_C(0x10ffff) || ({codepoint} >= INT32_C(0xd800) && {codepoint} <= INT32_C(0xdfff)))")) ;
            writer.assign(&variable_name(dst), &none);
            writer.end_block();
            writer.begin_block("else");
            writer.decl(
                "uint32_t",
                &value_name,
                Some(&format!("(uint32_t)({codepoint})")),
            );
            writer.decl("uint16_t", &length_name, None);
            writer.begin_block(&format!("if ({value_name} <= UINT32_C(0x7f))"));
            writer.assign(&length_name, "UINT16_C(1)");
            writer.end_block();
            writer.begin_block(&format!("else if ({value_name} <= UINT32_C(0x7ff))"));
            writer.assign(&length_name, "UINT16_C(2)");
            writer.end_block();
            writer.begin_block(&format!("else if ({value_name} <= UINT32_C(0xffff))"));
            writer.assign(&length_name, "UINT16_C(3)");
            writer.end_block();
            writer.begin_block("else");
            writer.assign(&length_name, "UINT16_C(4)");
            writer.end_block();
            writer.begin_block(&format!("if ({length_name} > {capacity})"));
            writer . line ("emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"string capacity exceeded\", UINT16_C(24));" ,) ;
            writer.end_block();
            // Only the encodings that fit a fixed array are emitted: clang rejects
            // constant indexes past its end even behind the capacity defect above.
            let fits = |bytes: u16| {
                string_buffers
                    .array_capacity(dst)
                    .is_none_or(|capacity| bytes <= capacity)
            };
            writer.begin_block(&format!("if ({length_name} == UINT16_C(1))"));
            writer.assign(&format!("{buffer}[0]"), &format!("(char){value_name}"));
            writer.end_block();
            if fits(2) {
                writer.begin_block(&format!("else if ({length_name} == UINT16_C(2))"));
                writer.assign(
                    &format!("{buffer}[0]"),
                    &format!("(char)(UINT32_C(0xc0) | ({value_name} >> UINT32_C(6)))"),
                );
                writer.assign(
                    &format!("{buffer}[1]"),
                    &format!("(char)(UINT32_C(0x80) | ({value_name} & UINT32_C(0x3f)))"),
                );
                writer.end_block();
            }
            if fits(3) {
                writer.begin_block(&format!("else if ({length_name} == UINT16_C(3))"));
                writer.assign(
                    &format!("{buffer}[0]"),
                    &format!("(char)(UINT32_C(0xe0) | ({value_name} >> UINT32_C(12)))"),
                );
                writer.assign(
                    &format!("{buffer}[1]"),
                    &format!(
                        "(char)(UINT32_C(0x80) | (({value_name} >> UINT32_C(6)) & UINT32_C(0x3f)))"
                    ),
                );
                writer.assign(
                    &format!("{buffer}[2]"),
                    &format!("(char)(UINT32_C(0x80) | ({value_name} & UINT32_C(0x3f)))"),
                );
                writer.end_block();
            }
            if fits(4) {
                writer.begin_block(&format!("else if ({length_name} == UINT16_C(4))"));
                writer.assign(
                    &format!("{buffer}[0]"),
                    &format!("(char)(UINT32_C(0xf0) | ({value_name} >> UINT32_C(18)))"),
                );
                writer.assign(
                    &format!("{buffer}[1]"),
                    &format!(
                        "(char)(UINT32_C(0x80) | (({value_name} >> UINT32_C(12)) & UINT32_C(0x3f)))"
                    ),
                );
                writer.assign(
                    &format!("{buffer}[2]"),
                    &format!(
                        "(char)(UINT32_C(0x80) | (({value_name} >> UINT32_C(6)) & UINT32_C(0x3f)))"
                    ),
                );
                writer.assign(
                    &format!("{buffer}[3]"),
                    &format!("(char)(UINT32_C(0x80) | ({value_name} & UINT32_C(0x3f)))"),
                );
                writer.end_block();
            }
            writer.assign(&variable_name(dst), &some);
            writer.end_block();
        }
        Ok(())
    }
}
