//! crate::contract::ir::Value::Tuple
use super::super::*;
pub(in crate::compiler::emit) fn emit(
    context: &mut Context<'_, '_>,
    dst: &Option<VarId>,
    value: &Value,
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    match value {
        Value::Tuple(args) => {
            emit_tuple(layouts, function, *dst, args, writer)?;
        }
        other => return Err(unsupported(&function.name, value_name(other))),
    };
    Ok(())
}

pub(in crate::compiler::emit) fn emit_tuple(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    args: &[Operand],
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let Some(dst) = dst else {
        return Ok(());
    };
    let ty = function.vars[dst].ty;
    let Type::Tuple(fields) = &layouts.shapes.types[ty] else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: tuple value defines a non-tuple variable",
                function.name
            ),
        }]);
    };
    if args.len() != fields.len() {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: tuple has {} elements, expected {}",
                function.name,
                args.len(),
                fields.len()
            ),
        }]);
    }

    let target = variable_name(dst);
    let mut represented = false;
    for (index, (&field_ty, arg)) in fields.iter().zip(args).enumerate() {
        if layouts.type_repr(field_ty) == Repr::Void {
            continue;
        }
        represented = true;
        emit_aggregate_field_store(
            layouts,
            function,
            ty,
            0,
            index,
            field_ty,
            arg,
            &format!("{target}.f{index}"),
            writer,
        )?;
    }
    if !represented {
        writer.assign(&format!("{target}._unit"), "UINT8_C(0)");
    }
    Ok(())
}
