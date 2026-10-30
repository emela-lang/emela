//! Existing interrupt entry policy over the shared compiler analysis.
use super::*;

pub(super) fn validate_isr_entries(
    flow: &crate::contract::ir::Program,
    emitted: &HashSet<FnId>,
    heap_functions: &HashSet<FnId>,
    layouts: &TypeLayouts<'_>,
    names: &HashMap<FnId, String>,
    options: &Options,
) -> Result<Vec<String>, Vec<Error>> {
    let mut required = HashSet::new();
    for entry_name in options.isr_entries() {
        let Some((entry, _)) = flow.functions.iter().find(|(id, function)| {
            emitted.contains(id) && function.exported && function.name == *entry_name
        }) else {
            return Err(vec![Error {
                kind: ErrorKind::InvalidOptions,
                message: format!(
                    "ISR entry `{entry_name}` must name an emitted exported Emela function"
                ),
            }]);
        };
        validate_isr_function(
            flow,
            entry,
            entry_name,
            heap_functions,
            layouts,
            names,
            options,
            &mut HashSet::new(),
            &mut HashSet::new(),
            &mut required,
        )?;
    }
    let mut required: Vec<_> = required.into_iter().collect();
    required.sort();
    Ok(required)
}

// Keep the lowering context explicit across this emission boundary.
#[allow(clippy::too_many_arguments)]
fn validate_isr_function(
    flow: &crate::contract::ir::Program,
    id: FnId,
    entry_name: &str,
    heap_functions: &HashSet<FnId>,
    layouts: &TypeLayouts<'_>,
    names: &HashMap<FnId, String>,
    options: &Options,
    active: &mut HashSet<FnId>,
    complete: &mut HashSet<FnId>,
    required: &mut HashSet<String>,
) -> Result<(), Vec<Error>> {
    if active.contains(&id) {
        return Err(unsupported(
            &flow.functions[id].name,
            &format!("recursive call reachable from ISR entry `{entry_name}`"),
        ));
    }
    if complete.contains(&id) {
        return Ok(());
    }
    let function = &flow.functions[id];
    if layouts.functions.runtime_consts.contains(&id) {
        return Err(unsupported(
            &function.name,
            &format!("runtime const initialization reachable from ISR entry `{entry_name}`"),
        ));
    }
    if function.suspends {
        return Err(unsupported(
            &function.name,
            &format!("suspending function reachable from ISR entry `{entry_name}`"),
        ));
    }
    if heap_functions.contains(&id)
        || std::iter::once(function.ret)
            .chain(function.vars.iter().map(|(_, variable)| variable.ty))
            .any(|ty| layouts.contains_heap_ref(ty, &mut HashSet::new()))
    {
        return Err(unsupported(
            &function.name,
            &format!("heap-backed value reachable from ISR entry `{entry_name}`"),
        ));
    }

    let reachable_statements = function.active_statements.clone();
    if function.stmts.iter().any(|(id, statement)| {
        reachable_statements.contains(&id)
            && matches!(
                statement,
                Stmt::Let {
                    value: Value::Runtime {
                        op: RuntimeOp::Hold | RuntimeOp::Unhold,
                        ..
                    },
                    ..
                }
            )
    }) {
        return Err(unsupported(
            &function.name,
            &format!("fiber interrupt control reachable from ISR entry `{entry_name}`"),
        ));
    }

    active.insert(id);
    for (_, statement) in function.stmts.iter() {
        let Stmt::Let {
            value: Value::Call {
                callee, suspends, ..
            },
            ..
        } = statement
        else {
            continue;
        };
        if *suspends {
            active.remove(&id);
            return Err(unsupported(
                &function.name,
                &format!("suspending call reachable from ISR entry `{entry_name}`"),
            ));
        }
        match callee {
            Callee::Direct(callee_id) => {
                let callee = &flow.functions[*callee_id];
                if callee.suspends {
                    active.remove(&id);
                    return Err(unsupported(
                        &function.name,
                        &format!("suspending call reachable from ISR entry `{entry_name}`"),
                    ));
                }
                if callee.external.is_some() {
                    let symbol =
                        external::isr_symbol(flow, callee, &names[callee_id], options, entry_name)?;
                    if !options
                        .isr_safe_externals()
                        .iter()
                        .any(|safe| safe == symbol)
                    {
                        active.remove(&id);
                        return Err(unsupported(
                            &function.name,
                            &format!(
                                "external C function `{symbol}` reachable from ISR entry `{entry_name}` is not marked ISR-safe"
                            ),
                        ));
                    }
                    required.insert(symbol.to_owned());
                } else if !callee.constant {
                    validate_isr_function(
                        flow,
                        *callee_id,
                        entry_name,
                        heap_functions,
                        layouts,
                        names,
                        options,
                        active,
                        complete,
                        required,
                    )?;
                }
            }
            Callee::Indirect(_) | Callee::Op { .. } => {
                active.remove(&id);
                return Err(unsupported(
                    &function.name,
                    &format!("dynamic call reachable from ISR entry `{entry_name}`"),
                ));
            }
        }
    }
    active.remove(&id);
    complete.insert(id);
    Ok(())
}
