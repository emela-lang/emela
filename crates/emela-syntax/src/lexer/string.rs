//! Scanning string bodies (spec 2.4).

use super::range;
use crate::diagnostic::{Diagnostic, DiagnosticCode};

/// Scans a string from `start` and returns the end of the body and the escape diagnostics.
///
/// Stops before any of the following, without consuming it:
///
/// - the closing `"` (`\"` does not stop)
/// - an interpolation `#{` (`\#{` does not stop; `#` not followed by `{` is an ordinary character)
/// - a newline `\n` or `\r`
/// - the end of the input
///
/// Accepts the escapes `\" \\ \n \t \r \#` and `\u{...}` (1 to 6 hex digits). Any other escape
/// gets a diagnostic over `\` and the next character, and scanning continues. A `\` right before
/// the end of the input or a newline gets a diagnostic over the `\` alone.
///
/// Called only when `src[start..]` does not begin with one of the stop characters. `start` is on
/// a character boundary.
pub(super) fn scan_string_text(src: &str, start: usize) -> (usize, Vec<Diagnostic>) {
    let mut diagnostics = Vec::new();
    let mut i = start;
    while let Some(c) = src[i..].chars().next() {
        match c {
            '"' | '\n' | '\r' => return (i, diagnostics),
            '#' if src[i..].starts_with("#{") => return (i, diagnostics),
            '\\' => match escape(src, i, &mut diagnostics) {
                Some(next) => i = next,
                None => return (i + 1, diagnostics),
            },
            _ => i += c.len_utf8(),
        }
    }
    (src.len(), diagnostics)
}

/// Scans a multiline string body from `start` and returns the end of the body, the escape
/// diagnostics, and the start of each line that begins inside the body.
///
/// Stops before `"""` and an interpolation `#{`. On reaching a line holding only the closing
/// `"""`, stops at the start of that line (its leading whitespace belongs to the closing token).
/// Newlines and one or two `"` are part of the body.
pub(super) fn scan_multiline_text(src: &str, start: usize) -> (usize, Vec<Diagnostic>, Vec<usize>) {
    let mut diagnostics = Vec::new();
    let mut lines = Vec::new();
    let mut i = start;
    while let Some(c) = src[i..].chars().next() {
        match c {
            '"' if src[i..].starts_with("\"\"\"") => break,
            '#' if src[i..].starts_with("#{") => break,
            '\n' => {
                i += 1;
                lines.push(i);
                if closing_indent(src, i).is_some() {
                    break;
                }
            }
            // In a multiline string, a `\` before a newline does not end the body.
            '\\' => i = escape(src, i, &mut diagnostics).unwrap_or(i + 1),
            _ => i += c.len_utf8(),
        }
    }
    (i, diagnostics, lines)
}

/// If `src[i..]` is a line of only whitespace and `"""`, returns the width of the whitespace.
pub(super) fn closing_indent(src: &str, i: usize) -> Option<usize> {
    let rest = &src[i..];
    let indent = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    rest[indent..].starts_with("\"\"\"").then_some(indent)
}

/// Reads one escape starting at the `\` at `src[i]` and returns the next position. If nothing
/// follows `\`, reports a diagnostic and returns `None` (the caller keeps the `\` in the body and
/// stops).
fn escape(src: &str, i: usize, diagnostics: &mut Vec<Diagnostic>) -> Option<usize> {
    match src[i + 1..].chars().next() {
        None | Some('\n' | '\r') => {
            diagnostics.push(error(
                i,
                i + 1,
                DiagnosticCode::MissingEscapedCharacter,
                "missing character after `\\`",
            ));
            None
        }
        Some('"' | '\\' | 'n' | 't' | 'r' | '#') => Some(i + 2),
        Some('u') => Some(unicode_escape(src, i, diagnostics)),
        Some(other) => {
            let end = i + 1 + other.len_utf8();
            diagnostics.push(error(
                i,
                end,
                DiagnosticCode::UnknownEscape,
                format!("unknown escape sequence `\\{}`", other.escape_debug()),
            ));
            Some(end)
        }
    }
}

