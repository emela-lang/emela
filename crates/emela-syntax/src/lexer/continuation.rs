//! 行の継続（2.5）．NEWLINE のうち，前の行が続くものを NEWLINE_CONT に付け替える．

use super::Token;
use crate::SyntaxKind;

pub(super) fn mark_continuations(tokens: &mut [Token]) {
    use SyntaxKind::*;

    let mut stack = Vec::new();

    for i in 0..tokens.len() {
        let kind = tokens[i].kind;
        match kind {
            L_PAREN | L_BRACK | L_BRACE | INTERP_START => stack.push(kind),
            R_PAREN | R_BRACK | R_BRACE | INTERP_END => {
                stack.pop();
            }
            NEWLINE if matches!(stack.last(), Some(L_PAREN | L_BRACK | INTERP_START)) => {
                tokens[i].kind = NEWLINE_CONT;
            }
            NEWLINE => {
                let continues = tokens[i + 1..]
                    .iter()
                    .map(|t| t.kind)
                    .find(|&k| !k.is_trivia() && k != NEWLINE)
                    .is_some_and(starts_continuation);
                if continues {
                    tokens[i].kind = NEWLINE_CONT;
                }
            }
            _ => {}
        }
    }
}

fn starts_continuation(kind: SyntaxKind) -> bool {
    use SyntaxKind::*;
    matches!(
        kind,
        STAR | SLASH
            | PERCENT
            | PLUS
            | EQ2
            | NEQ
            | LT
            | LTEQ
            | GT
            | GTEQ
            | AMP2
            | PIPE2
            | PIPE_GT
            | DOT
            | ESCAPE_KW
            | ELSE_KW
            | THIN_ARROW
            | PIPE
    )
}

#[cfg(test)]
mod tests {
    use crate::SyntaxKind::{self, *};
    use crate::lex;

    /// 改行の種類だけを順に取り出す．
    fn newlines(src: &str) -> Vec<SyntaxKind> {
        lex(src)
            .tokens
            .iter()
            .map(|t| t.kind)
            .filter(|k| matches!(k, NEWLINE | NEWLINE_CONT))
            .collect()
    }

    #[test]
    fn 二項演算子とドットと_escape_で始まる行は続き() {
        assert_eq!(newlines("a\n|> f"), [NEWLINE_CONT]);
        assert_eq!(newlines("a\n  + b\n  * c"), [NEWLINE_CONT, NEWLINE_CONT]);
        assert_eq!(newlines("a\n&& b\n|| c\n== d"), [NEWLINE_CONT; 3]);
        assert_eq!(newlines("users\n  .find(id)"), [NEWLINE_CONT]);
        assert_eq!(newlines("load(id)\nescape {\n}"), [NEWLINE_CONT, NEWLINE]);
    }

    #[test]
    fn マイナスと括弧で始まる行は新しい文() {
        assert_eq!(newlines("a\n-b"), [NEWLINE]);
        assert_eq!(newlines("foo\n(bar)"), [NEWLINE]);
        assert_eq!(newlines("foo\n[bar]"), [NEWLINE]);
        assert_eq!(newlines("a\nb"), [NEWLINE]);
    }

    #[test]
    fn 丸括弧と角括弧の中は続き() {
        assert_eq!(newlines("f(\n  a,\n  b,\n)"), [NEWLINE_CONT; 3]);
        assert_eq!(newlines("[\n1\n]"), [NEWLINE_CONT; 2]);
        assert_eq!(newlines("\"#{\nx\n}\""), [NEWLINE_CONT; 2]);
    }

    #[test]
    fn 波括弧の中は区切り() {
        // 頂上が `{` なら，括弧の中でも改行は文を区切る
        assert_eq!(newlines("f(fn(x) {\na\nb\n})"), [NEWLINE; 3]);
        assert_eq!(newlines("{\na\n|> f\n}"), [NEWLINE, NEWLINE_CONT, NEWLINE]);
    }

    #[test]
    fn 空白とコメントと空行は飛ばして次のトークンを見る() {
        assert_eq!(newlines("a  # c\n  |> f"), [NEWLINE_CONT]);
        assert_eq!(newlines("a\n  # c\n  |> f"), [NEWLINE_CONT; 2]);
        assert_eq!(newlines("a\n\n|> f"), [NEWLINE_CONT; 2]);
    }

    #[test]
    fn 閉じ波括弧の後の_else_は続き() {
        assert_eq!(
            newlines("if ok {\na\n}\nelse {\nb\n}"),
            [NEWLINE, NEWLINE, NEWLINE_CONT, NEWLINE, NEWLINE]
        );
    }

    #[test]
    fn 行頭の矢印は続き() {
        assert_eq!(newlines("fn load(id: Int)\n  -> User"), [NEWLINE_CONT]);
        assert_eq!(
            newlines("match x {\n  A\n    -> 1\n}"),
            [NEWLINE, NEWLINE_CONT, NEWLINE]
        );
    }

    #[test]
    fn 行頭の縦棒は続き() {
        assert_eq!(
            newlines("fn load(id: Int) -> User fails NotFound\n  | DbError"),
            [NEWLINE_CONT]
        );
    }

    #[test]
    fn 入力の終わりの改行は区切り() {
        assert_eq!(newlines("a\n"), [NEWLINE]);
        assert_eq!(newlines("a\n  # c\n"), [NEWLINE; 2]);
    }

    #[test]
    fn 対応の合わない閉じ括弧でも止まらない() {
        assert_eq!(newlines(")\n]\n}\na"), [NEWLINE; 3]);
    }
}
