use super::super::*;

pub(in crate::compiler::plan) fn needs_heap(op: Builtin) -> bool {
    matches!(op, Builtin::CellNew)
}
