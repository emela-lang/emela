//! Converting file and directory names to type names (one module-name segment).
//!
//! Names must be lower snake_case. Conversion capitalizes each `_`-separated word and joins them.
//! The result must match the lexical class of type names (spec 2.3: starts with an uppercase
//! letter, contains at least one lowercase letter, alphanumeric only).

use std::fmt;

use smol_str::SmolStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameError {
    /// Not lower snake_case (uppercase letters, symbols such as `-` or `.`, or a leading,
    /// trailing or doubled `_`).
    NotSnakeCase,
    /// Snake case, but the converted name is not a valid type name (`2fa` → `2fa`, `x_y` → `XY`).
    NotTypeName { converted: SmolStr },
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NameError::NotSnakeCase => f.write_str("小文字の snake_case ではない"),
            NameError::NotTypeName { converted } => {
                write!(f, "変換後の `{converted}` が型名にならない")
            }
        }
    }
}

/// `json_codec` becomes `JsonCodec`, `http2` becomes `Http2`.
pub fn to_type_name(name: &str) -> Result<SmolStr, NameError> {
    if !is_snake_case(name) {
        return Err(NameError::NotSnakeCase);
    }
    let mut converted = String::with_capacity(name.len());
    for word in name.split('_') {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            converted.push(first.to_ascii_uppercase());
            converted.extend(chars);
        }
    }
    if is_type_name(&converted) {
        Ok(SmolStr::new(converted))
    } else {
        Err(NameError::NotTypeName {
            converted: SmolStr::new(converted),
        })
    }
}

/// Rejects empty words (a leading, trailing or doubled `_`).
fn is_snake_case(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        && name.split('_').all(|word| !word.is_empty())
}

/// The lexical class of type names (spec 2.3): starts with an uppercase letter, contains at least
/// one lowercase letter, alphanumeric only.
pub fn is_type_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.first().is_some_and(u8::is_ascii_uppercase)
        && bytes.iter().any(u8::is_ascii_lowercase)
        && bytes.iter().all(u8::is_ascii_alphanumeric)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(name: &str) -> String {
        to_type_name(name).unwrap().to_string()
    }

    #[test]
    fn converts_snake_case() {
        assert_eq!(ok("client"), "Client");
        assert_eq!(ok("json_codec"), "JsonCodec");
        assert_eq!(ok("http2"), "Http2");
        assert_eq!(ok("http_2"), "Http2");
        assert_eq!(ok("ab"), "Ab");
    }

    #[test]
    fn rejects_non_snake_case() {
        for name in [
            "", "_x", "x_", "a__b", "Foo", "fooBar", "foo-bar", "foo.bar", "föö", " ",
        ] {
            assert_eq!(to_type_name(name), Err(NameError::NotSnakeCase), "{name:?}");
        }
    }

    #[test]
    fn rejects_non_type_name() {
        let err = |name| match to_type_name(name) {
            Err(NameError::NotTypeName { converted }) => converted.to_string(),
            other => panic!("{name:?}: {other:?}"),
        };
        // Starts with a digit.
        assert_eq!(err("2fa"), "2fa");
        // No lowercase letter.
        assert_eq!(err("a"), "A");
        assert_eq!(err("x_y"), "XY");
        assert_eq!(err("a_1"), "A1");
    }

    #[test]
    fn type_name_class() {
        assert!(is_type_name("Http2"));
        assert!(!is_type_name("HTTP"));
        assert!(!is_type_name("Http_2"));
        assert!(!is_type_name("http"));
        assert!(!is_type_name(""));
    }
}
