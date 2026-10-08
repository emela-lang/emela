//! 文字列の本文の読み取り（2.4）．

use super::range;
use crate::diagnostic::{Diagnostic, DiagnosticCode};

/// 文字列の中を `start` から読み進め，本文の終わりの位置と，エスケープの診断を返す．
///
/// 次のどれかの手前で止まる．止まった位置の文字は読まない．
///
/// - 閉じる `"`（`\"` は止まらない）
/// - 補間の `#{`（`\#{` は止まらない．`#` の後ろが `{` でなければただの文字）
/// - 改行 `\n` または `\r`
/// - 入力の終わり
///
/// 通すエスケープは `\" \\ \n \#` だけ．それ以外は `\` とその次の1文字の範囲に診断を出し，
/// 読むのはやめずに続ける．`\` が入力の終わりや改行の直前にあるときは `\` だけの範囲に診断を出す．
///
/// `src[start..]` の先頭が上の止まる文字でないときだけ呼ばれる．`start` は文字の境界にある．
pub(super) fn scan_string_text(src: &str, start: usize) -> (usize, Vec<Diagnostic>) {
    let mut diagnostics = Vec::new();
    let mut chars = src[start..].char_indices();

    while let Some((offset, c)) = chars.next() {
        let i = start + offset;
        match c {
            '"' | '\n' | '\r' => return (i, diagnostics),
            '#' if src[i..].starts_with("#{") => return (i, diagnostics),
            '\\' => match chars.next() {
                Some((_, '"' | '\\' | 'n' | '#')) => {}
                None | Some((_, '\n' | '\r')) => {
                    diagnostics.push(error(
                        i,
                        i + 1,
                        DiagnosticCode::MissingEscapedCharacter,
                        "missing character after `\\`",
                    ));
                    return (i + 1, diagnostics);
                }
                Some((next, other)) => {
                    let end = start + next + other.len_utf8();
                    diagnostics.push(error(
                        i,
                        end,
                        DiagnosticCode::UnknownEscape,
                        format!("unknown escape sequence `\\{}`", other.escape_debug()),
                    ));
                }
            },
            _ => {}
        }
    }

    (src.len(), diagnostics)
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

    /// 本文の終わりと，診断の範囲だけを取り出す．
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
    }

    #[test]
    fn 通さないエスケープ() {
        assert_eq!(scan(r#"a\tb""#, 0), (4, vec![(1, 3)]));
        assert_eq!(scan(r"\q\w", 0), (4, vec![(0, 2), (2, 4)]));
        // 次の文字が複数バイトでも，1文字ぶんの範囲にする
        assert_eq!(scan("\\é\"", 0), (3, vec![(0, 3)]));
    }

    #[test]
    fn 行末と入力の終わりの逆スラッシュ() {
        assert_eq!(scan("a\\", 0), (2, vec![(1, 2)]));
        assert_eq!(scan("a\\\nb", 0), (2, vec![(1, 2)]));
    }
}
