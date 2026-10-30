use super::super::*;
pub(in crate::compiler::emit) fn string_append(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::StringAppend;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        2,
        ResultType::Exact(Type::String),
        |layouts, function, args| {
            args.iter()
                .all(|operand| operand_is_string(layouts, function, operand))
        },
    )?;
    let function = context.function;
    let writer = &mut *context.writer;
    let string_buffers = context.string_buffers;
    let expression = match string_buffers.append_modes[&dst] {
        AppendMode::Constant => {
            let [
                Operand::Const(Const::String(left)),
                Operand::Const(Const::String(right)),
            ] = args
            else {
                unreachable!("constant append")
            };
            constant_expression(&Const::String(format!("{left}{right}")))
                .map_err(|variant| unsupported(&function.name, variant))?
                .ok_or_else(|| {
                    invalid_flow_ir(&function.name, "String append has no C expression")
                })?
        }
        AppendMode::AliasInput => {
            let [left, right] = args else {
                unreachable!("binary append")
            };
            let other = if matches!(left, Operand::Const(Const::String(value)) if value.is_empty())
            {
                right
            } else {
                left
            };
            operand_expression(other).map_err(|variant| unsupported(&function.name, variant))?
        }
        AppendMode::Buffered => {
            let Some((buffer, capacity)) = string_buffers.buffer(dst) else {
                return Err(invalid_flow_ir(
                    &function.name,
                    "dynamic String append has no planned output buffer",
                ));
            };
            let copy_left = if matches!(args[0], Operand::Const(Const::String(_))) {
                "INT32_C(1)".to_owned()
            } else {
                format!("(({}).data != {buffer})", expressions[0])
            };
            format!(
                "emela_emb_string_append({}, {}, {buffer}, {capacity}, {copy_left})",
                expressions[0], expressions[1]
            )
        }
    };
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

pub(in crate::compiler::emit) fn emit_string_append_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED EmelaString emela_emb_string_append(EmelaString left, EmelaString right, char *buffer, uint16_t capacity, int copy_left)",
    );
    writer.begin_block(
        "if (left.length > capacity || right.length > (uint16_t)(capacity - left.length))",
    );
    writer.line(
        "emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"string capacity exceeded\", UINT16_C(24));",
    );
    writer.end_block();
    writer.begin_block("if (copy_left)");
    writer.begin_block("for (uint32_t i = UINT32_C(0); i < (uint32_t)left.length; ++i)");
    writer.line("buffer[i] = left.data[i];");
    writer.end_block();
    writer.end_block();
    writer.begin_block("for (uint32_t i = UINT32_C(0); i < (uint32_t)right.length; ++i)");
    writer.line("buffer[(uint32_t)left.length + i] = right.data[i];");
    writer.end_block();
    writer.line(
        "return (EmelaString){ .data = buffer, .length = (uint16_t)(left.length + right.length) };",
    );
    writer.end_block();
    writer.blank();
}

use crate::contract::operations::AppendMode;