/// Reports E0108 if the escape is malformed or its value is a surrogate or above 10FFFF.
fn unicode_escape(src: &str, i: usize, diagnostics: &mut Vec<Diagnostic>) -> usize {
    let after_u = i + 2;
    let mut report = |end: usize, message: String| {
        diagnostics.push(error(i, end, DiagnosticCode::InvalidUnicodeEscape, message));
        end
    };
    if !src[after_u..].starts_with('{') {
        return report(after_u, "expected `{` after `\\u`".to_owned());
    }
    let digits_start = after_u + 1;
    let digits_len = src[digits_start..]
        .bytes()
        .take_while(u8::is_ascii_hexdigit)
        .count();
    let digits_end = digits_start + digits_len;
    if !src[digits_end..].starts_with('}') {
        return report(
            digits_end,
            "unterminated unicode escape: expected `}`".to_owned(),
        );
    }
    let end = digits_end + 1;
    let digits = &src[digits_start..digits_end];
    if !(1..=6).contains(&digits_len) {
        return report(end, "a unicode escape needs 1 to 6 hex digits".to_owned());
    }
    let value = u32::from_str_radix(digits, 16).expect("at most 6 hex digits");
    if (0xD800..=0xDFFF).contains(&value) {
        return report(
            end,
            format!("`\\u{{{digits}}}` is a surrogate, not a Unicode scalar value"),
        );
    }
    if value > 0x10FFFF {
        return report(
            end,
            format!("`\\u{{{digits}}}` is out of range: the maximum is `\\u{{10FFFF}}`"),
        );
    }
    end
}

fn error(start: usize, end: usize, code: DiagnosticCode, message: impl Into<String>) -> Diagnostic {
    Diagnostic {
        range: range(start, end),
        code,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only the end of the body and the diagnostic ranges.
    fn scan(src: &str, start: usize) -> (usize, Vec<(u32, u32)>) {
        let (end, diagnostics) = scan_string_text(src, start);
        let ranges = diagnostics
            .iter()
            .map(|d| (d.range.start().into(), d.range.end().into()))
            .collect();
        (end, ranges)
    }

    #[test]
    fn 止まる位置() {
        assert_eq!(scan("abc\"", 0), (3, vec![]));
        assert_eq!(scan("ab#{x}", 0), (2, vec![]));
        assert_eq!(scan("ab\ncd", 0), (2, vec![]));
        assert_eq!(scan("ab\r\n", 0), (2, vec![]));
        assert_eq!(scan("abc", 0), (3, vec![]));
        assert_eq!(scan("\"abc\"", 1), (4, vec![]));
        assert_eq!(scan("日本\"", 0), (6, vec![]));
    }

    #[test]
    fn 止まらない文字() {
        assert_eq!(scan("a#b\"", 0), (3, vec![]));
        assert_eq!(scan("a#\"", 0), (2, vec![]));
    }

    #[test]
    fn 通すエスケープ() {
        assert_eq!(scan(r#"a\"b""#, 0), (4, vec![]));
        assert_eq!(scan(r#"a\\""#, 0), (3, vec![]));
        assert_eq!(scan(r"a\nb", 0), (4, vec![]));
        assert_eq!(scan(r"\#{x}", 0), (5, vec![]));
        assert_eq!(scan(r"a\tb\rc", 0), (7, vec![]));
    }

    #[test]
    fn ユニコードのエスケープ() {
        assert_eq!(scan(r"\u{1F600}x", 0), (10, vec![]));
        assert_eq!(scan(r"\u{a}\u{10FFFF}\u{D7FF}\u{E000}", 0), (31, vec![]));
        // Malformed: no `{`, no `}`, no digits, 7 digits
        assert_eq!(scan(r"\u41", 0), (4, vec![(0, 2)]));
        assert_eq!(scan(r#"\u{41""#, 0), (5, vec![(0, 5)]));
        assert_eq!(scan(r"\u{}x", 0), (5, vec![(0, 4)]));
        assert_eq!(scan(r"\u{1234567}", 0), (11, vec![(0, 11)]));
        // Values: surrogates and above 10FFFF
        assert_eq!(scan(r"\u{D800}\u{dfff}", 0), (16, vec![(0, 8), (8, 16)]));
        assert_eq!(scan(r"\u{110000}", 0), (10, vec![(0, 10)]));
    }

    #[test]
    fn ユニコードのエスケープの診断はe0108() {
        let (_, diagnostics) = scan_string_text(r"\u{D800}\q", 0);
        let codes: Vec<_> = diagnostics.iter().map(|d| d.code).collect();
        assert_eq!(
            codes,
            [
                DiagnosticCode::InvalidUnicodeEscape,
                DiagnosticCode::UnknownEscape
            ]
        );
    }

    #[test]
    fn 通さないエスケープ() {
        assert_eq!(scan(r#"a\vb""#, 0), (4, vec![(1, 3)]));
        assert_eq!(scan(r"\q\w", 0), (4, vec![(0, 2), (2, 4)]));
        // A multibyte next character still yields a one-character range
        assert_eq!(scan("\\é\"", 0), (3, vec![(0, 3)]));
    }

    #[test]
    fn 行末と入力の終わりの逆スラッシュ() {
        assert_eq!(scan("a\\", 0), (2, vec![(1, 2)]));
        assert_eq!(scan("a\\\nb", 0), (2, vec![(1, 2)]));
    }
}
