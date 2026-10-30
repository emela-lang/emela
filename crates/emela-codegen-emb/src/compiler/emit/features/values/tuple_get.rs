//! crate::contract::ir::Value::TupleGet
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
        Value::TupleGet { base, index } => {
            emit_tuple_get(layouts, function, *dst, *base, *index, writer)?;
        }
        other => return Err(unsupported(&function.name, value_name(other))),
    };
    Ok(())
}

pub(in crate::compiler::emit) fn emit_tuple_get(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    base: VarId,
    index: usize,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let base_ty = function.vars[base].ty;
    let Type::Tuple(fields) = &layouts.shapes.types[base_ty] else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: tuple projection base is not a tuple",
                function.name
            ),
        }]);
    };
    let Some(&field_ty) = fields.get(index) else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: tuple field index is out of range",
                function.name
            ),
        }]);
    };
    let has_repr = layouts.type_repr(field_ty) != Repr::Void;
    if dst.is_some() && !has_repr {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: tuple projection destination does not match its representation",
                function.name
            ),
        }]);
    }
    if let Some(dst) = dst {
        let access = format!("{}.f{index}", variable_name(base));
        let expression = if layouts.is_recursive_box_field(base_ty, 0, index) {
            format!("*(({} *){access})", layouts.c_type(field_ty))
        } else {
            access
        };
        writer.assign(&variable_name(dst), &expression);
    }
    Ok(())
}
