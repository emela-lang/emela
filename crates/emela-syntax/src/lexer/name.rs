//! Classifying names and reserved words (spec 2.3, 3.1–3.3).

use crate::SyntaxKind;

/// Classifies a word cut as `[A-Za-z_][A-Za-z0-9_]*`.
///
/// Reserved words are looked up with `keyword`. The contextual words of spec 3.2 and the words of
/// spec 3.4 come back as ordinary names. Returns `None` if no class matches; the caller reports
/// the diagnostic.
pub(super) fn classify_name(word: &str) -> Option<SyntaxKind> {
    use SyntaxKind::*;
    if let Some(kind) = keyword(word) {
        return Some(kind);
    }

    if word == "_" {
        return Some(UNDERSCORE);
    }

    let first = word.bytes().next()?;
    let upper_name = word
        .bytes()
        .all(|b| matches!(b, b'A'..=b'Z' | b'0'..=b'9' | b'_'));
    let type_name = word.bytes().all(|b| b.is_ascii_alphanumeric());

    match first {
        b'a'..=b'z' | b'_' => Some(LOWER_NAME),
        b'A'..=b'Z' if upper_name => Some(UPPER_NAME),
        b'A'..=b'Z' if type_name => Some(TYPE_NAME),
        _ => None,
    }
}

/// Reserved words (spec 3.1) and words reserved for the future (spec 3.3).
fn keyword(word: &str) -> Option<SyntaxKind> {
    use SyntaxKind::*;
    let kind = match word {
        "fn" => FN_KW,
        "type" => TYPE_KW,
        "enum" => ENUM_KW,
        "error" => ERROR_KW,
        "const" => CONST_KW,
        "trait" => TRAIT_KW,
        "impl" => IMPL_KW,
        "effect" => EFFECT_KW,
        "handler" => HANDLER_KW,
        "layer" => LAYER_KW,
        "import" => IMPORT_KW,
        "pub" => PUB_KW,
        "opaque" => OPAQUE_KW,
        "suspend" => SUSPEND_KW,
        "if" => IF_KW,
        "else" => ELSE_KW,
        "match" => MATCH_KW,
        "fail" => FAIL_KW,
        "escape" => ESCAPE_KW,
        "use" => USE_KW,
        "with" => WITH_KW,
        "assert" => ASSERT_KW,
        "self" => SELF_KW,
        "Self" => SELF_TYPE_KW,
        "as" => AS_KW,
        "var" | "return" | "while" | "loop" | "let" | "mut" | "in" | "where" | "do" | "try"
        | "catch" | "throw" | "async" | "yield" | "defer" | "macro" | "mod" => RESERVED_KW,
        _ => return None,
    };
    Some(kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use SyntaxKind::*;

    fn check(cases: &[(&str, Option<SyntaxKind>)]) {
        for &(word, expected) in cases {
            assert_eq!(classify_name(word), expected, "{word}");
        }
    }

    #[test]
    fn 小文字名() {
        check(&[
            ("user_id", Some(LOWER_NAME)),
            ("find", Some(LOWER_NAME)),
            ("_tmp", Some(LOWER_NAME)),
            ("__", Some(LOWER_NAME)),
            ("x1", Some(LOWER_NAME)),
            // Accepted by the lexer per the spec 17.2 EBNF; snake_case is left to lint
            ("userId", Some(LOWER_NAME)),
        ]);
    }

    #[test]
    fn 束縛しないパターン() {
        check(&[("_", Some(UNDERSCORE))]);
    }

    #[test]
    fn 型名() {
        check(&[
            ("User", Some(TYPE_NAME)),
            ("Http", Some(TYPE_NAME)),
            ("Db", Some(TYPE_NAME)),
            ("A1b", Some(TYPE_NAME)),
            ("HTTPServer", Some(TYPE_NAME)),
            ("True", Some(TYPE_NAME)),
            ("None", Some(TYPE_NAME)),
        ]);
    }

    #[test]
    fn 大文字名() {
        check(&[
            ("MAX_SIZE", Some(UPPER_NAME)),
            ("A", Some(UPPER_NAME)),
            ("HTTP", Some(UPPER_NAME)),
            ("A1", Some(UPPER_NAME)),
            ("A_", Some(UPPER_NAME)),
        ]);
    }

    #[test]
    fn どのクラスにも合わない名前() {
        check(&[("Http_client", None), ("Ab_", None), ("Max_SIZE", None)]);
    }

    #[test]
    fn 予約語() {
        check(&[
            ("fn", Some(FN_KW)),
            ("as", Some(AS_KW)),
            ("self", Some(SELF_KW)),
            ("Self", Some(SELF_TYPE_KW)),
            ("return", Some(RESERVED_KW)),
            ("mod", Some(RESERVED_KW)),
        ]);
    }

    #[test]
    fn 文脈依存の語と_3_4_の語は名前のまま() {
        check(&[
            ("derive", Some(LOWER_NAME)),
            ("fails", Some(LOWER_NAME)),
            ("release", Some(LOWER_NAME)),
            ("panic", Some(LOWER_NAME)),
            ("resume", Some(LOWER_NAME)),
        ]);
    }
}
