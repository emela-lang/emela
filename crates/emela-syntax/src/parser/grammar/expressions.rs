//! 式（17.5）．ブロックと制御構文はまだ．

use super::literal;
use super::types::path;
use crate::SyntaxKind::{self, *};
use crate::diagnostic::DiagnosticCode;
use crate::parser::{CompletedMarker, Parser};

/// 式が読めなかったとき，次のトークンが続きの一部なら消費せずに止まる．
const RECOVERY: &[SyntaxKind] = &[
    R_PAREN, R_BRACK, L_BRACE, R_BRACE, COMMA, EQ, COLON, THIN_ARROW, NEWLINE, INTERP_END,
];

/// `"fail" expr | "assert" expr | pipe_expr`
pub(crate) fn expr(p: &mut Parser<'_>) {
    let kind = match p.current() {
        FAIL_KW => FAIL_EXPR,
        ASSERT_KW => ASSERT_EXPR,
        _ => {
            expr_bp(p, 0);
            return;
        }
    };
    let m = p.start();
    p.bump();
    expr(p);
    m.complete(p, kind);
}

/// 二項演算子の強さ．大きいほど強く結合する．比較は結合しない．
fn infix_power(kind: SyntaxKind) -> Option<u8> {
    Some(match kind {
        PIPE_GT => 1,
        PIPE2 => 2,
        AMP2 => 3,
        EQ2 | NEQ | LT | LTEQ | GT | GTEQ => 4,
        PLUS | MINUS => 5,
        STAR | SLASH | PERCENT => 6,
        _ => return None,
    })
}

/// 強さ `min` 以上の二項演算子をつないで読む（Pratt）．左結合．
fn expr_bp(p: &mut Parser<'_>, min: u8) -> Option<CompletedMarker> {
    let mut lhs = unary(p)?;
    let mut after_comparison = false;
    while let Some(power) = infix_power(p.current()) {
        if power < min {
            break;
        }
        let is_comparison = power == 4;
        if is_comparison && after_comparison {
            p.error(
                DiagnosticCode::ChainedComparison,
                "comparison operators cannot be chained",
            );
        }
        let m = lhs.precede(p);
        p.bump();
        expr_bp(p, power + 1);
        lhs = m.complete(p, BIN_EXPR);
        after_comparison = is_comparison;
    }
    Some(lhs)
}

/// `( "!" | "-" ) unary | "use" type_ref | postfix`
fn unary(p: &mut Parser<'_>) -> Option<CompletedMarker> {
    match p.current() {
        BANG | MINUS => {
            let m = p.start();
            p.bump();
            unary(p);
            Some(m.complete(p, PREFIX_EXPR))
        }
        USE_KW => {
            let m = p.start();
            p.bump();
            if p.at(TYPE_NAME) {
                path(p);
            } else {
                p.expect(TYPE_NAME);
            }
            let use_expr = m.complete(p, USE_EXPR);
            if p.at(DOT) || p.at(L_PAREN) {
                p.error(
                    DiagnosticCode::UnparenthesizedUse,
                    "wrap `use` in parentheses to call its operations: `(use X).op()`",
                );
            }
            Some(use_expr)
        }
        _ => postfix(p),
    }
}

/// `primary { "(" [ args ] ")" | "." lower_name }`
fn postfix(p: &mut Parser<'_>) -> Option<CompletedMarker> {
    let mut lhs = primary(p)?;
    loop {
        match p.current() {
            L_PAREN => {
                let m = lhs.precede(p);
                arg_list(p);
                lhs = m.complete(p, CALL_EXPR);
            }
            DOT => {
                let m = lhs.precede(p);
                p.bump();
                p.expect(LOWER_NAME);
                lhs = m.complete(p, FIELD_EXPR);
            }
            _ => return Some(lhs),
        }
    }
}

