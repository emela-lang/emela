use super::super::*;

pub(in crate::compiler::plan) fn string_bound(value: &Const) -> Option<usize> {
    match value {
        Const::String(value) => Some(value.len()),
        Const::Int(_) | Const::Int64(_) | Const::Float(_) | Const::Bool(_) | Const::Unit => None,
    }
}
