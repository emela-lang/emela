//! Function-value and external-signature validation for the existing backend.
use super::{
    AggregateKind, TypeLayouts,
    c::is_c_identifier,
    diagnostics::{invalid_flow_ir, unsupported},
    function_positions,
};
use crate::Error;
use crate::contract::ir::{Callee, FnId, Function, Repr, Stmt, Value};
use crate::contract::ir::{Type, TypeId};
use crate::semantics::abi::LayoutRead;
use std::collections::HashSet;

pub(in crate::compiler::plan) fn validate_function_value_types(
    layouts: &TypeLayouts<'_>,
    emitted: &HashSet<FnId>,
    flow: &crate::contract::ir::Program,
) -> Result<(), Vec<Error>> {
    for &ty in &layouts.shapes.function_types {
        let Type::Fn(function_type) = layouts.shapes.types[ty].clone() else {
            unreachable!("function type registry contains a non-function")
        };
        if function_type.sets.is_some()
            || !function_value_abi_type_supported(layouts, function_type.ret, &mut HashSet::new())
            || function_type.params.iter().any(|&parameter| {
                !function_value_abi_type_supported(layouts, parameter, &mut HashSet::new())
            })
        {
            let owner = flow
                .functions
                .iter()
                .find(|(id, function)| {
                    emitted.contains(id)
                        && (function.ret == ty
                            || function.vars.iter().any(|(_, variable)| variable.ty == ty))
                })
                .map_or("function value", |(_, function)| function.name.as_str());
            return Err(unsupported(
                owner,
                "function values with unsupported parameter, result, or suspend types",
            ));
        }
    }
    Ok(())
}

pub(in crate::compiler::plan) fn function_value_abi_type_supported(
    layouts: &TypeLayouts<'_>,
    ty: TypeId,
    seen: &mut HashSet<TypeId>,
) -> bool {
    match layouts.shapes.types[ty].clone() {
        Type::Int
        | Type::Int64
        | Type::Float
        | Type::Bool
        | Type::Unit
        | Type::Never
        | Type::String
        | Type::Bytes
        | Type::Any
        | Type::List(_)
        | Type::Cell(_)
        | Type::Handler(_)
        | Type::Evidence
        | Type::FiberScope => true,
        Type::Enum(enum_id, _) => {
            layouts.enum_is_i32(enum_id)
                || layouts.shapes.core.enums[enum_id].external
                || layouts.is_cause(ty)
                || aggregate_function_abi_supported(layouts, ty, seen)
        }
        Type::Tuple(_) => aggregate_function_abi_supported(layouts, ty, seen),
        Type::Fn(function_type) => {
            if !seen.insert(ty) {
                // Function signatures may refer back through an aggregate field.
                return true;
            }
            let supported = function_type.sets.is_none()
                && function_type
                    .params
                    .iter()
                    .all(|&parameter| function_value_abi_type_supported(layouts, parameter, seen))
                && function_value_abi_type_supported(layouts, function_type.ret, seen);
            seen.remove(&ty);
            supported
        }
        _ => false,
    }
}

pub(in crate::compiler::plan) fn aggregate_function_abi_supported(
    layouts: &TypeLayouts<'_>,
    ty: TypeId,
    seen: &mut HashSet<TypeId>,
) -> bool {
    let Some(&index) = layouts.shapes.aggregate_ids.get(&ty) else {
        return false;
    };
    if !seen.insert(ty) {
        return true;
    }
    let fields = match &layouts.shapes.aggregates[index].kind {
        AggregateKind::Tuple(fields) => vec![fields.as_slice()],
        AggregateKind::Enum(variants) => variants.iter().map(Vec::as_slice).collect(),
    };
    let supported = fields.into_iter().enumerate().all(|(variant, fields)| {
        fields.iter().enumerate().all(|(index, &field)| {
            // Opaque owned edges use the heap header ABI; materialized payload roots are checked separately.
            layouts.is_recursive_box_field(ty, variant, index)
                || layouts.type_repr(field) == Repr::Void
                || function_value_abi_type_supported(layouts, field, seen)
        })
    });
    seen.remove(&ty);
    supported
}

