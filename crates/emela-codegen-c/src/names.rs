//! C names and literals.

use std::fmt::Write as _;

/// A C identifier for the IR function `index` named `name`: `e12_show_List_Int_`. The index makes
/// it unique; the name is for reading.
pub(crate) fn function(index: usize, name: &str) -> String {
    let mut out = format!("e{index}_");
    for c in name.chars().take(48) {
        out.push(if c.is_ascii_alphanumeric() { c } else { '_' });
    }
    out
}

/// Whether `name` can be used as a C identifier (a host function name, spec 12.1).
pub(crate) fn is_c_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A C string literal holding exactly `bytes`. Bytes other than printable ASCII are written as
/// three-digit octal escapes, which cannot run into the following characters.
pub(crate) fn string_literal(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() + 2);
    out.push('"');
    for &b in bytes {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            // `?` could start a trigraph.
            b'?' => out.push_str("\\077"),
            0x20..=0x7e => out.push(b as char),
            _ => {
                let _ = write!(out, "\\{b:03o}");
            }
        }
    }
    out.push('"');
    out
}

pub(crate) fn int32(n: i32) -> String {
    if n == i32::MIN {
        "(-2147483647 - 1)".to_owned()
    } else {
        n.to_string()
    }
}

pub(crate) fn int64(n: i64) -> String {
    if n == i64::MIN {
        "(-INT64_C(9223372036854775807) - 1)".to_owned()
    } else {
        format!("INT64_C({n})")
    }
}

pub(crate) fn float(x: f64) -> String {
    if x.is_nan() {
        "(0.0 / 0.0)".to_owned()
    } else if x.is_infinite() {
        if x > 0.0 {
            "(1.0 / 0.0)"
        } else {
            "(-1.0 / 0.0)"
        }
        .to_owned()
    } else {
        // Rust's shortest round-trip form, which C reads back exactly.
        let s = format!("{x:?}");
        if s.contains(['.', 'e', 'E']) {
            s
        } else {
            format!("{s}.0")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals() {
        assert_eq!(
            string_literal("a\"b\\?\n\u{e9}".as_bytes()),
            r#""a\"b\\\077\012\303\251""#
        );
        assert_eq!(int32(i32::MIN), "(-2147483647 - 1)");
        assert_eq!(float(1.0), "1.0");
        assert_eq!(float(1e300), "1e300");
        assert_eq!(float(-0.0), "-0.0");
        assert_eq!(function(3, "show$List[Int]"), "e3_show_List_Int_");
        assert!(is_c_identifier("emela_print_line"));
        assert!(!is_c_identifier("console.log"));
    }
}
