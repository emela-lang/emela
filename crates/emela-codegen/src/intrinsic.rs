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
///
/// `type_params` is non-empty for a generic intrinsic (spec 0021), whose
/// signature is written over type variables (`Type::Var`), e.g.
/// `array_get<T>(a: Array<T>, i: Int) -> T`. A call is monomorphized like a
/// generic function (spec 0014) before it reaches the IR, so `Type::Var` never
/// survives into `IrExpr::Intrinsic`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntrinsicFn {
    pub name: String,
    pub type_params: Vec<String>,
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
            type_params: Vec::new(),
            params: vec![Type::Int, Type::Int],
            ret,
        }
    }
    fn float2(name: &str, ret: Type) -> IntrinsicFn {
        IntrinsicFn {
            name: name.to_string(),
            type_params: Vec::new(),
            params: vec![Type::Float, Type::Float],
            ret,
        }
    }
    // A generic intrinsic over one type parameter `T` (spec 0021), used for the
    // `Array` operations. `params`/`ret` are written over `Type::Var("T")`.
    fn generic1(name: &str, params: Vec<Type>, ret: Type) -> IntrinsicFn {
        IntrinsicFn {
            name: name.to_string(),
            type_params: vec!["T".to_string()],
            params,
            ret,
        }
    }
    let array_t = || Type::Array(Box::new(Type::Var("T".to_string())));
    vec![
        int2("i32_add", Type::Int),
        int2("i32_sub", Type::Int),
        int2("i32_mul", Type::Int),
        int2("i32_div_s", Type::Int),
        int2("i32_rem_s", Type::Int),
        // Bitwise / shift operations on `Int` (spec 0053), the intrinsics behind
        // `& | ^ << >> >>>`. `i32_shr_s` is arithmetic, `i32_shr_u` logical.
        int2("i32_and", Type::Int),
        int2("i32_or", Type::Int),
        int2("i32_xor", Type::Int),
        int2("i32_shl", Type::Int),
        int2("i32_shr_s", Type::Int),
        int2("i32_shr_u", Type::Int),
        int2("i32_eq", Type::Bool),
        int2("i32_lt_s", Type::Bool),
        float2("f64_add", Type::Float),
        float2("f64_sub", Type::Float),
        float2("f64_mul", Type::Float),
        float2("f64_div", Type::Float),
        float2("f64_eq", Type::Bool),
        float2("f64_lt", Type::Bool),
        // Square root (spec 0030 companion), the intrinsic behind `std.float.sqrt`.
        IntrinsicFn {
            name: "f64_sqrt".to_string(),
            type_params: Vec::new(),
            params: vec![Type::Float],
            ret: Type::Float,
        },
        // String concatenation (spec 0017/0021), the intrinsic behind `++`.
        IntrinsicFn {
            name: "string_concat".to_string(),
            type_params: Vec::new(),
            params: vec![Type::String, Type::String],
            ret: Type::String,
        },
        // String equality and lexicographic order, the intrinsics behind
        // `Eq`/`Ord for String`. Comparison is over bytes (UTF-8), i.e. code
        // point order.
        IntrinsicFn {
            name: "string_eq".to_string(),
            type_params: Vec::new(),
            params: vec![Type::String, Type::String],
            ret: Type::Bool,
        },
        IntrinsicFn {
            name: "string_lt".to_string(),
            type_params: Vec::new(),
            params: vec![Type::String, Type::String],
            ret: Type::Bool,
        },
        // String scalar operations (spec 0030). Indices, lengths and slice
        // bounds are all in Unicode scalar (code point) units, never bytes.
        // `char_code` is the inverse of `char_from_code` (spec 0017).
        IntrinsicFn {
            name: "string_length".to_string(),
            type_params: Vec::new(),
            params: vec![Type::String],
            ret: Type::Int,
        },
        IntrinsicFn {
            name: "string_char_at".to_string(),
            type_params: Vec::new(),
            params: vec![Type::String, Type::Int],
            ret: Type::Char,
        },
        IntrinsicFn {
            name: "string_slice".to_string(),
            type_params: Vec::new(),
            params: vec![Type::String, Type::Int, Type::Int],
            ret: Type::String,
        },
        IntrinsicFn {
            name: "char_code".to_string(),
            type_params: Vec::new(),
            params: vec![Type::Char],
            ret: Type::Int,
        },
        // Char/String conversions (spec 0017), formerly the `Char::from_code` /
        // `String::from_char` builtins, now bare Core Prelude intrinsics.
        // `char_from_code` is the inverse of `char_code`.
        IntrinsicFn {
            name: "char_from_code".to_string(),
            type_params: Vec::new(),
            params: vec![Type::Int],
            ret: Type::Char,
        },
        IntrinsicFn {
            name: "string_from_char".to_string(),
            type_params: Vec::new(),
            params: vec![Type::Char],
            ret: Type::String,
        },
        // Byte-sequence operations (spec 0051). `Bytes` shares `String`'s
        // `[len][bytes]` representation but counts/indexes in bytes.
        // `bytes_from_string` is the identity on that representation (UTF-8
        // encode is a no-op); `bytes_concat`/`bytes_eq` reuse the string helpers.
        IntrinsicFn {
            name: "bytes_length".to_string(),
            type_params: Vec::new(),
            params: vec![Type::Bytes],
            ret: Type::Int,
        },
        IntrinsicFn {
            name: "bytes_get_unchecked".to_string(),
            type_params: Vec::new(),
            params: vec![Type::Bytes, Type::Int],
            ret: Type::Int,
        },
        IntrinsicFn {
            name: "bytes_slice".to_string(),
            type_params: Vec::new(),
            params: vec![Type::Bytes, Type::Int, Type::Int],
            ret: Type::Bytes,
        },
        IntrinsicFn {
            name: "bytes_concat".to_string(),
            type_params: Vec::new(),
            params: vec![Type::Bytes, Type::Bytes],
            ret: Type::Bytes,
        },
        IntrinsicFn {
            name: "bytes_eq".to_string(),
            type_params: Vec::new(),
            params: vec![Type::Bytes, Type::Bytes],
            ret: Type::Bool,
        },
        IntrinsicFn {
            name: "bytes_from_string".to_string(),
            type_params: Vec::new(),
            params: vec![Type::String],
            ret: Type::Bytes,
        },
        // The unchecked `Bytes` -> `String` reinterpret (spec 0051 B7). The safe
        // `std.bytes.string_from_bytes` validates UTF-8 first (in Emela) and only
        // then calls this. On the wasm backend it is the identity (the
        // representation is shared); on JS it is a `TextDecoder` decode.
        IntrinsicFn {
            name: "bytes_as_string_unchecked".to_string(),
            type_params: Vec::new(),
            params: vec![Type::Bytes],
            ret: Type::String,
        },
        // Array operations (spec 0007), formerly the `Array::length` /
        // `Array::get` / `Array::push` builtins, now bare generic Core Prelude
        // intrinsics. The element type is a type variable `T` monomorphized at
        // each call site (spec 0021 generic intrinsics). `array_get_unchecked`
        // is the raw accessor requiring `0 <= i < array_length(a)`; the safe
        // `array_get` returning `Option<T>` is a stdlib `pub fn` wrapper over it.
        generic1("array_length", vec![array_t()], Type::Int),
        generic1(
            "array_get_unchecked",
            vec![array_t(), Type::Int],
            Type::Var("T".to_string()),
        ),
        generic1(
            "array_push",
            vec![array_t(), Type::Var("T".to_string())],
            array_t(),
        ),
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
