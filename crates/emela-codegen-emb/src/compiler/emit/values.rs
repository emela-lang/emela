//! Value expressions, projections, and generated-value stores.
use super::functions::emit_retain_operand;
use super::{
    StringBufferPlan, TypeLayouts,
    c::int32_literal,
    diagnostics::{invalid_flow_ir, unsupported},
    features,
};
use crate::contract::ir::{Builtin, Type, TypeId};
use crate::contract::ir::{Const, Function, Operand, PrimOp, PrimTy, Repr, RuntimeOp, VarId};
use crate::semantics::abi::LayoutRead;
use crate::{Error, writer::Writer};

pub(in crate::compiler::emit) fn operand_is_string(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    operand: &Operand,
) -> bool {
    match operand {
        Operand::Const(Const::String(_)) => true,
        Operand::Var(variable) => {
            matches!(
                layouts.shapes.types[function.vars[*variable].ty],
                Type::String
            )
        }
        _ => false,
    }
}

pub(in crate::compiler::emit) fn operand_is_string_list(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    operand: &Operand,
) -> bool {
    match operand {
        Operand::Var(variable) => {
            matches!(
                layouts.shapes.types[function.vars[*variable].ty],
                Type::List(TypeId::STRING)
            )
        }
        _ => false,
    }
}

pub(in crate::compiler::emit) fn operand_is_int(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    operand: &Operand,
) -> bool {
    match operand {
        Operand::Const(Const::Int(_)) => true,
        Operand::Var(variable) => {
            matches!(layouts.shapes.types[function.vars[*variable].ty], Type::Int)
        }
        _ => false,
    }
}

pub(in crate::compiler::emit) fn operand_is_int64(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    operand: &Operand,
) -> bool {
    match operand {
        Operand::Const(Const::Int64(_)) => true,
        Operand::Var(variable) => {
            matches!(
                layouts.shapes.types[function.vars[*variable].ty],
                Type::Int64
            )
        }
        _ => false,
    }
}

pub(in crate::compiler::emit) fn operand_is_float(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    operand: &Operand,
) -> bool {
    match operand {
        Operand::Const(Const::Float(_)) => true,
        Operand::Var(variable) => matches!(
            layouts.shapes.types[function.vars[*variable].ty],
            Type::Float
        ),
        _ => false,
    }
}

pub(in crate::compiler::emit) fn option_variant_expression(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    ty: TypeId,
    variant_name: &str,
    value: Option<&str>,
) -> Result<String, Vec<Error>> {
    let Type::Enum(enum_id, _) = layouts.shapes.types[ty] else {
        return Err(invalid_flow_ir(
            &function.name,
            "Option builtin result is not an enum",
        ));
    };
    let definition = &layouts.shapes.core.enums[enum_id];
    if definition.name != "Option" {
        return Err(invalid_flow_ir(
            &function.name,
            "Option builtin result has the wrong enum type",
        ));
    }
    let Some(variant_index) = definition
        .variants
        .iter()
        .position(|variant| variant.name == variant_name)
    else {
        return Err(invalid_flow_ir(
            &function.name,
            "Option enum is missing a required variant",
        ));
    };
    let Some(fields) = layouts
        .enum_fields(ty)
        .and_then(|variants| variants.get(variant_index))
    else {
        return Err(invalid_flow_ir(
            &function.name,
            "Option result has no inline enum layout",
        ));
    };
    let mut initializers = vec![format!(".tag = {}", int32_literal(variant_index as i32))];
    match value {
        Some(value) if fields.len() == 1 && layouts.type_repr(fields[0]) != Repr::Void => {
            initializers.push(format!(".payload.v{variant_index}.f0 = {value}"));
        }
        Some(_) => {
            return Err(invalid_flow_ir(
                &function.name,
                "Option Some payload does not match its runtime representation",
            ));
        }
        None if fields
            .iter()
            .all(|field| layouts.type_repr(*field) == Repr::Void) =>
        {
            initializers.push(format!(".payload.v{variant_index}._unit = UINT8_C(0)"));
        }
        None => {
            return Err(invalid_flow_ir(
                &function.name,
                "Option None variant unexpectedly carries a runtime payload",
            ));
        }
    }
    Ok(format!(
        "({}){{ {} }}",
        layouts.c_type(ty),
        initializers.join(", ")
    ))
}

pub(in crate::compiler::emit) fn emit_builtin(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    op: Builtin,
    args: &[Operand],
    string_buffers: &StringBufferPlan,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    features::builtins::emit(
        &mut features::BuiltinContext {
            layouts,
            function,
            string_buffers,
            writer,
        },
        dst,
        op,
        args,
    )
}

pub(in crate::compiler::emit) fn emit_runtime(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    op: RuntimeOp,
    args: &[Operand],
    string_buffers: &StringBufferPlan,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    features::runtime::emit(
        &mut features::BuiltinContext {
            layouts,
            function,
            string_buffers,
            writer,
        },
        dst,
        op,
        args,
    )
}

#[allow(clippy::too_many_arguments)]
pub(in crate::compiler::emit) fn emit_aggregate_field_store(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    owner_ty: TypeId,
    variant: usize,
    field: usize,
    field_ty: TypeId,
    value: &Operand,
    target: &str,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let expression = operand_expression(value).map_err(|kind| unsupported(&function.name, kind))?;
    if layouts.is_recursive_box_field(owner_ty, variant, field) {
        let box_kind = layouts.recursive_box_kind(field_ty).ok_or_else(|| {
            invalid_flow_ir(
                &function.name,
                "recursive aggregate field has no heap box layout",
            )
        })?;
        let field_type = layouts.c_type(field_ty);
        writer.assign(
            target,
            &format!("emela_heap_allocate(sizeof({field_type}), UINT32_C({box_kind}))"),
        );
        layouts.emit_owned_value_store(
            field_ty,
            &expression,
            &format!("*(({field_type} *){target})"),
            writer,
        );
    } else {
        if layouts.is_managed_value(field_ty) {
            emit_retain_operand(layouts, function, value, writer);
        }
        writer.assign(target, &expression);
    }
    Ok(())
}

pub(in crate::compiler::emit) fn primitive_expression(
    op: PrimOp,
    ty: PrimTy,
    args: &[Operand],
) -> Result<String, &'static str> {
    features::primitive_types::emit(op, ty, args)
}

pub(in crate::compiler::emit) fn operand_expression(
    operand: &Operand,
) -> Result<String, &'static str> {
    features::operands::dispatch(operand)
}

pub(in crate::compiler::emit) fn constant_expression(
    value: &crate::contract::ir::Const,
) -> Result<Option<String>, &'static str> {
    features::constants::dispatch(value)
}

use super::layouts::LayoutEmission;
