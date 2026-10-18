//! Line continuation (spec 2.5). Retags as NEWLINE_CONT each NEWLINE that continues the previous
//! line.

use super::Token;
use crate::SyntaxKind;

pub(super) fn mark_continuations(src: &str, tokens: &mut [Token]) {
    use SyntaxKind::*;

    let mut stack = Vec::new();
    // The bracket depth of the signature being read (rule 4): from `fn name` of a declaration or
    // an effect operation to the body's `{`, or to the newline that ends a declaration without a
    // body.
    let mut signature: Option<usize> = None;

    for i in 0..tokens.len() {
        let kind = tokens[i].kind;
        match kind {
            // `fn name` starts a signature; `fn(` is a lambda or a function type.
            FN_KW if next_significant(tokens, i).is_some_and(|t| t.kind == LOWER_NAME) => {
                signature = Some(stack.len());
            }
            L_BRACE if signature == Some(stack.len()) => {
                signature = None;
                stack.push(kind);
            }
            L_PAREN | L_BRACK | L_BRACE | INTERP_START => stack.push(kind),
            R_PAREN | R_BRACK | R_BRACE | INTERP_END => {
                stack.pop();
            }
            NEWLINE if matches!(stack.last(), Some(L_PAREN | L_BRACK | INTERP_START)) => {
                tokens[i].kind = NEWLINE_CONT;
            }
            NEWLINE => {
                let next = next_significant(tokens, i).copied();
                let continues = next.is_some_and(|t| starts_continuation(t.kind))
                    || postfix_match_follows(tokens, i);
                let in_signature = signature == Some(stack.len());
                if continues || (in_signature && next.is_some_and(|t| continues_signature(src, t)))
                {
                    tokens[i].kind = NEWLINE_CONT;
                } else if in_signature {
                    signature = None;
                }
            }
            _ => {}
        }
    }
}

/// The first token after `tokens[i]` that is not trivia or a newline.
fn next_significant(tokens: &[Token], i: usize) -> Option<&Token> {
    tokens[i + 1..]
        .iter()
        .find(|t| !t.kind.is_trivia() && !matches!(t.kind, SyntaxKind::NEWLINE))
}

/// Rule 1 of spec 2.5 for `match`: a line starting with `match {` continues the previous one.
/// When something other than `{` follows, the line is a prefix match (E0142); it starts a new
/// statement so that the parser can report it as such.
fn postfix_match_follows(tokens: &[Token], i: usize) -> bool {
    let mut rest = tokens[i + 1..]
        .iter()
        .filter(|t| !t.kind.is_trivia() && !matches!(t.kind, SyntaxKind::NEWLINE));
    rest.next().is_some_and(|t| t.kind == SyntaxKind::MATCH_KW)
        && rest.next().is_some_and(|t| t.kind == SyntaxKind::L_BRACE)
}

/// Rule 4 of spec 2.5: inside a signature, a line starting with `fails` or `use` continues it.
/// `fails` is a contextual word, lexed as a lower name.
fn continues_signature(src: &str, token: Token) -> bool {
    match token.kind {
        SyntaxKind::USE_KW => true,
        SyntaxKind::LOWER_NAME => &src[token.range] == "fails",
        _ => false,
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

    /// Only the kinds of the newlines, in order.
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
    fn 後置の_match_で始まる行は続き() {
        assert_eq!(
            newlines("xs\n  |> f\n  match {\n}"),
            [NEWLINE_CONT, NEWLINE_CONT, NEWLINE]
        );
        assert_eq!(
            newlines("a\n# c\nmatch {\n}"),
            [NEWLINE_CONT, NEWLINE_CONT, NEWLINE]
        );
        // Prefix `match` (E0142) starts a new statement so the parser can report it.
        assert_eq!(newlines("a\nmatch x {\n}"), [NEWLINE, NEWLINE]);
    }

    #[test]
    fn マイナスと括弧で始まる行は新しい文() {
        assert_eq!(newlines("a\n-b"), [NEWLINE]);
        // An operator at the end of a line does not continue the expression.
        assert_eq!(newlines("a = 10 -\n  1"), [NEWLINE]);
        assert_eq!(newlines("a = 10 +\n  1"), [NEWLINE]);
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
        // If the innermost bracket is `{`, newlines separate statements even inside parentheses
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
            newlines("x match {\n  A\n    -> 1\n}"),
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
    fn シグネチャの中の_fails_と_use_で始まる行は続き() {
        // The example of spec 2.5.
        let src = "pub fn get(url: String) -> Response\n  fails InvalidUrl | ConnectionFailed\n  use Http {\n  x\n}";
        assert_eq!(
            newlines(src),
            [NEWLINE_CONT, NEWLINE_CONT, NEWLINE, NEWLINE]
        );
        assert_eq!(
            newlines("fn f() -> Int\n  use Log {\n  1\n}"),
            [NEWLINE_CONT, NEWLINE, NEWLINE]
        );
        // Effect operations and functions without a body.
        assert_eq!(
            newlines("effect E {\n  fn get() -> Int\n    fails Boom\n  fn put() -> ()\n}"),
            [NEWLINE, NEWLINE_CONT, NEWLINE, NEWLINE]
        );
        assert_eq!(
            newlines("@external(js, \"f\")\nfn f() -> Int\n  use {}\nfn g() { 1 }"),
            [NEWLINE, NEWLINE_CONT, NEWLINE]
        );
    }

    #[test]
    fn 本体の中の_fails_と_use_で始まる行は新しい文() {
        assert_eq!(
            newlines("fn f() {\n  a\n  fails = 1\n  use Log\n}"),
            [NEWLINE, NEWLINE, NEWLINE, NEWLINE]
        );
        // After the signature ends at the body, and in lambdas.
        assert_eq!(
            newlines("fn f() -> Int {\n  g(fn() {\n    a\n    use Log\n  })\n}"),
            [NEWLINE, NEWLINE, NEWLINE, NEWLINE, NEWLINE]
        );
        // A declaration without a body ends at its newline.
        assert_eq!(
            newlines("effect E {\n  fn get() -> Int\n}\nfails\nuse X"),
            [NEWLINE, NEWLINE, NEWLINE, NEWLINE]
        );
        // `fn(` is a function type, not a signature.
        assert_eq!(newlines("x = fn(a)\nuse Log"), [NEWLINE]);
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
