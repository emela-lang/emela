//! crate::contract::ir::Value::Make
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
        Value::Make { ctor, args } => {
            emit_make(layouts, function, *dst, *ctor, args, writer)?;
        }
        other => return Err(unsupported(&function.name, value_name(other))),
    };
    Ok(())
}

pub(in crate::compiler::emit) fn emit_make(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    ctor: CtorRef,
    args: &[Operand],
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let Some(dst) = dst else {
        return Ok(());
    };
    let ty = function.vars[dst].ty;
    let Type::Enum(enum_id, _) = layouts.shapes.types[ty] else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: enum constructor defines a non-enum value",
                function.name
            ),
        }]);
    };
    if enum_id != ctor.enum_id {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: enum constructor and destination types differ",
                function.name
            ),
        }]);
    }
    let Some(variant) = layouts.shapes.core.enums[enum_id]
        .variants
        .get(ctor.variant)
    else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: enum constructor index is out of range",
                function.name
            ),
        }]);
    };
    if args.len() != variant.tys.len() {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: enum constructor has {} arguments, expected {}",
                function.name,
                args.len(),
                variant.tys.len()
            ),
        }]);
    }

    let target = variable_name(dst);
    if layouts.enum_is_i32(enum_id) {
        let tag = i32::try_from(ctor.variant)
            .map_err(|_| unsupported(&function.name, "enum with too many variants"))?;
        writer.assign(&target, &int32_literal(tag));
        return Ok(());
    }

    let Some(fields) = layouts
        .enum_fields(ty)
        .and_then(|variants| variants.get(ctor.variant))
    else {
        return Err(unsupported(
            &function.name,
            "enum value without an inline C layout",
        ));
    };
    let tag = i32::try_from(ctor.variant)
        .map_err(|_| unsupported(&function.name, "enum with too many variants"))?;
    // A payload-free variant still shares storage with larger union members.
    // Initialize that storage before copying the whole enum (not just its tag
    // and dummy byte), including through optimized Option/error branches.
    if fields
        .iter()
        .all(|field| layouts.type_repr(*field) == Repr::Void)
    {
        writer.assign(&target, &format!("({}){{0}}", layouts.c_type(ty)));
    }
    writer.assign(&format!("{target}.tag"), &int32_literal(tag));

    let mut represented = false;
    for (index, (field_ty, arg)) in fields.iter().zip(args).enumerate() {
        if layouts.type_repr(*field_ty) == Repr::Void {
            continue;
        }
        represented = true;
        emit_aggregate_field_store(
            layouts,
            function,
            ty,
            ctor.variant,
            index,
            *field_ty,
            arg,
            &format!("{target}.payload.v{}.f{index}", ctor.variant),
            writer,
        )?;
    }
    if !represented {
        writer.assign(
            &format!("{target}.payload.v{}._unit", ctor.variant),
            "UINT8_C(0)",
        );
    }
    Ok(())
}
