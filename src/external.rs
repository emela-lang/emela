use crate::ast::{Capability, PrimType, Type};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExternalImplementation {
    Runtime,
}

#[derive(Debug, Clone)]
pub(crate) struct ExternalFunction {
    pub(crate) path: &'static [&'static str],
    pub(crate) name: &'static str,
    pub(crate) params: &'static [Type],
    pub(crate) ret: Type,
    pub(crate) capabilities: &'static [Capability],
    pub(crate) implementation: ExternalImplementation,
}

const PRINT_I32_PARAMS: &[Type] = &[Type::Prim(PrimType::I32)];
const PRINT_BOOL_PARAMS: &[Type] = &[Type::Prim(PrimType::Bool)];
const CLOCK_NOW_PARAMS: &[Type] = &[];

const EXTERNAL_FUNCTIONS: &[ExternalFunction] = &[
    ExternalFunction {
        path: &["platform", "io"],
        name: "print_i32!",
        params: PRINT_I32_PARAMS,
        ret: Type::Prim(PrimType::Unit),
        capabilities: &[Capability::Stdout],
        implementation: ExternalImplementation::Runtime,
    },
    ExternalFunction {
        path: &["platform", "io"],
        name: "print_bool!",
        params: PRINT_BOOL_PARAMS,
        ret: Type::Prim(PrimType::Unit),
        capabilities: &[Capability::Stdout],
        implementation: ExternalImplementation::Runtime,
    },
    ExternalFunction {
        path: &["platform", "clock"],
        name: "now_i32!",
        params: CLOCK_NOW_PARAMS,
        ret: Type::Prim(PrimType::I32),
        capabilities: &[Capability::Clock],
        implementation: ExternalImplementation::Runtime,
    },
];

pub(crate) fn resolve_import(path: &[String], name: &str) -> Option<&'static ExternalFunction> {
    EXTERNAL_FUNCTIONS.iter().find(|function| {
        function.name == name
            && function.path.len() == path.len()
            && function
                .path
                .iter()
                .zip(path.iter())
                .all(|(left, right)| left == right)
    })
}
