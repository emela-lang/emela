use crate::contract::ir::{TypeId, VariantDef, VariantFields};

pub(crate) fn unit() -> usize {
    0
}
pub(crate) fn positional(count: usize) -> usize {
    count
}
pub(crate) fn named(names: &[String]) -> usize {
    names.len()
}

pub(crate) fn arity(fields: &VariantFields) -> usize {
    match fields {
        VariantFields::Unit => unit(),
        VariantFields::Positional(count) => positional(*count),
        VariantFields::Named(names) => named(names),
    }
}

pub(crate) fn types(variant: &VariantDef) -> Vec<TypeId> {
    let mut result = Vec::with_capacity(arity(&variant.fields));
    result.extend_from_slice(&variant.tys);
    result
}
