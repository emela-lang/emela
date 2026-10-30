//! Source const origins carried through verified Flow IR as private, unreachable metadata.

use std::collections::{HashMap, HashSet};

use emela_core::{FnType, Type, TypeId};
use emela_lir::{FnBuilder, FnId, Module, Operand, Stmt, Term, Value};

use crate::{Error, ErrorKind};

const PREFIX: &str = "$emb_const_origins$v1$";
const FUNCTION_PREFIX: &str = "$emb_fn_origins$v1$";

#[derive(Debug, Clone)]
pub(crate) struct Group {
    pub(crate) canonical: FnId,
    pub(crate) specializations: Vec<FnId>,
}

pub(crate) struct Prepared {
    pub(crate) module: emela_core::Module,
    pub(crate) groups: Vec<Group>,
    pub(crate) function_groups: Vec<Vec<FnId>>,
}

pub(crate) fn monomorphize(core: &emela_core::Module) -> Prepared {
    let roots: Vec<_> = core
        .functions
        .iter()
        .filter_map(|(id, f)| f.exported.then_some(id))
        .collect();
    let mut mono = emela_core::monomorphize_traced(core, &roots);
    let mut shared = Vec::new();
    loop {
        let mut counts = HashMap::new();
        for (_, &origin) in mono.origins.iter() {
            let f = &core.functions[origin];
            if f.constant && f.type_params != 0 {
                *counts.entry(origin).or_insert(0) += 1;
            }
        }
        let mut needed: Vec<_> = counts
            .into_iter()
            .filter_map(|(id, count)| (count > 1).then_some(id))
            .collect();
        needed.extend(shared.iter().copied());
        needed.sort_by_key(|id| u32::from(id.into_raw()));
        needed.dedup();
        if needed == shared {
            break;
        }
        shared = needed;
        let all_roots: Vec<_> = roots
            .iter()
            .copied()
            .chain(shared.iter().copied())
            .collect();
        mono = emela_core::monomorphize_traced(core, &all_roots);
    }
    // A single specialization already has one cache and needs no erased representation bridge.
    if shared.is_empty() {
        return Prepared {
            module: mono.module,
            groups: vec![],
            function_groups: vec![],
        };
    }
    let groups = shared
        .iter()
        .enumerate()
        .map(|(index, &origin)| {
            let canonical = mono.roots[roots.len() + index];
            let specializations = mono
                .origins
                .iter()
                .filter_map(|(id, &source)| {
                    (source == origin && id != canonical).then_some(FnId::from_raw(id.into_raw()))
                })
                .collect();
            Group {
                canonical: FnId::from_raw(canonical.into_raw()),
                specializations,
            }
        })
        .collect();
    let mut ordinary: HashMap<_, Vec<FnId>> = HashMap::new();
    for (id, &origin) in mono.origins.iter() {
        if !core.functions[origin].constant && core.functions[origin].type_params != 0 {
            ordinary
                .entry(origin)
                .or_default()
                .push(FnId::from_raw(id.into_raw()));
        }
    }
    let mut ordinary: Vec<_> = ordinary
        .into_iter()
        .filter(|(_, ids)| ids.len() > 1)
        .collect();
    ordinary.sort_by_key(|(origin, _)| u32::from(origin.into_raw()));
    Prepared {
        module: mono.module,
        groups,
        function_groups: ordinary.into_iter().map(|(_, ids)| ids).collect(),
    }
}

pub(crate) fn attach(flow: &mut Module, groups: &[Group]) {
    for (index, group) in groups.iter().enumerate() {
        let types: Vec<_> = std::iter::once(group.canonical)
            .chain(group.specializations.iter().copied())
            .map(|id| {
                flow.core
                    .types
                    .intern(Type::Fn(FnType::pure(vec![], flow.functions[id].ret)))
            })
            .collect();
        let mut name = format!("{PREFIX}{index}");
        while flow.functions.iter().any(|(_, f)| f.name == name) {
            name.push('$');
        }
        let mut b = FnBuilder::new(&flow.core, &name);
        b.set_constant();
        for (index, (id, ty)) in std::iter::once(group.canonical)
            .chain(group.specializations.iter().copied())
            .zip(types)
            .enumerate()
        {
            b.assign(
                if index == 0 {
                    "canonical"
                } else {
                    "specialization"
                },
                ty,
                Value::FnRef(id),
            );
        }
        b.term(Term::Return(None));
        flow.add(b.finish());
    }
}

