//! Line continuation (spec 2.5). Retags as NEWLINE_CONT each NEWLINE that continues the previous
//! line.

use super::Token;
use crate::SyntaxKind;

pub(super) fn mark_continuations(src: &str, tokens: &mut [Token]) {
    use SyntaxKind::*;

    let mut stack = Vec::new();
    // Rule 5: where the current line stands in the error set of a fails clause.
    let mut error_set = ErrorSet::Outside;

    for i in 0..tokens.len() {
        let kind = tokens[i].kind;
        if !kind.is_trivia() && kind != NEWLINE {
            error_set = error_set.next(src, tokens[i]);
        }
        match kind {
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
                    || postfix_match_follows(tokens, i)
                    || starts_clause(src, tokens, i)
                    || (error_set == ErrorSet::AfterItem && next.is_some_and(|t| t.kind == PIPE));
                if continues {
                    tokens[i].kind = NEWLINE_CONT;
                } else {
                    error_set = ErrorSet::Outside;
                }
            }
            _ => {}
        }
    }
}

/// Rule 5 of spec 2.5 follows the tokens of the error set of a fails clause: `fails` followed by
/// an error item (a type name, possibly qualified, or an uppercase name), then `|` and error
/// items. A line that ends right after an item continues when the next line starts with `|`.
///
/// `fails` followed by a name is always a clause (as an identifier, `fails` is never followed by
/// a name), so the tokens decide it without knowing whether they are in a signature.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ErrorSet {
    Outside,
    /// Right after `fails`, or after `|` inside the set.
    BeforeItem,
    /// Right after an error item (`NotFound`, `Http.Error`, `E`).
    AfterItem,
    /// After the `.` of a qualified item.
    AfterDot,
}

