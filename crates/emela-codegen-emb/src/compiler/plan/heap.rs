//! Combine feature allocation requirements with type, failure, and closure information.
use super::*;

// A fixed error can coexist with managed error variants used only by main. Follow
// failure edges instead of assuming every fallible function can construct every tag.
pub(in crate::compiler::plan) fn fixed_failure_graph(
    flow: &crate::contract::ir::Program,
    id: FnId,
    layouts: &TypeLayouts<'_>,
    active: &mut HashSet<FnId>,
) -> bool {
    if !active.insert(id) {
        return false;
    }
    let f = &flow.functions[id];
    if f.external.is_some() && f.fails {
        active.remove(&id);
        return false;
    }
    let mut fixed = true;
    if f.vars.iter().any(|(_, data)| matches!(layouts.shapes.types[data.ty], Type::Enum(error, _) if layouts.shapes.core.enums[error].error && layouts.is_managed_value(data.ty))) {
        fixed = false;
    }
    // Any is heap-free here only as a caught failure from the checked graph.
    // An Any parameter, returned box, or unrelated dynamic value is not such a proof.
    for (var, data) in f.vars.iter() {
        if matches!(layouts.shapes.types[data.ty], Type::Any)
            && !f
                .stmts
                .iter()
                .any(|(_, stmt)| matches!(stmt, Stmt::Block { dst: Some(dst), .. } if *dst == var))
        {
            fixed = false;
        }
    }
    for (_, sequence) in f.seqs.iter() {
        if let Term::Fail(Operand::Var(var)) = &sequence.term {
            let ty = f.vars[*var].ty;
            if !matches!(layouts.shapes.types[ty], Type::Any) && layouts.is_managed_value(ty) {
                fixed = false;
            }
        }
    }
    for (_, statement) in f.stmts.iter() {
        if let Stmt::Let {
            value:
                Value::Call {
                    callee,
                    on_fail: Some(_),
                    ..
                },
            ..
        } = statement
        {
            fixed &= match callee {
                Callee::Direct(target) => fixed_failure_graph(flow, *target, layouts, active),
                _ => false,
            };
        }
    }
    active.remove(&id);
    fixed
}

pub(in crate::compiler::plan) fn function_needs_heap(
    flow: &crate::contract::ir::Program,
    function_id: FnId,
    function: &Function,
    layouts: &TypeLayouts<'_>,
    closures: &crate::contract::lowered::ClosureMetadata,
    string_output_functions: &HashSet<FnId>,
    empty_list_functions: &HashSet<FnId>,
) -> bool {
    let fixed_errors = fixed_failure_graph(flow, function_id, layouts, &mut HashSet::new());
    let mut known_empty = HashSet::new();
    for (_, statement) in function.stmts.iter() {
        if let Stmt::Let {
            dst: Some(dst),
            value,
            ..
        } = statement
            && features::values::is_known_empty(value, &known_empty, empty_list_functions)
        {
            known_empty.insert(*dst);
        }
    }
    let context = HeapContext {
        layouts,
        function,
        string_outputs: string_output_functions,
        known_empty: &known_empty,
    };
    let statement_needs_heap = function.stmts.iter().any(|(_, statement)| {
        matches!(statement, Stmt::Let { dst, value, .. }
            if features::values::needs_heap(value, *dst, &context))
    });
    let returns_managed_alias = matches!(layouts.shapes.types[function.ret], Type::List(_))
        && function.seqs.iter().any(|(_, sequence)| {
            matches!(
                sequence.term,
                Term::Return(Some(Operand::Var(variable)))
                    if function.vars[variable].ty == function.ret
                        && !known_empty.contains(&variable)
            )
        });
    let local_managed_value = function.vars.iter().any(|(variable, data)| {
        !function.params.contains(&variable)
            && !(fixed_errors && matches!(layouts.shapes.types[data.ty], Type::Any))
            && (layouts.contains_heap_ref(data.ty, &mut HashSet::new())
                || matches!(layouts.shapes.types[data.ty], Type::Enum(error, _) if layouts.shapes.core.enums[error].error && layouts.is_managed_value(data.ty)))
            && !matches!(layouts.shapes.types[data.ty], Type::Handler(_) | Type::Evidence)
            && !known_empty.contains(&variable)
    });
    let returns_managed_aggregate = !matches!(layouts.shapes.types[function.ret], Type::List(_))
        && layouts.contains_heap_ref(function.ret, &mut HashSet::new());
    (function.fails && layouts.runtime.managed_error_payloads && !fixed_errors)
        || layouts
            .functions
            .owned_aggregate_returns
            .contains(&function.name)
        || (layouts.functions.runtime_consts.contains(&function_id)
            && layouts
                .functions
                .owned_string_returns
                .contains(&function.name))
        || statement_needs_heap
        || (function.suspends
            && (layouts.has_borrowed_views(function.ret)
                || function
                    .vars
                    .iter()
                    .any(|(_, var)| layouts.has_borrowed_views(var.ty))))
        || returns_managed_alias
        || local_managed_value
        || returns_managed_aggregate
        || closures
            .environments
            .iter()
            .any(|environment| environment.parent == function_id)
}
