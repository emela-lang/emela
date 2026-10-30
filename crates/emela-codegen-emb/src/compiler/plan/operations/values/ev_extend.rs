use super::super::*;

pub(in crate::compiler::plan) fn result_ownership(is_empty: bool) -> ResultOwnership {
    if is_empty {
        ResultOwnership::Borrowed
    } else {
        ResultOwnership::Owned
    }
}

pub(in crate::compiler::plan) fn needs_heap(is_empty: bool) -> bool {
    !is_empty
}
