//! Stable C identifiers for generated types, functions, and local values.
use crate::contract::ir::{EnumId, TypeId};
use crate::contract::ir::{FnId, Function, VarId};

pub(crate) fn is_c_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic())
        || !chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
    {
        return false;
    }
    !matches!(
        value,
        "auto"
            | "break"
            | "case"
            | "char"
            | "const"
            | "continue"
            | "default"
            | "do"
            | "double"
            | "else"
            | "enum"
            | "extern"
            | "float"
            | "for"
            | "goto"
            | "if"
            | "inline"
            | "int"
            | "long"
            | "register"
            | "restrict"
            | "return"
            | "short"
            | "signed"
            | "sizeof"
            | "static"
            | "struct"
            | "switch"
            | "typedef"
            | "union"
            | "unsigned"
            | "void"
            | "volatile"
            | "while"
            | "_Alignas"
            | "_Alignof"
            | "_Atomic"
            | "_Bool"
            | "_Complex"
            | "_Generic"
            | "_Imaginary"
            | "_Noreturn"
            | "_Static_assert"
            | "_Thread_local"
    )
}

pub(crate) fn type_name(ty: TypeId) -> String {
    format!("emela_type_{}", ty.index())
}

pub(crate) fn function_type_name(ty: TypeId) -> String {
    format!("emela_function_type_{}", ty.index())
}

pub(crate) fn function_name(id: FnId, function: &Function) -> String {
    let index = u32::from(id.into_raw());
    if function.exported {
        format!("emela_export_{}_{}", c_identifier(&function.name), index)
    } else {
        format!("emela_fn_{index}")
    }
}

pub(crate) fn enum_index(id: EnumId) -> u32 {
    u32::from(id.into_raw())
}

pub(crate) fn c_identifier(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "function".to_owned()
    } else {
        sanitized
    }
}

pub(crate) fn variable_name(id: VarId) -> String {
    format!("emela_v_{}", u32::from(id.into_raw()))
}

pub(crate) fn owned_variable_name(id: VarId) -> String {
    format!("emela_owned_v_{}", u32::from(id.into_raw()))
}

pub(crate) fn function_ref_adapter_name(target: FnId, function_type: TypeId) -> String {
    format!(
        "emela_fn_ref_adapter_{}_{}",
        u32::from(target.into_raw()),
        function_type.index()
    )
}

pub(crate) fn function_coerce_adapter_name(index: usize) -> String {
    format!("emela_fn_coerce_adapter_{index}")
}

pub(crate) fn is_reserved_alias(value: &str) -> bool {
    // Protect generated declarations, header guards, and identifiers supplied by <stdint.h>.
    value.starts_with('_')
        || value.starts_with("emela_")
        || value.starts_with("Emela")
        || value.starts_with("EMELA_")
        || is_stdint_identifier(value)
        || matches!(
            value,
            "main"
                | "NULL"
                | "bool"
                | "true"
                | "false"
                | "size_t"
                | "ptrdiff_t"
                | "wchar_t"
                | "sig_atomic_t"
                | "max_align_t"
                | "offsetof"
        )
}

pub(crate) fn is_c_keyword(value: &str) -> bool {
    matches!(
        value,
        "auto"
            | "break"
            | "case"
            | "char"
            | "const"
            | "continue"
            | "default"
            | "do"
            | "double"
            | "else"
            | "enum"
            | "extern"
            | "float"
            | "for"
            | "goto"
            | "if"
            | "inline"
            | "int"
            | "long"
            | "register"
            | "restrict"
            | "return"
            | "short"
            | "signed"
            | "sizeof"
            | "static"
            | "struct"
            | "switch"
            | "typedef"
            | "union"
            | "unsigned"
            | "void"
            | "volatile"
            | "while"
            | "_Alignas"
            | "_Alignof"
            | "_Atomic"
            | "_Bool"
            | "_Complex"
            | "_Generic"
            | "_Imaginary"
            | "_Noreturn"
            | "_Static_assert"
            | "_Thread_local"
    )
}

pub(crate) fn is_stdint_identifier(value: &str) -> bool {
    for (type_prefix, macro_prefix) in [("int", "INT"), ("uint", "UINT")] {
        for suffix in [
            "8", "16", "32", "64", "_least8", "_least16", "_least32", "_least64", "_fast8",
            "_fast16", "_fast32", "_fast64", "ptr", "max",
        ] {
            if value == format!("{type_prefix}{suffix}_t")
                || value == format!("{macro_prefix}{}_MIN", suffix.to_ascii_uppercase())
                || value == format!("{macro_prefix}{}_MAX", suffix.to_ascii_uppercase())
            {
                return true;
            }
        }
        for suffix in ["8", "16", "32", "64", "MAX"] {
            if value == format!("{macro_prefix}{suffix}_C") {
                return true;
            }
        }
    }
    matches!(
        value,
        "SIG_ATOMIC_MIN"
            | "SIG_ATOMIC_MAX"
            | "SIZE_MAX"
            | "PTRDIFF_MIN"
            | "PTRDIFF_MAX"
            | "WCHAR_MIN"
            | "WCHAR_MAX"
            | "WINT_MIN"
            | "WINT_MAX"
    )
}
