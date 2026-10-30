use super::*;
pub(in crate::compiler::plan) fn function_ref_adapters(
    flow: &crate::contract::ir::Program,
    emitted: &HashSet<FnId>,
    layouts: &TypeLayouts<'_>,
) -> Vec<(FnId, TypeId)> {
    let mut adapters = HashSet::new();
    for (function_id, function) in flow.functions.iter() {
        if !emitted.contains(&function_id) {
            continue;
        }
        for (_, statement) in function.stmts.iter() {
            let Stmt::Let {
                dst: Some(dst),
                value: Value::FnRef(target),
                ..
            } = statement
            else {
                continue;
            };
            if !layouts.functions.closure_functions.contains_key(target) {
                adapters.insert((*target, function.vars[*dst].ty));
            }
        }
    }
    let mut adapters: Vec<_> = adapters.into_iter().collect();
    adapters.sort_by_key(|(target, function_type)| {
        (u32::from(target.into_raw()), function_type.index())
    });
    adapters
}
