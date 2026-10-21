//! Detect unbounded generic instantiation before expanding concrete field layouts.
use std::collections::{HashMap, HashSet};

use emela_core::{EnumId, Module, Type, TypeId};

pub(crate) fn expanding_enums(core: &Module) -> HashSet<EnumId> {
    let mut slots = HashMap::new();
    let mut owners = Vec::new();
    for (id, definition) in core.enums.iter() {
        let parameters: Vec<_> = (0..definition.params)
            .map(|_| {
                let slot = owners.len();
                owners.push(id);
                slot
            })
            .collect();
        slots.insert(id, parameters);
    }
    // Only arguments eventually exposed as field types expose their nested applications.
    // Opaque/phantom parameters still participate in the outer instance's identity.
    let mut exposed = HashSet::new();
    loop {
        let mut next = exposed.clone();
        for (id, definition) in core.enums.iter() {
            for &field in definition.variants.iter().flat_map(|v| &v.tys) {
                exposed_parameters(core, field, &exposed, &mut |parameter| {
                    if parameter < definition.params {
                        next.insert((id, parameter));
                    }
                });
            }
        }
        if next == exposed {
            break;
        }
        exposed = next;
    }
    let mut edges = vec![Vec::new(); owners.len()];
    for (id, definition) in core.enums.iter() {
        for &field in definition.variants.iter().flat_map(|v| &v.tys) {
            applications(core, field, id, &slots, &exposed, &mut edges);
        }
    }
    let mut expanding = HashSet::new();
    for (source, outgoing) in edges.iter().enumerate() {
        for &(target, grows) in outgoing {
            if !grows {
                continue;
            }
            let mut pending = vec![target];
            let mut seen = HashSet::new();
            while let Some(node) = pending.pop() {
                if node == source {
                    expanding.insert(owners[source]);
                    break;
                }
                if seen.insert(node) {
                    pending.extend(edges[node].iter().map(|&(next, _)| next));
                }
            }
        }
    }
    expanding
}

fn exposed_parameters(
    core: &Module,
    ty: TypeId,
    exposed: &HashSet<(EnumId, usize)>,
    visit: &mut impl FnMut(usize),
) {
    match &core.types[ty] {
        Type::Param(index) => visit(*index),
        Type::Enum(id, args) => {
            for (index, &argument) in args.iter().enumerate() {
                if exposed.contains(&(*id, index)) {
                    exposed_parameters(core, argument, exposed, visit);
                }
            }
        }
        Type::Tuple(fields) => {
            for &field in fields {
                exposed_parameters(core, field, exposed, visit);
            }
        }
        Type::List(element) | Type::Cell(element) => {
            exposed_parameters(core, *element, exposed, visit)
        }
        Type::Fn(function) => {
            for &part in function.params.iter().chain([&function.ret]) {
                exposed_parameters(core, part, exposed, visit);
            }
        }
        _ => {}
    }
}

fn parameter_occurrences(
    core: &Module,
    ty: TypeId,
    wrapped: bool,
    visit: &mut impl FnMut(usize, bool),
) {
    match &core.types[ty] {
        Type::Param(index) => visit(*index, wrapped),
        Type::Enum(_, parts) | Type::Tuple(parts) => {
            for &part in parts {
                parameter_occurrences(core, part, true, visit);
            }
        }
        Type::List(element) | Type::Cell(element) => {
            parameter_occurrences(core, *element, true, visit)
        }
        Type::Fn(function) => {
            for &part in function.params.iter().chain([&function.ret]) {
                parameter_occurrences(core, part, true, visit);
            }
        }
        // Closed set summaries range over finite ABI flags; substitution merges them.
        Type::Set(_) => {}
        _ => {}
    }
}

fn applications(
    core: &Module,
    ty: TypeId,
    owner: EnumId,
    slots: &HashMap<EnumId, Vec<usize>>,
    exposed: &HashSet<(EnumId, usize)>,
    edges: &mut [Vec<(usize, bool)>],
) {
    match &core.types[ty] {
        Type::Enum(id, args) => {
            for (index, &argument) in args.iter().enumerate() {
                if let Some(&target) = slots[id].get(index) {
                    parameter_occurrences(core, argument, false, &mut |parameter, grows| {
                        if let Some(&source) = slots[&owner].get(parameter)
                            && !edges[source].contains(&(target, grows))
                        {
                            edges[source].push((target, grows));
                        }
                    });
                }
                if exposed.contains(&(*id, index)) {
                    applications(core, argument, owner, slots, exposed, edges);
                }
            }
        }
        Type::Tuple(fields) => {
            for &field in fields {
                applications(core, field, owner, slots, exposed, edges);
            }
        }
        Type::List(element) | Type::Cell(element) => {
            applications(core, *element, owner, slots, exposed, edges)
        }
        Type::Fn(function) => {
            for &part in function.params.iter().chain([&function.ret]) {
                applications(core, part, owner, slots, exposed, edges);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emela_core::{EnumDef, VariantDef};

    fn definition(core: &mut Module, name: &str, params: usize) -> EnumId {
        core.add_enum(EnumDef {
            name: name.into(),
            params,
            variants: vec![],
            error: false,
            external: false,
        })
    }

    #[test]
    fn argument_permutations_and_temporary_growth_that_resets_are_finite() {
        let mut core = Module::new();
        let first = core.type_param(0);
        let second = core.type_param(1);
        let swap = definition(&mut core, "Swap", 2);
        let swapped = core.enum_type(swap, vec![second, first]);
        core.enums[swap].variants = vec![VariantDef::positional("Next", vec![swapped])];
        let reset = definition(&mut core, "Reset", 2);
        let list = core.list(second);
        let next = core.enum_type(reset, vec![list, TypeId::BOOL]);
        core.enums[reset].variants = vec![VariantDef::positional("Next", vec![next])];
        assert!(expanding_enums(&core).is_empty());
    }

    #[test]
    fn direct_and_mutual_positive_parameter_cycles_require_lazy_edges() {
        let mut core = Module::new();
        let parameter = core.type_param(0);
        let direct = definition(&mut core, "Grow", 1);
        let list = core.list(parameter);
        let next = core.enum_type(direct, vec![list]);
        core.enums[direct].variants = vec![VariantDef::positional("Next", vec![next])];
        let first = definition(&mut core, "First", 1);
        let second = definition(&mut core, "Second", 1);
        let forward = core.enum_type(second, vec![list]);
        let back = core.enum_type(first, vec![parameter]);
        core.enums[first].variants = vec![VariantDef::positional("Forward", vec![forward])];
        core.enums[second].variants = vec![VariantDef::positional("Back", vec![back])];
        let growing = expanding_enums(&core);
        assert!(growing.contains(&direct));
        assert!(growing.contains(&first));
    }

    #[test]
    fn phantom_arguments_do_not_expose_hidden_enum_definitions() {
        let mut core = Module::new();
        let parameter = core.type_param(0);
        let first = definition(&mut core, "First", 1);
        let second = definition(&mut core, "Second", 1);
        let opaque = definition(&mut core, "Opaque", 1);
        core.enums[opaque].external = true;
        let list = core.list(parameter);
        let hidden = core.enum_type(second, vec![list]);
        let outer = core.enum_type(opaque, vec![hidden]);
        let back = core.enum_type(first, vec![parameter]);
        core.enums[first].variants = vec![VariantDef::positional("Hide", vec![outer])];
        core.enums[second].variants = vec![VariantDef::positional("Back", vec![back])];
        assert!(expanding_enums(&core).is_empty());
    }
}
