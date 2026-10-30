//! crate::contract::ir::Value::Field
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
        Value::Field { base, ctor, index } => {
            emit_field(layouts, function, *dst, *base, *ctor, *index, writer)?;
        }
        other => return Err(unsupported(&function.name, value_name(other))),
    };
    Ok(())
}

pub(in crate::compiler::emit) fn emit_field(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    base: VarId,
    ctor: CtorRef,
    index: usize,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let base_ty = function.vars[base].ty;
    let Type::Enum(enum_id, _) = layouts.shapes.types[base_ty] else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: field base is not an enum",
                function.name
            ),
        }]);
    };
    if enum_id != ctor.enum_id {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: field constructor and base types differ",
                function.name
            ),
        }]);
    }
    let Some(fields) = layouts
        .enum_fields(base_ty)
        .and_then(|variants| variants.get(ctor.variant))
    else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: field base has no inline enum layout",
                function.name
            ),
        }]);
    };
    let Some(&field_ty) = fields.get(index) else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: enum field index is out of range",
                function.name
            ),
        }]);
    };
    let has_repr = layouts.type_repr(field_ty) != Repr::Void;
    if dst.is_some() && !has_repr {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: enum field destination does not match its representation",
                function.name
            ),
        }]);
    }
    if let Some(dst) = dst {
        let access = format!("{}.payload.v{}.f{index}", variable_name(base), ctor.variant);
        let expression = if layouts.is_recursive_box_field(base_ty, ctor.variant, index) {
            format!("*(({} *){access})", layouts.c_type(field_ty))
        } else {
            access
        };
        writer.assign(&variable_name(dst), &expression);
    }
    Ok(())
}
