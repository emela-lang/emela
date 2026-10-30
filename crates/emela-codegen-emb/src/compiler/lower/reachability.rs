//! Runtime reachability from public entries and explicit connection roots.
use super::runtime_statements;
use emela_core::Type;
use emela_lir::{Callee, FnId, Stmt, Value};
use std::collections::HashSet;

pub(crate) fn runtime_reachable(flow: &emela_lir::Module) -> HashSet<FnId> {
    runtime_reachable_from(flow, &[])
}

pub(crate) fn runtime_reachable_from(
    flow: &emela_lir::Module,
    extra_roots: &[FnId],
) -> HashSet<FnId> {
    let mut pending: Vec<FnId> = flow
        .functions
        .iter()
        .filter_map(|(id, function)| function.exported.then_some(id))
        .chain(extra_roots.iter().copied())
        .collect();
    if pending.is_empty() {
        pending.extend(
            flow.functions
                .iter()
                .filter_map(|(id, function)| (!function.constant).then_some(id)),
        );
    }

    let mut reachable = HashSet::new();
    loop {
        while let Some(id) = pending.pop() {
            if !reachable.insert(id) {
                continue;
            }
            let function = &flow.functions[id];
            if function.constant {
                continue;
            }
            let active = runtime_statements(function);
            for (_, statement) in function.stmts.iter().filter(|(id, _)| active.contains(id)) {
                if let Stmt::Let { dst, value, .. } = statement {
                    match value {
                        Value::Call {
                            callee: Callee::Direct(callee),
                            ..
                        } => pending.push(*callee),
                        Value::FnRef(callee) if dst.is_some() => pending.push(*callee),
                        _ => {}
                    }
                }
            }
        }
        let shows_causes = reachable.iter().any(|id| {
            flow.functions[*id].stmts.iter().any(|(_, stmt)| {
                matches!(
                    stmt,
                    Stmt::Let {
                        value: Value::Fiber { .. }
                            | Value::Call {
                                callee: Callee::Op { .. },
                                suspends: true,
                                ..
                            },
                        ..
                    }
                )
            })
        });
        if shows_causes {
            for &(error, show) in &flow.error_shows {
                if !reachable.contains(&show)
                    && reachable.iter().any(|id| {
                        flow.functions[*id].vars.iter().any(
                        |(_, v)| matches!(flow.core.types[v.ty], Type::Enum(e, _) if e == error),
                    )
                    })
                {
                    pending.push(show);
                }
            }
        }
        if pending.is_empty() {
            break;
        }
    }
    reachable
}
