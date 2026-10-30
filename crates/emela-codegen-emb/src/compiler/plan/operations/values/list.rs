use super::super::*;

pub(in crate::compiler::plan) fn result_ownership(
    elems: &[Operand],
    tail: &Option<Operand>,
) -> ResultOwnership {
    if elems.is_empty() && tail.is_some() {
        ResultOwnership::Borrowed
    } else {
        ResultOwnership::Owned
    }
}

pub(in crate::compiler::plan) fn is_known_empty(
    elems: &[Operand],
    tail: &Option<Operand>,
    known_empty: &HashSet<VarId>,
) -> bool {
    elems.is_empty()
        && match tail {
            None => true,
            Some(Operand::Var(var)) => known_empty.contains(var),
            _ => false,
        }
}

pub(in crate::compiler::plan) fn needs_heap(
    elems: &[Operand],
    tail: &Option<Operand>,
    context: &HeapContext<'_, '_>,
) -> bool {
    !is_known_empty(elems, tail, context.known_empty)
}