fn primary(p: &mut Parser<'_>) -> Option<CompletedMarker> {
    let m = p.start();
    let kind = match p.current() {
        INT | FLOAT | STRING_QUOTE => {
            m.abandon(p);
            return Some(literal(p, interp));
        }
        LOWER_NAME | UPPER_NAME | SELF_KW => {
            p.bump();
            NAME_REF
        }
        UNDERSCORE => {
            p.bump();
            UNDERSCORE_EXPR
        }
        TYPE_NAME => {
            path(p);
            PATH_EXPR
        }
        L_PAREN => paren_expr(p),
        L_BRACK => {
            list_expr(p);
            LIST_EXPR
        }
        _ => {
            m.abandon(p);
            p.err_recover(
                DiagnosticCode::ExpectedExpression,
                "expected expression",
                RECOVERY,
            );
            return None;
        }
    };
    Some(m.complete(p, kind))
}

/// 補間 `#{ }` の中身．
fn interp(p: &mut Parser<'_>) {
    expr(p);
}

/// `()`，`(expr)`，要素が2つ以上のタプル．`(a,)` は診断を出してタプルにする．
fn paren_expr(p: &mut Parser<'_>) -> SyntaxKind {
    p.bump_kind(L_PAREN);
    let mut count = 0;
    let mut trailing_comma = false;
    while !p.at(R_PAREN) && !p.at_eof() {
        count += 1;
        expr(p);
        trailing_comma = p.eat(COMMA);
        if !trailing_comma {
            break;
        }
    }
    if count == 1 && trailing_comma {
        p.error(
            DiagnosticCode::SingleElementTuple,
            "a tuple needs at least two elements",
        );
    }
    p.expect(R_PAREN);
    match (count, trailing_comma) {
        (0, _) => UNIT_EXPR,
        (1, false) => PAREN_EXPR,
        _ => TUPLE_EXPR,
    }
}

/// `"[" [ expr { "," expr } [ "," ".." expr ] ] "]"`
fn list_expr(p: &mut Parser<'_>) {
    p.bump_kind(L_BRACK);
    let mut count = 0;
    let mut seen_rest = false;
    while !p.at(R_BRACK) && !p.at_eof() {
        if seen_rest {
            p.error(DiagnosticCode::RestNotLast, "`..` must come last");
        }
        if p.at(DOT2) {
            if count == 0 {
                p.error(
                    DiagnosticCode::RestWithoutElements,
                    "`..` needs at least one element before it",
                );
            }
            rest_expr(p);
            seen_rest = true;
        } else {
            count += 1;
            expr(p);
        }
        if !p.eat(COMMA) {
            break;
        }
    }
    p.expect(R_BRACK);
}

/// `..` と，あれば後ろの式．
fn rest_expr(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump_kind(DOT2);
    if !matches!(p.current(), COMMA | R_BRACK | R_PAREN) {
        expr(p);
    }
    m.complete(p, REST_EXPR);
}

/// `"(" [ arg { "," arg } ] ")"`．`arg = expr | lower_name ":" [ expr ]`
///
/// 束縛の左辺 `User(name:, ..)` として読むために，`..` も受け付ける．
fn arg_list(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump_kind(L_PAREN);
    let mut seen_named = false;
    while !p.at(R_PAREN) && !p.at_eof() {
        if p.at(LOWER_NAME) && p.nth(1) == COLON {
            let a = p.start();
            p.bump();
            p.bump();
            if !p.at(COMMA) && !p.at(R_PAREN) {
                expr(p);
            }
            a.complete(p, NAMED_ARG);
            seen_named = true;
        } else if p.at(DOT2) {
            rest_expr(p);
        } else {
            if seen_named {
                p.error(
                    DiagnosticCode::PositionalAfterNamed,
                    "positional arguments cannot follow named arguments",
                );
            }
            expr(p);
        }
        if !p.eat(COMMA) {
            break;
        }
    }
    p.expect(R_PAREN);
    m.complete(p, ARG_LIST);
}
