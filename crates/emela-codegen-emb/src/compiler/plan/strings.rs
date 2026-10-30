use super::*;
use crate::contract::strings::StringBuffers as StringBufferPlan;
pub(in crate::compiler::plan) trait StringPlanning: Sized {
    fn new(
        layouts: &TypeLayouts<'_>,
        function_id: FnId,
        function: &Function,
        capacity: u16,
        output_calls: &HashSet<FnId>,
        returns_to_caller: bool,
    ) -> Self;
}
impl StringPlanning for StringBufferPlan {
    fn new(
        layouts: &TypeLayouts<'_>,
        function_id: FnId,
        function: &Function,
        capacity: u16,
        output_calls: &HashSet<FnId>,
        returns_to_caller: bool,
    ) -> Self {
        let (statement_positions, sequence_positions) = function_positions(function);
        let mut reuse_candidates = HashMap::new();
        let mut buffer_producers = HashSet::new();
        let mut producer_positions = HashMap::new();
        for (statement_id, statement) in function
            .stmts
            .iter()
            .filter(|(id, _)| statement_positions.contains_key(id))
        {
            let Stmt::Let {
                dst: Some(dst),
                value,
                ..
            } = statement
            else {
                continue;
            };
            let behavior = features::values::string_behavior(value);
            if behavior.needs_output(output_calls)
                && behavior.accepts(layouts, function.vars[*dst].ty)
            {
                if let Some(input) = behavior.reusable_input {
                    reuse_candidates.insert(*dst, input);
                }
                buffer_producers.insert(*dst);
                producer_positions.insert(*dst, statement_id);
            }
        }

        let mut uses = HashMap::new();
        let mut count = |variable| *uses.entry(variable).or_insert(0usize) += 1;
        for (_, statement) in function
            .stmts
            .iter()
            .filter(|(id, _)| statement_positions.contains_key(id))
        {
            match statement {
                Stmt::Let { value, .. } => for_each_string_plan_var(layouts, value, &mut count),
                Stmt::If { cond, .. } => count_operand(cond, &mut count),
                Stmt::Switch { on, .. } => match on {
                    SwitchOn::Tag(variable) | SwitchOn::Lit(variable) => count(*variable),
                },
                Stmt::Loop { inits, .. } => {
                    for operand in inits {
                        count_operand(operand, &mut count);
                    }
                }
                Stmt::Dup(variable) | Stmt::Drop(variable) => count(*variable),
                Stmt::Block { .. } | Stmt::Protect { .. } => {}
            }
        }
        for (_, sequence) in function
            .seqs
            .iter()
            .filter(|(id, _)| sequence_positions.contains_key(id))
        {
            match &sequence.term {
                Term::Yield(Some(operand))
                | Term::Return(Some(operand))
                | Term::Fail(operand)
                | Term::Defect(operand) => count_operand(operand, &mut count),
                Term::Yield(None) | Term::Return(None) => {}
                Term::Break { value, .. } => {
                    if let Some(value) = value {
                        count_operand(value, &mut count);
                    }
                }
                Term::Recur(args) => {
                    for operand in args {
                        count_operand(operand, &mut count);
                    }
                }
                Term::Unreachable | Term::Unwind => {}
            }
        }
        drop(count);

        let mut last_uses = HashMap::new();
        let mut non_reusable_variables = HashSet::new();
        for (statement_id, statement) in function
            .stmts
            .iter()
            .filter(|(id, _)| statement_positions.contains_key(id))
        {
            let position = statement_positions.get(&statement_id).copied().unwrap_or(0);
            let mut mark_use = |variable| {
                last_uses
                    .entry(variable)
                    .and_modify(|last: &mut usize| *last = (*last).max(position))
                    .or_insert(position);
            };
            match statement {
                Stmt::Let { value, .. } => {
                    for_each_string_plan_var(layouts, value, &mut mark_use);
                    if features::values::string_behavior(value).input_use == InputUse::Escapes {
                        for_each_string_plan_var(layouts, value, |variable| {
                            non_reusable_variables.insert(variable);
                        });
                    }
                }

                Stmt::If { cond, .. } => {
                    count_operand(cond, &mut mark_use);
                    if let Operand::Var(variable) = cond {
                        non_reusable_variables.insert(*variable);
                    }
                }
                Stmt::Switch { on, .. } => {
                    let variable = match on {
                        SwitchOn::Tag(variable) | SwitchOn::Lit(variable) => *variable,
                    };
                    mark_use(variable);
                    non_reusable_variables.insert(variable);
                }
                Stmt::Loop { inits, .. } => {
                    for operand in inits {
                        count_operand(operand, &mut mark_use);
                        if let Operand::Var(variable) = operand {
                            non_reusable_variables.insert(*variable);
                        }
                    }
                }
                Stmt::Dup(variable) | Stmt::Drop(variable) => {
                    mark_use(*variable);
                    non_reusable_variables.insert(*variable);
                }
                Stmt::Block { .. } | Stmt::Protect { .. } => {}
            }
        }
        for (sequence_id, sequence) in function
            .seqs
            .iter()
            .filter(|(id, _)| sequence_positions.contains_key(id))
        {
            let position = sequence_positions.get(&sequence_id).copied().unwrap_or(0);
            let mut mark_use = |variable| {
                last_uses
                    .entry(variable)
                    .and_modify(|last: &mut usize| *last = (*last).max(position))
                    .or_insert(position);
                non_reusable_variables.insert(variable);
            };
            match &sequence.term {
                Term::Yield(Some(operand))
                | Term::Return(Some(operand))
                | Term::Fail(operand)
                | Term::Defect(operand) => count_operand(operand, &mut mark_use),
                Term::Yield(None) | Term::Return(None) => {}
                Term::Break { value, .. } => {
                    if let Some(value) = value {
                        count_operand(value, &mut mark_use);
                    }
                }
                Term::Recur(args) => {
                    for operand in args {
                        count_operand(operand, &mut mark_use);
                    }
                }
                Term::Unreachable | Term::Unwind => {}
            }
        }

        let mut bounds: HashMap<VarId, usize> = function
            .vars
            .iter()
            .filter_map(|(id, variable)| {
                matches!(layouts.shapes.types[variable.ty], Type::String)
                    .then_some((id, usize::from(capacity)))
            })
            .collect();
        for _ in 0..=function.vars.len() {
            let mut changed = false;
            for (_, statement) in function
                .stmts
                .iter()
                .filter(|(id, _)| statement_positions.contains_key(id))
            {
                let Stmt::Let {
                    dst: Some(dst),
                    value,
                    ..
                } = statement
                else {
                    continue;
                };
                let is_string =
                    matches!(layouts.shapes.types[function.vars[*dst].ty], Type::String);
                let behavior = features::values::string_behavior(value);
                let is_nested_string = matches!(behavior.result, BufferResult::OptionalString)
                    && behavior.accepts(layouts, function.vars[*dst].ty);
                if !is_string && !is_nested_string {
                    continue;
                }
                let bound = features::values::string_bound(
                    value,
                    &StringBounds {
                        values: &bounds,
                        capacity,
                    },
                )
                .unwrap_or(usize::from(capacity));
                if bounds.get(dst).copied() != Some(bound) {
                    bounds.insert(*dst, bound);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        let mut roots: HashMap<VarId, VarId> =
            buffer_producers.iter().map(|&id| (id, id)).collect();
        for _ in 0..=reuse_candidates.len() {
            let mut changed = false;
            for (&dst, left) in &reuse_candidates {
                if uses.get(left).copied() != Some(1) || !roots.contains_key(left) {
                    continue;
                }
                let root = roots[left];
                if roots.get(&dst).copied() != Some(root) {
                    roots.insert(dst, root);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        let returned_root = if returns_to_caller {
            function
                .body
                .map(|body| &function.seqs[body].term)
                .and_then(|term| match term {
                    Term::Return(Some(Operand::Var(variable))) => roots.get(variable).copied(),
                    _ => None,
                })
        } else {
            None
        };

        let mut root_capacities: HashMap<VarId, usize> = HashMap::new();
        let mut root_intervals: HashMap<VarId, (usize, usize)> = HashMap::new();
        let mut non_reusable_roots = HashSet::new();
        for (&variable, &root) in &roots {
            let bound = bounds
                .get(&variable)
                .copied()
                .unwrap_or(usize::from(capacity))
                .min(usize::from(capacity));
            root_capacities
                .entry(root)
                .and_modify(|current| *current = (*current).max(bound))
                .or_insert(bound);
            let definition = producer_positions
                .get(&variable)
                .and_then(|statement| statement_positions.get(statement))
                .copied()
                .unwrap_or(0);
            let end = last_uses.get(&variable).copied().unwrap_or(definition);
            root_intervals
                .entry(root)
                .and_modify(|(start, last)| {
                    *start = (*start).min(definition);
                    *last = (*last).max(end);
                })
                .or_insert((definition, end));
            if non_reusable_variables.contains(&variable) {
                non_reusable_roots.insert(root);
            }
        }
        let mut groups: Vec<_> = root_capacities
            .iter()
            .filter(|(root, _)| Some(**root) != returned_root)
            .map(|(&root, &size)| {
                let (start, end) = root_intervals.get(&root).copied().unwrap_or((0, 0));
                (root, start, end, size, !non_reusable_roots.contains(&root))
            })
            .collect();
        groups.sort_by_key(|(root, start, _, _, _)| (*start, format!("{root:?}")));

        let mut slot_ends: Vec<Option<usize>> = Vec::new();
        let mut slot_sizes: Vec<usize> = Vec::new();
        let mut root_slots = HashMap::new();
        for (root, start, end, size, reusable) in groups {
            let slot = reusable
                .then(|| {
                    slot_ends
                        .iter()
                        .position(|last| last.is_some_and(|last| last < start))
                })
                .flatten();
            let slot = if let Some(slot) = slot {
                slot_ends[slot] = Some(end);
                slot_sizes[slot] = slot_sizes[slot].max(size.max(1));
                slot
            } else {
                let slot = slot_ends.len();
                slot_ends.push(reusable.then_some(end));
                slot_sizes.push(size.max(1));
                slot
            };
            root_slots.insert(root, slot);
        }
        let declarations = slot_sizes
            .iter()
            .enumerate()
            .map(|(slot, size)| (string_buffer_name(slot), *size))
            .collect();
        let buffers = roots
            .into_iter()
            .map(|(variable, root)| {
                let buffer_capacity = root_capacities[&root] as u16;
                (
                    variable,
                    (
                        (Some(root) != returned_root).then(|| root_slots[&root]),
                        buffer_capacity,
                    ),
                )
            })
            .collect();
        let result_ownership = function
            .stmts
            .iter()
            .filter_map(|(_, statement)| match statement {
                Stmt::Let {
                    dst: Some(dst),
                    value,
                    ..
                } => Some((*dst, features::values::result_ownership(value))),
                _ => None,
            })
            .collect();
        let append_modes = function
            .stmts
            .iter()
            .filter_map(|(_, statement)| match statement {
                Stmt::Let {
                    dst: Some(dst),
                    value,
                    ..
                } => features::values::append_mode(value).map(|mode| (*dst, mode)),
                _ => None,
            })
            .collect();
        Self {
            result_ownership,
            append_modes,
            discarded_results: layouts
                .functions
                .discarded_results
                .get(&function_id)
                .cloned()
                .unwrap_or_default(),
            buffers,
            declarations,
            output_calls: output_calls.clone(),
            returns_to_caller,
            capacity,
        }
    }
}
pub(in crate::compiler::plan) fn for_each_string_plan_var(
    layouts: &TypeLayouts<'_>,
    value: &Value,
    mut visit: impl FnMut(VarId),
) {
    features::values::for_each_plan_var(layouts, value, &mut visit);
}

pub(in crate::compiler::plan) fn count_operand(operand: &Operand, count: &mut impl FnMut(VarId)) {
    if let Operand::Var(variable) = operand {
        count(*variable);
    }
}

fn string_buffer_name(slot: usize) -> String {
    format!("emela_string_buffer_{slot}")
}

pub(in crate::compiler::plan) fn functions_with_string_output(
    flow: &crate::contract::ir::Program,
    emitted: &HashSet<FnId>,
    layouts: &TypeLayouts<'_>,
) -> HashSet<FnId> {
    let mut result = HashSet::new();
    loop {
        let mut changed = false;
        for (id, function) in flow.functions.iter() {
            if !emitted.contains(&id)
                || function.constant
                || function.suspends
                || function.external.is_some()
                || layouts
                    .functions
                    .owned_string_returns
                    .contains(&function.name)
                || !matches!(layouts.shapes.types[function.ret], Type::String)
                || result.contains(&id)
            {
                continue;
            }
            let has_output_producer = function.stmts.iter().any(|(_, statement)| {
                let Stmt::Let { value, .. } = statement else {
                    return false;
                };
                features::values::string_behavior(value).needs_output(&result)
            });
            if has_output_producer {
                result.insert(id);
                changed = true;
            }
        }
        if !changed {
            return result;
        }
    }
}
