use super::super::*;

pub(in crate::compiler::plan) fn result_ownership() -> ResultOwnership {
    ResultOwnership::Borrowed
}

pub(in crate::compiler::plan) fn needs_heap() -> bool {
    true
}