impl ErrorSet {
    fn next(self, src: &str, token: Token) -> ErrorSet {
        use SyntaxKind::*;
        match (self, token.kind) {
            (_, LOWER_NAME) if &src[token.range] == "fails" => ErrorSet::BeforeItem,
            (ErrorSet::BeforeItem | ErrorSet::AfterDot, TYPE_NAME) => ErrorSet::AfterItem,
            (ErrorSet::BeforeItem, UPPER_NAME) => ErrorSet::AfterItem,
            (ErrorSet::AfterItem, DOT) => ErrorSet::AfterDot,
            (ErrorSet::AfterItem, PIPE) => ErrorSet::BeforeItem,
            _ => ErrorSet::Outside,
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

/// Rule 4 of spec 2.5: a line starting with `fails` or `uses` followed, on that line, by a type
/// name, an uppercase name or `{` continues the previous one. Both are contextual words, lexed as
/// lower names. As identifiers they are never followed by a name or `{`, so this holds anywhere.
fn starts_clause(src: &str, tokens: &[Token], i: usize) -> bool {
    use SyntaxKind::*;
    let mut rest = tokens[i + 1..]
        .iter()
        .filter(|t| !t.kind.is_trivia() && t.kind != NEWLINE);
    let Some(first) = rest.next() else {
        return false;
    };
    if first.kind != LOWER_NAME || !matches!(&src[first.range], "fails" | "uses") {
        return false;
    }
    // The next token on the same line: a NEWLINE in between ends it.
    let after = tokens[i + 1..]
        .iter()
        .skip_while(|t| t.range != first.range)
        .skip(1)
        .find(|t| !t.kind.is_trivia());
    after.is_some_and(|t| matches!(t.kind, TYPE_NAME | UPPER_NAME | L_BRACE))
}

/// Rules 1 and 3 of `[lex.continuation]`. `-` and `||` are binary operators that can also start
/// an operand (a unary minus, an anonymous function without parameters), so a line starting with
/// them starts a new statement.
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
            | PIPE_GT
            | DOT
            | ESCAPE_KW
            | ELSE_KW
            | THIN_ARROW
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
        assert_eq!(newlines("a\n&& b\n== d"), [NEWLINE_CONT; 2]);
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
    fn 縦棒と二本の縦棒で始まる行は新しい文() {
        // `||` at the start of a line starts an anonymous function, not the logical or.
        assert_eq!(newlines("x = a\n|| b"), [NEWLINE]);
        assert_eq!(newlines("{\n  m\n  || m\n}"), [NEWLINE; 3]);
        // `|` outside the error set of a fails clause starts an anonymous function.
        assert_eq!(newlines("{\n  a\n  |x| x\n}"), [NEWLINE; 3]);
        // Inside parentheses, a line break continues by rule 2.
        assert_eq!(newlines("(\n  a\n  || b\n)"), [NEWLINE_CONT; 3]);
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
        assert_eq!(newlines("f(|x| {\na\nb\n})"), [NEWLINE; 3]);
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
    fn fails_節の_error_set_で終わる行の次の縦棒は続き() {
        // Rule 5 of spec 2.5.
        assert_eq!(
            newlines("fn load(id: Int) -> User fails NotFound\n  | DbError"),
            [NEWLINE_CONT]
        );
        assert_eq!(
            newlines("fn f() -> A fails B | Http.Error\n  | C\n  | D {\n}"),
            [NEWLINE_CONT, NEWLINE_CONT, NEWLINE]
        );
        assert_eq!(
            newlines("effect E {\n  fn get() -> Int\n    fails A\n    | B\n}"),
            [NEWLINE, NEWLINE_CONT, NEWLINE_CONT, NEWLINE]
        );
        // The `fails` is on an earlier line that rule 4 continued.
        assert_eq!(
            newlines("fn f() -> A\n  fails B\n  | C"),
            [NEWLINE_CONT, NEWLINE_CONT]
        );
        // A set type parameter, comment lines in between, and a binding's type annotation.
        assert_eq!(
            newlines("fn f() -> A fails E # c\n  # d\n  | B"),
            [NEWLINE_CONT; 2]
        );
        assert_eq!(
            newlines("fn f() {\n  g: fn() -> A fails B\n    | C = h\n}"),
            [NEWLINE, NEWLINE_CONT, NEWLINE]
        );
    }

    #[test]
    fn それ以外の縦棒は新しい文() {
        // Not inside the error set of a fails clause: a `|` starts an anonymous function.
        assert_eq!(newlines("a\n| b"), [NEWLINE]);
        assert_eq!(
            newlines("effect E {\n  fn get() -> Int\n}\n|x| x"),
            [NEWLINE, NEWLINE, NEWLINE]
        );
        assert_eq!(newlines("x = A\n|y| y"), [NEWLINE]);
        assert_eq!(newlines("fn f() -> A fails {}\n  | B"), [NEWLINE]);
        assert_eq!(newlines("fn f() -> A fails B uses { C }\n  | D"), [NEWLINE]);
        assert_eq!(
            newlines("fn f() -> A fails B {\n  | C\n}"),
            [NEWLINE, NEWLINE]
        );
        assert_eq!(newlines("fails = A\n| b"), [NEWLINE]);
        // A lower name other than `fails` before a type name does not open an error set.
        assert_eq!(newlines("type T(x: Int) derive Eq\n|y| y"), [NEWLINE]);
    }

    #[test]
    fn fails_と_uses_の後に名前か波括弧が続く行は続き() {
        // Rule 4 of spec 2.5, with the example there.
        let src = "pub fn get(url: String) -> Response\n  fails InvalidUrl | ConnectionFailed\n  uses { Http } {\n  x\n}";
        assert_eq!(
            newlines(src),
            [NEWLINE_CONT, NEWLINE_CONT, NEWLINE, NEWLINE]
        );
        assert_eq!(
            newlines("fn f() -> Int\n  uses Log {\n  1\n}"),
            [NEWLINE_CONT, NEWLINE, NEWLINE]
        );
        assert_eq!(newlines("fn f[R]() -> Int\n  uses R"), [NEWLINE_CONT]);
        assert_eq!(
            newlines("fn f() -> Int\n  fails {}\n  uses {}"),
            [NEWLINE_CONT; 2]
        );
        // Effect operations and functions without a body.
        assert_eq!(
            newlines("effect E {\n  fn get() -> Int\n    fails Boom\n  fn put() -> ()\n}"),
            [NEWLINE, NEWLINE_CONT, NEWLINE, NEWLINE]
        );
        assert_eq!(
            newlines("@external(js, \"f\")\nfn f() -> Int\n  uses {}\nfn g() { 1 }"),
            [NEWLINE, NEWLINE_CONT, NEWLINE]
        );
        // Anywhere, including the type annotation of a binding.
        assert_eq!(
            newlines("fn f() {\n  g: fn() -> Int\n    fails Boom\n    uses { Db } = h\n}"),
            [NEWLINE, NEWLINE_CONT, NEWLINE_CONT, NEWLINE]
        );
    }

    #[test]
    fn 名前としての_fails_と_uses_で始まる行は新しい文() {
        assert_eq!(
            newlines("fn f() {\n  a\n  fails = 1\n  uses = 2\n  uses + fails\n}"),
            [NEWLINE; 5]
        );
        assert_eq!(newlines("a\nuses(x)\nuses.y\nfails\nuses"), [NEWLINE; 4]);
        assert_eq!(
            newlines("[\n  a\n]\nuses: Int = 1"),
            [NEWLINE_CONT, NEWLINE_CONT, NEWLINE]
        );
    }

    #[test]
    fn use_で始まる行は続かない() {
        // `use` starts an expression; the old clause on its own line is left to the parser
        // (E0147).
        assert_eq!(
            newlines("fn f() {\n  a\n  use Log\n}"),
            [NEWLINE, NEWLINE, NEWLINE]
        );
        assert_eq!(
            newlines("fn f() -> Int\n  use Log {\n  1\n}"),
            [NEWLINE, NEWLINE, NEWLINE]
        );
        assert_eq!(
            newlines("effect E {\n  fn get() -> Int\n}\nfails\nuse X"),
            [NEWLINE, NEWLINE, NEWLINE, NEWLINE]
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
