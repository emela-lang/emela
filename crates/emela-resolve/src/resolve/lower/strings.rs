//! String literals and the string arguments of attributes.

use emela_syntax::SyntaxKind::*;
use emela_syntax::SyntaxToken;
use emela_syntax::ast::{self, HasAttrs};
use smol_str::SmolStr;

/// The value of `@key("...")` or `@tag("...")` (`[kw.annotation]`): its one argument when that is
/// a string literal without interpolation. `None` for any other argument, which is E3310.
pub fn string_attr_value(annotation: &ast::Annotation) -> Option<SmolStr> {
    let mut args = annotation.args();
    let arg = args.next()?;
    if args.next().is_some() {
        return None;
    }
    // A name (`@key(k)`) or `name: value` puts a token before the literal.
    if arg
        .children_with_tokens()
        .any(|e| matches!(e.kind(), LOWER_NAME | COLON))
    {
        return None;
    }
    let mut nodes = arg.children();
    let string = nodes.next().filter(|n| n.kind() == STRING)?;
    if nodes.next().is_some() || string.children().next().is_some() {
        // An interpolation is a child node of the string.
        return None;
    }
    let text: String = string
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == STRING_TEXT)
        .map(|t| string_text(&t))
        .collect();
    Some(text.into())
}

/// The argument of the first attribute `name` of `node`, if it is valid ([`string_attr_value`]).
/// A second attribute of the same name is E3310.
pub fn string_attr(node: &impl HasAttrs, name: &str) -> Option<SmolStr> {
    node.annotations()
        .find(|a| a.name().is_some_and(|n| n.text() == name))
        .and_then(|a| string_attr_value(&a))
}

/// The value of a STRING_TEXT token. In a multiline string (spec 2.4), the newline after the
/// opening `"""` and the one before the closing `"""` are dropped, and the closing line's
/// indentation is removed from the start of each line (whitespace-only lines may be shallower;
/// the lexer reports other shallower lines, E1009). This works on the text as written, before
/// unescaping, so that a `\n` escape does not start a line.
pub(super) fn string_text(token: &SyntaxToken) -> String {
    let Some(string) = token
        .parent()
        .filter(|s| s.first_token().is_some_and(|t| t.kind() == TRIPLE_QUOTE))
    else {
        return unescape(token.text());
    };
    let first = token
        .prev_sibling_or_token()
        .is_some_and(|e| e.kind() == TRIPLE_QUOTE);
    let last = token
        .next_sibling_or_token()
        .is_some_and(|e| e.kind() == TRIPLE_QUOTE);
    // The closing `"""` holds the whitespace before it on its line (`    """`).
    let close = string.last_child_or_token().and_then(|e| e.into_token());
    let indent = close
        .as_ref()
        .filter(|t| t.kind() == TRIPLE_QUOTE)
        .and_then(|t| t.text().strip_suffix("\"\"\""))
        .unwrap_or("");
    let mut text = token.text();
    if first {
        text = text
            .strip_prefix("\r\n")
            .or_else(|| text.strip_prefix('\n'))
            .unwrap_or(text);
    }
    if last {
        text = text
            .strip_suffix("\r\n")
            .or_else(|| text.strip_suffix('\n'))
            .unwrap_or(text);
    }
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        // A token after an interpolation starts in the middle of a line.
        let line = if i == 0 && !first {
            line
        } else if let Some(rest) = line.strip_prefix(indent) {
            rest
        } else if line.trim().is_empty() {
            line.trim_start_matches([' ', '\t'])
        } else {
            line
        };
        out.push_str(line);
    }
    unescape(&out)
}

/// Unescapes `\"` `\\` `\n` `\t` `\r` `\#` `\u{...}`. Invalid escapes are already reported by
/// the lexer, so they are kept as is.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('u') => {
                let rest = chars.as_str();
                let value = rest
                    .strip_prefix('{')
                    .and_then(|r| r.split_once('}'))
                    .and_then(|(digits, _)| u32::from_str_radix(digits, 16).ok())
                    .and_then(char::from_u32);
                match value {
                    Some(v) => {
                        out.push(v);
                        let skip = rest.find('}').unwrap_or(0) + 1;
                        chars = rest[skip..].chars();
                    }
                    None => out.push_str("\\u"),
                }
            }
            Some(c @ ('"' | '\\' | '#')) => out.push(c),
            Some(c) => {
                out.push('\\');
                out.push(c);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The values of the STRING_TEXT tokens of `src`, in order.
    fn texts(src: &str) -> Vec<String> {
        emela_syntax::parse(src)
            .syntax()
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| t.kind() == STRING_TEXT)
            .map(|t| string_text(&t))
            .collect()
    }

    #[test]
    fn 複数行文字列は閉じる行のインデントと前後の改行を除く() {
        // The example of `[lex.literal]`, with the interpolation as the separate part it is.
        let src = "fn f() {\n  query = \"\"\"\n    SELECT *\n    FROM users\n    WHERE id = #{id}\n    \"\"\"\n}";
        assert_eq!(texts(src), ["SELECT *\nFROM users\nWHERE id = ", ""]);
    }

    #[test]
    fn 補間の後の部品は行の途中から始まる() {
        let src = "fn f() {\n  s = \"\"\"\n    a #{x}  b\n      c\n    \"\"\"\n}";
        assert_eq!(texts(src), ["a ", "  b\n  c"]);
    }

    #[test]
    fn 空白だけの行は閉じる行より浅くてよい() {
        let src = "fn f() {\n  s = \"\"\"\n    a\n  \n\t\n    b\n    \"\"\"\n}";
        assert_eq!(texts(src), ["a\n\n\nb"]);
    }

    #[test]
    fn 複数行文字列の改行は_crlf_でもよい() {
        let src = "fn f() {\r\n  s = \"\"\"\r\n    a\r\n    \"\"\"\r\n}";
        assert_eq!(texts(src), ["a"]);
    }

    #[test]
    fn 複数行文字列のエスケープは字下げを除いた後に解く() {
        // `\n` does not start a line, so the indentation after it is kept.
        let src = "fn f() {\n  s = \"\"\"\n    a\\n    b\n    \"\"\"\n}";
        assert_eq!(texts(src), ["a\n    b"]);
    }

    #[test]
    fn 七つのエスケープ() {
        assert_eq!(
            unescape(r#"\" \\ \n \t \r \# \u{41}"#),
            "\" \\ \n \t \r # A"
        );
        assert_eq!(unescape(r"\u{1F600}!"), "😀!");
        assert_eq!(unescape(r"a\u{62}c\u{64}"), "abcd");
    }

    /// Invalid escapes are reported by the lexer and kept as written.
    #[test]
    fn 不正なエスケープは書いたまま残す() {
        assert_eq!(unescape(r"\q"), r"\q");
        assert_eq!(unescape(r"\u{D800}x"), r"\u{D800}x");
        assert_eq!(unescape(r"\u41"), r"\u41");
        assert_eq!(unescape("a\\"), "a\\");
    }

    #[test]
    fn ふつうの文字列はエスケープだけを解く() {
        assert_eq!(texts("fn f() { \"  a\\tb  \" }"), ["  a\tb  "]);
    }
}
