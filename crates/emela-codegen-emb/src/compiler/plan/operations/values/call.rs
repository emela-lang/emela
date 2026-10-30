use super::super::*;

pub(in crate::compiler::plan) fn lower(
    context: &mut LoweringContext<'_>,
    id: StmtId,
    value: &Value,
    next: usize,
) -> Result<usize, Vec<Error>> {
    let Value::Call { on_fail, .. } = value else {
        unreachable!("value graph entry")
    };
    let graph = &mut *context.graph;
    let next = if let Some(edge) = on_fail {
        let failed = graph.failure(context.function, *edge)?;
        graph.add(Node::CheckError {
            normal: next,
            failed,
            slot: graph.error_depth,
        })
    } else {
        next
    };
    Ok(graph.add(Node::Statement(id, next)))
}

pub(in crate::compiler::plan) fn string_behavior(callee: &Callee) -> StringBehavior {
    match callee {
        Callee::Direct(id) => StringBehavior {
            output: StringOutput::Call(*id),
            ..StringBehavior::default()
        },
        Callee::Indirect(_) | Callee::Op { .. } => StringBehavior::default(),
    }
}

pub(in crate::compiler::plan) fn require_runtime(
    callee: &Callee,
    suspends: bool,
    on_fail: &Option<OnFail>,
    requirements: &mut crate::contract::runtime::RuntimeRequirements,
) {
    match callee {
        Callee::Indirect(_) => requirements.defects |= !suspends && on_fail.is_none(),
        Callee::Direct(_) | Callee::Op { .. } => {}
    }
}

pub(in crate::compiler::plan) fn needs_heap(callee: &Callee) -> bool {
    match callee {
        Callee::Op { .. } => true,
        Callee::Direct(_) | Callee::Indirect(_) => false,
    }
}

pub(in crate::compiler::plan) fn is_known_empty(
    callee: &Callee,
    empty_list_functions: &HashSet<FnId>,
) -> bool {
    match callee {
        Callee::Direct(id) => empty_list_functions.contains(id),
        Callee::Indirect(_) | Callee::Op { .. } => false,
    }
}
