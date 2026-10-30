//! C11 spelling of byte strings and numeric constants.

pub(in crate::compiler::emit) fn c_string_literal(bytes: &[u8]) -> String {
    let escaped = bytes
        .iter()
        .map(|byte| format!("\\x{byte:02x}"))
        .collect::<String>();
    format!("\"{escaped}\"")
}

pub(in crate::compiler::emit) fn int32_literal(value: i32) -> String {
    if value == i32::MIN {
        "(-INT32_C(2147483647) - INT32_C(1))".to_owned()
    } else if value < 0 {
        format!("(-INT32_C({}))", -value)
    } else {
        format!("INT32_C({value})")
    }
}

pub(in crate::compiler::emit) fn int64_literal(value: i64) -> String {
    if value == i64::MIN {
        "(-INT64_C(9223372036854775807) - INT64_C(1))".to_owned()
    } else {
        format!("INT64_C({value})")
    }
}

pub(in crate::compiler::emit) fn float_literal(value: f64) -> String {
    if value.is_nan() {
        "NAN".to_owned()
    } else if value == f64::INFINITY {
        "INFINITY".to_owned()
    } else if value == f64::NEG_INFINITY {
        "(-INFINITY)".to_owned()
    } else {
        let literal = format!("{value:?}");
        if literal.contains(['.', 'e', 'E']) {
            literal
        } else {
            format!("{literal}.0")
        }
    }
}
