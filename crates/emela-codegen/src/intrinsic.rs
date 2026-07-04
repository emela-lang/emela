//! The intrinsic-function interface (spec 0021).
//!
//! Intrinsics are the pure analog of platform functions (spec 0013): the
//! standard library declares them with `intrinsic fn`, and each backend inlines
//! them to a native instruction. The compiler holds no semantics for them beyond
//! their signature here and the per-backend name -> instruction table; this is
//! how primitive behaviour (e.g. `Int` addition) moves out of the compiler and
//! into stdlib, which wraps the intrinsic in an `impl`/`pub fn`.
//!
//! Unlike platform functions, an intrinsic is identified by its bare name (the
//! operation identity is module-independent) and is always pure (`uses {}`).

use crate::types::Type;

/// One entry of the intrinsic interface: a bare name and a signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntrinsicFn {
    pub name: String,
    pub params: Vec<Type>,
    pub ret: Type,
}

/// The normative intrinsic interface. The initial set covers the primitive
/// arithmetic and comparison operations on `Int` and `Float`, which lets stdlib
/// provide the operator instances of spec 0020 without the compiler hard-coding
/// operator semantics.
pub fn intrinsic_interface() -> Vec<IntrinsicFn> {
    fn int2(name: &str, ret: Type) -> IntrinsicFn {
        IntrinsicFn {
            name: name.to_string(),
            params: vec![Type::Int, Type::Int],
            ret,
        }
    }
    fn float2(name: &str, ret: Type) -> IntrinsicFn {
        IntrinsicFn {
            name: name.to_string(),
            params: vec![Type::Float, Type::Float],
            ret,
        }
    }
    vec![
        int2("i32_add", Type::Int),
        int2("i32_sub", Type::Int),
        int2("i32_mul", Type::Int),
        int2("i32_div_s", Type::Int),
        int2("i32_rem_s", Type::Int),
        int2("i32_eq", Type::Bool),
        int2("i32_lt_s", Type::Bool),
        float2("f64_add", Type::Float),
        float2("f64_sub", Type::Float),
        float2("f64_mul", Type::Float),
        float2("f64_div", Type::Float),
        float2("f64_eq", Type::Bool),
        float2("f64_lt", Type::Bool),
        // String concatenation (spec 0017/0021), the intrinsic behind `++`.
        IntrinsicFn {
            name: "string_concat".to_string(),
            params: vec![Type::String, Type::String],
            ret: Type::String,
        },
        // String equality and lexicographic order, the intrinsics behind
        // `Eq`/`Ord for String`. Comparison is over bytes (UTF-8), i.e. code
        // point order.
        IntrinsicFn {
            name: "string_eq".to_string(),
            params: vec![Type::String, Type::String],
            ret: Type::Bool,
        },
        IntrinsicFn {
            name: "string_lt".to_string(),
            params: vec![Type::String, Type::String],
            ret: Type::Bool,
        },
    ]
}

/// Looks an intrinsic up by its bare name (e.g. `i32_add`).
pub fn lookup(name: &str) -> Option<IntrinsicFn> {
    intrinsic_interface()
        .into_iter()
        .find(|entry| entry.name == name)
}

/// Whether `name` is part of the normative intrinsic interface (spec 0021).
///
/// Backends use this for their intrinsic-coverage check so the set of intrinsic
/// names has a single source of truth here rather than a hard-coded copy in
/// each backend.
pub fn is_intrinsic(name: &str) -> bool {
    lookup(name).is_some()
}
