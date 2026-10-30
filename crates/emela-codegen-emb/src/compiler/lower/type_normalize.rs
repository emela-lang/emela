//! Resolve concrete function sets without changing the verified calling convention.
use std::collections::HashMap;

use emela_core::{Type, TypeId, TypeTable};
use emela_lir::Module;

use crate::{Error, ErrorKind};

pub(crate) fn normalize(module: &mut Module) -> Result<(), Vec<Error>> {
    let mut normalized = HashMap::new();
    for (_, function) in module.functions.iter_mut() {
        let mut resolve = |ty| {
            normalize_type(&mut module.core.types, ty, &mut normalized).map_err(|reason| {
                vec![Error {
                    kind: ErrorKind::InvalidFlowIr,
                    message: format!("invalid Flow IR in function `{}`: {reason}", function.name),
                }]
            })
        };
        function.ret = resolve(function.ret)?;
        for (_, variable) in function.vars.iter_mut() {
            variable.ty = resolve(variable.ty)?;
        }
    }
    for (_, effect) in module.core.effects.iter_mut() {
        for argument in &mut effect.args {
            *argument = normalize_type(&mut module.core.types, *argument, &mut normalized)
                .map_err(|reason| {
                    vec![Error {
                        kind: ErrorKind::InvalidFlowIr,
                        message: format!("invalid Flow effect `{}`: {reason}", effect.name),
                    }]
                })?;
        }
        for operation in &mut effect.ops {
            if operation.type_params != 0 {
                continue;
            }
            for part in operation.params.iter_mut().chain([&mut operation.ret]) {
                *part = normalize_type(&mut module.core.types, *part, &mut normalized).map_err(
                    |reason| {
                        vec![Error {
                            kind: ErrorKind::InvalidFlowIr,
                            message: format!(
                                "invalid Flow operation `{}.{}`: {reason}",
                                effect.name, operation.name
                            ),
                        }]
                    },
                )?;
            }
        }
    }
    Ok(())
}

fn normalize_type(
    types: &mut TypeTable,
    ty: TypeId,
    normalized: &mut HashMap<TypeId, TypeId>,
) -> Result<TypeId, &'static str> {
    if let Some(&result) = normalized.get(&ty) {
        return Ok(result);
    }
    let kind = types[ty].clone();
    let mut visit = |child| normalize_type(types, child, normalized);
    let result = match kind {
        Type::Param(_) => return Err("unresolved type parameter after monomorphization"),
        Type::Set(ref summary) if !summary.params.is_empty() => {
            return Err("unresolved error/effect set parameter after monomorphization");
        }
        Type::Fn(function) => {
            let params = function
                .params
                .iter()
                .map(|&part| visit(part))
                .collect::<Result<_, _>>()?;
            let ret = visit(function.ret)?;
            let resolved = if let Some(sets) = function.sets {
                let (Type::Set(errors), Type::Set(effects)) =
                    (&types[sets.errors], &types[sets.effects])
                else {
                    return Err("function error/effect sets are not set summaries");
                };
                if !errors.params.is_empty() || !effects.params.is_empty() {
                    return Err(
                        "unresolved function error/effect set parameter after monomorphization",
                    );
                }
                let exact = emela_core::fn_with_sets(
                    types,
                    params,
                    ret,
                    &function,
                    sets.errors,
                    sets.effects,
                );
                if (function.fails, function.suspends, function.ev)
                    != (exact.fails, exact.suspends, exact.ev)
                {
                    return Err("function ABI flags disagree with its concrete error/effect sets");
                }
                exact
            } else {
                emela_core::FnType {
                    params,
                    ret,
                    ..function
                }
            };
            types.intern(Type::Fn(resolved))
        }
        Type::Tuple(parts) => {
            let parts = parts
                .into_iter()
                .map(&mut visit)
                .collect::<Result<_, _>>()?;
            types.tuple(parts)
        }
        Type::List(element) => {
            let element = visit(element)?;
            types.list(element)
        }
        Type::Cell(element) => {
            let element = visit(element)?;
            types.cell(element)
        }
        Type::Enum(id, args) => {
            let args = args.into_iter().map(&mut visit).collect::<Result<_, _>>()?;
            types.enum_type(id, args)
        }
        _ => ty,
    };
    normalized.insert(ty, result);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use emela_core::{FnSets, FnType, SetSummary};

    #[test]
    fn nested_function_sets_preserve_each_exact_calling_convention() {
        let mut types = emela_core::Module::new().types;
        for fails in [false, true] {
            for suspends in [false, true] {
                for ev in [false, true] {
                    let errors = types.intern(Type::Set(SetSummary {
                        fails,
                        ..Default::default()
                    }));
                    let effects = types.intern(Type::Set(SetSummary {
                        suspends,
                        ev,
                        ..Default::default()
                    }));
                    let inner = types.intern(Type::Fn(FnType {
                        params: vec![TypeId::INT],
                        ret: TypeId::STRING,
                        fails,
                        suspends,
                        ev,
                        sets: Some(FnSets { errors, effects }),
                    }));
                    let list = types.list(inner);
                    let root = types.tuple(vec![inner, list]);
                    let resolved = normalize_type(&mut types, root, &mut HashMap::new()).unwrap();
                    let Type::Tuple(parts) = &types[resolved] else {
                        panic!("tuple")
                    };
                    let Type::Fn(function) = &types[parts[0]] else {
                        panic!("function")
                    };
                    assert_eq!(
                        (function.fails, function.suspends, function.ev),
                        (fails, suspends, ev)
                    );
                    assert!(function.sets.is_none());
                    let Type::List(element) = types[parts[1]] else {
                        panic!("List")
                    };
                    assert_eq!(element, parts[0]);
                }
            }
        }
    }
}