pub(crate) fn attach_function_groups(flow: &mut Module, groups: &[Vec<FnId>]) {
    for (index, ids) in groups.iter().enumerate() {
        let types: Vec<_> = ids
            .iter()
            .map(|&id| {
                let core_id = emela_core::FnId::from_raw(id.into_raw());
                let f = &flow.core.functions[core_id];
                let ev = flow.core.takes_ev(core_id);
                let declared = &f.params[..f.params.len() - usize::from(ev)];
                flow.core.types.intern(Type::Fn(FnType {
                    params: declared.iter().map(|&p| flow.core.locals[p].ty).collect(),
                    ret: f.ret,
                    fails: f.fails,
                    suspends: f.suspends,
                    ev,
                    sets: None,
                }))
            })
            .collect();
        let mut name = format!("{FUNCTION_PREFIX}{index}");
        while flow.functions.iter().any(|(_, f)| f.name == name) {
            name.push('$');
        }
        let mut b = FnBuilder::new(&flow.core, &name);
        b.set_constant();
        for (&id, ty) in ids.iter().zip(types) {
            b.assign("specialization", ty, Value::FnRef(id));
        }
        b.term(Term::Return(None));
        flow.add(b.finish());
    }
}

pub(crate) fn groups(flow: &Module) -> Result<Vec<Group>, Vec<Error>> {
    Ok(read_markers(flow, PREFIX, true)?
        .into_iter()
        .map(|ids| Group {
            canonical: ids[0],
            specializations: ids[1..].to_vec(),
        })
        .collect())
}

pub(crate) fn function_groups(flow: &Module) -> Result<Vec<Vec<FnId>>, Vec<Error>> {
    read_markers(flow, FUNCTION_PREFIX, false)
}

fn read_markers(
    flow: &Module,
    prefix: &str,
    constant_targets: bool,
) -> Result<Vec<Vec<FnId>>, Vec<Error>> {
    let mut result = Vec::new();
    let mut members = HashSet::new();
    for (_, marker) in flow
        .functions
        .iter()
        .filter(|(_, f)| f.name.starts_with(prefix))
    {
        let invalid = || {
            vec![Error {
                kind: ErrorKind::InvalidFlowIr,
                message: format!("invalid embedded const origin marker `{}`", marker.name),
            }]
        };
        if marker.name[prefix.len()..]
            .trim_end_matches('$')
            .parse::<usize>()
            .is_err()
            || marker.defects
            || !marker.labels.is_empty()
            || marker.exported
            || !marker.constant
            || !marker.params.is_empty()
            || marker.ret != TypeId::UNIT
            || marker.fails
            || marker.suspends
            || marker.external.is_some()
            || marker.env.is_some()
        {
            return Err(invalid());
        }
        let Some(body) = marker.body else {
            return Err(invalid());
        };
        let seq = &marker.seqs[body];
        if !matches!(
            seq.term,
            Term::Return(None) | Term::Return(Some(Operand::Const(emela_lir::Const::Unit)))
        ) || seq.stmts.len() < 2
            || marker.seqs.len() != 1
            || marker.stmts.len() != seq.stmts.len()
            || marker.vars.len() != seq.stmts.len()
        {
            return Err(invalid());
        }
        let mut ids = Vec::new();
        for &stmt in &seq.stmts {
            let Stmt::Let {
                dst: Some(dst),
                value: Value::FnRef(target),
                on_defect: None,
                on_suspend: None,
            } = marker.stmts[stmt]
            else {
                return Err(invalid());
            };
            if u32::from(target.into_raw()) as usize >= flow.functions.len() {
                return Err(invalid());
            }
            let target_function = &flow.functions[target];
            if (constant_targets
                && (!target_function.constant
                    || !target_function.params.is_empty()
                    || target_function.fails
                    || target_function.suspends
                    || target_function.external.is_some()))
                || (!constant_targets && target_function.constant)
                || !members.insert(target)
            {
                return Err(invalid());
            }
            let Type::Fn(ft) = &flow.core.types[marker.vars[dst].ty] else {
                return Err(invalid());
            };
            let ev = target_function
                .params
                .last()
                .is_some_and(|&p| target_function.vars[p].ty == TypeId::EVIDENCE);
            let expected: Vec<_> = target_function
                .params
                .iter()
                .map(|&p| target_function.vars[p].ty)
                .filter(|&t| t != TypeId::EVIDENCE)
                .collect();
            let actual: Vec<_> = ft
                .params
                .iter()
                .copied()
                .filter(|&t| emela_lir::repr_of(&flow.core, t) != emela_lir::Repr::Void)
                .collect();
            if actual != expected
                || ft.ret != target_function.ret
                || ft.fails != target_function.fails
                || ft.ev != ev
                || ft.suspends != target_function.suspends
                || ft.sets.is_some()
            {
                return Err(invalid());
            }
            ids.push(target);
        }
        result.push(ids);
    }
    Ok(result)
}
