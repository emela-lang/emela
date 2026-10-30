use super::super::*;

pub(in crate::compiler::plan) fn needs_heap(target: FnId, context: &HeapContext<'_, '_>) -> bool {
    context.string_outputs.contains(&target)
}

pub(in crate::compiler::plan) fn for_each_capture(
    layouts: &TypeLayouts<'_>,
    target: FnId,
    mut visit: impl FnMut(VarId),
) {
    if let Some(&environment) = layouts.functions.closure_functions.get(&target) {
        for capture in &layouts.functions.closure_environments[environment].captures {
            visit(capture.source);
        }
    }
}
