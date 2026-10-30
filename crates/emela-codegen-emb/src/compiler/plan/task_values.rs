use super::*;

pub(super) fn result_types(
    flow: &crate::contract::ir::Program,
    emitted: &HashSet<FnId>,
) -> Vec<TypeId> {
    let mut types: Vec<_> = flow
        .functions
        .iter()
        .filter_map(|(id, f)| {
            (emitted.contains(&id)
                && f.suspends
                && f.external.is_some()
                && !suspend::scalar(&flow.core.types, f.ret))
            .then_some(f.ret)
        })
        .collect();
    types.sort_by_key(|ty| ty.index());
    types.dedup();
    types
}

pub(super) fn validate_types(
    flow: &crate::contract::ir::Program,
    emitted: &HashSet<FnId>,
    layouts: &TypeLayouts<'_>,
) -> Result<(), Vec<Error>> {
    for (id, function) in flow.functions.iter() {
        if !emitted.contains(&id) || !function.suspends || function.external.is_none() {
            continue;
        }
        for ty in std::iter::once(function.ret)
            .chain(function.params.iter().map(|&p| function.vars[p].ty))
        {
            if !boundary_type(layouts, ty, &mut HashSet::new()) {
                return Err(unsupported(
                    &function.name,
                    "external task signature containing a callback or runtime capability",
                ));
            }
        }
    }
    Ok(())
}

fn boundary_type(layouts: &TypeLayouts<'_>, ty: TypeId, seen: &mut HashSet<TypeId>) -> bool {
    if !seen.insert(ty) {
        return true;
    }
    match &layouts.shapes.types[ty] {
        Type::Int
        | Type::Int64
        | Type::Float
        | Type::Bool
        | Type::Unit
        | Type::Never
        | Type::String
        | Type::Bytes
        | Type::Any => true,
        Type::List(element) => boundary_type(layouts, *element, seen),
        Type::Tuple(_) | Type::Enum(_, _) => {
            if layouts.is_runtime_handle(ty) {
                return false;
            }
            if let Type::Enum(id, _) = layouts.shapes.types[ty]
                && (layouts.shapes.core.enums[id].external || layouts.enum_is_i32(id))
            {
                return true;
            }
            let Some(&index) = layouts.shapes.aggregate_ids.get(&ty) else {
                return false;
            };
            let fields: Vec<_> = match &layouts.shapes.aggregates[index].kind {
                AggregateKind::Tuple(fields) => vec![fields.as_slice()],
                AggregateKind::Enum(variants) => variants.iter().map(Vec::as_slice).collect(),
            };
            fields
                .into_iter()
                .flatten()
                .all(|&field| boundary_type(layouts, field, seen))
        }
        _ => false,
    }
}