pub(in crate::compiler::plan) fn validate_function_refs(
    flow: &crate::contract::ir::Program,
    emitted: &HashSet<FnId>,
    layouts: &TypeLayouts<'_>,
) -> Result<(), Vec<Error>> {
    for (function_id, function) in flow.functions.iter() {
        if !emitted.contains(&function_id) {
            continue;
        }
        for (_, statement) in function.stmts.iter() {
            let Stmt::Let {
                dst,
                value: Value::FnRef(target),
                ..
            } = statement
            else {
                continue;
            };
            let Some(dst) = dst else {
                continue;
            };
            let Type::Fn(function_type) = layouts.shapes.types[function.vars[*dst].ty].clone()
            else {
                return Err(invalid_flow_ir(
                    &function.name,
                    "FnRef destination is not a function value",
                ));
            };
            let target_function = &flow.functions[*target];
            let mut expected_params: Vec<TypeId> = function_type
                .params
                .into_iter()
                .filter(|&parameter| layouts.type_repr(parameter) != Repr::Void)
                .collect();
            if function_type.ev {
                expected_params.push(TypeId::EVIDENCE);
            }
            let target_params: Vec<TypeId> = target_function
                .params
                .iter()
                .skip(usize::from(target_function.env.is_some()))
                .map(|&parameter| target_function.vars[parameter].ty)
                .collect();
            if expected_params != target_params
                || function_type.ret != target_function.ret
                || function_type.fails != target_function.fails
                || function_type.suspends != target_function.suspends
            {
                return Err(invalid_flow_ir(
                    &function.name,
                    "FnRef type does not match the referenced function ABI",
                ));
            }
        }
    }
    Ok(())
}

pub(in crate::compiler::plan) fn validate_external(
    flow: &crate::contract::ir::Program,
    function: &Function,
) -> Result<String, Vec<Error>> {
    if function.infallible_cause_fail {
        if function.params.len() != 1
            || function.vars[function.params[0]].ty != TypeId::ANY
            || function.ret != TypeId::NEVER
            || function.fails
            || function.suspends
        {
            return Err(invalid_flow_ir(
                &function.name,
                "invalid backend Cause landing binding",
            ));
        }
        return Ok(function.name.clone());
    }
    let Some(external) = &function.external else {
        return Err(unsupported(&function.name, "missing external binding"));
    };
    let Some(binding) = external.binding("c") else {
        return Err(unsupported(
            &function.name,
            "external function without a C binding",
        ));
    };
    let symbol = crate::semantics::host_refs::symbol(&binding.host).to_owned();
    if !is_c_identifier(&symbol) {
        return Err(unsupported(
            &function.name,
            "external function with an invalid C symbol",
        ));
    }

    let supported_type = |ty| {
        matches!(
            flow.core.ty(ty),
            crate::contract::ir::Type::Int
                | crate::contract::ir::Type::Int64
                | crate::contract::ir::Type::Float
                | crate::contract::ir::Type::Bool
                | crate::contract::ir::Type::Unit
                | crate::contract::ir::Type::Never
        )
    };
    if function
        .params
        .iter()
        .any(|&param| !supported_type(function.vars[param].ty))
        || !supported_type(function.ret)
    {
        return Err(unsupported(
            &function.name,
            "external function with a non-scalar signature",
        ));
    }
    Ok(symbol)
}

pub(in crate::compiler::plan) fn directly_calls(caller: &Function, target: FnId) -> bool {
    let (positions, _) = function_positions(caller);
    caller.stmts.iter().any(|(id, statement)| {
        positions.contains_key(&id)
            && matches!(
                statement,
                Stmt::Let {
                    value: Value::Call {
                        callee: Callee::Direct(id),
                        ..
                    },
                    ..
                } if *id == target
            )
    })
}
