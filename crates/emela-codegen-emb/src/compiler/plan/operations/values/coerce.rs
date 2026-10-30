use super::super::*;

pub(in crate::compiler::plan) fn needs_heap(
    dst: Option<VarId>,
    source: VarId,
    context: &HeapContext<'_, '_>,
) -> bool {
    dst.is_some_and(|dst| {
        matches!(
            context.layouts.shapes.types[context.function.vars[dst].ty],
            Type::Any
        ) && !matches!(
            context.layouts.shapes.types[context.function.vars[source].ty],
            Type::Any
        ) && context.layouts.runtime.any_box_required
    })
}
