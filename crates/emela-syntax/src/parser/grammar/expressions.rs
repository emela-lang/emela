//! Expressions (spec 17.5).

use super::patterns::pattern;
use super::types::{effect_type_args, path, type_};
use super::{lambda_param_list, literal};
use crate::SyntaxKind::{self, *};
use crate::diagnostic::DiagnosticCode;
use crate::parser::{CompletedMarker, Parser};

/// When an expression fails to parse, stop without consuming the next token if it belongs to
/// what follows.
const RECOVERY: &[SyntaxKind] = &[
    R_PAREN, R_BRACK, L_BRACE, R_BRACE, COMMA, EQ, COLON, THIN_ARROW, NEWLINE, INTERP_END,
];

/// `"fail" expr | "assert" expr | pipe_expr { ( "escape" | "match" ) "{" { arm NL } "}" }`
pub(crate) fn expr(p: &mut Parser<'_>) -> Option<CompletedMarker> {
    let kind = match p.current() {
        FAIL_KW => FAIL_EXPR,
        ASSERT_KW => ASSERT_EXPR,
        _ => {
            let mut lhs = expr_bp(p, 0)?;
            loop {
                let kind = match p.current() {
                    ESCAPE_KW => ESCAPE_EXPR,
                    MATCH_KW => MATCH_EXPR,
                    _ => break,
                };
                let m = lhs.precede(p);
                p.bump();
                arm_list(p);
                lhs = m.complete(p, kind);
            }
            return Some(lhs);
        }
    };
    let m = p.start();
    p.bump();
    expr(p);
    Some(m.complete(p, kind))
}

/// `binding | expr`．`binding = pattern [ ":" type ] "=" expr`
///
/// The left side is parsed as an expression first, and rewrapped in BINDING if `:` or `=`
/// follows.
pub(crate) fn stmt(p: &mut Parser<'_>) {
    let Some(lhs) = expr(p) else { return };
    if !p.at(COLON) && !p.at(EQ) {
        return;
    }
    let m = lhs.precede(p);
    if p.eat(COLON) {
        type_(p);
    }
    p.expect(EQ);
    expr(p);
    m.complete(p, BINDING);
}

/// `"{" { stmt NL } "}"`
pub(crate) fn block(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    p.bump_kind(L_BRACE);
    loop {
        while p.eat(NEWLINE) {}
        if p.at(R_BRACE) || p.at_eof() || p.at_decl_start() {
            break;
        }
        let before = p.position();
        stmt(p);
        if p.position() == before {
            // A diagnostic was reported but no token was consumed (e.g. `=` at line start).
            bump_error(p);
            continue;
        }
        if !at_stmt_end(p) {
            p.error(
                DiagnosticCode::ExpectedStatementEnd,
                "expected newline or `}` after statement",
            );
            skip_to_line_end(p);
        }
    }
    p.expect(R_BRACE);
    m.complete(p, BLOCK_EXPR)
}

fn bump_error(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump();
    m.complete(p, ERROR);
}

/// Inside an unclosed `(` newlines are continuations, so a line start also ends a statement.
fn at_stmt_end(p: &Parser<'_>) -> bool {
    p.at(NEWLINE) || p.at(R_BRACE) || p.at_eof() || p.at_line_start()
}

/// Wraps everything up to a newline, the matching outer `}` or a declaration at the start of a
/// line in ERROR and skips it.
fn skip_to_line_end(p: &mut Parser<'_>) {
    let m = p.start();
    let mut depth = 0usize;
    while !(p.at_eof() || p.at_decl_start() || depth == 0 && (p.at(NEWLINE) || p.at(R_BRACE))) {
        match p.current() {
            L_BRACE => depth += 1,
            R_BRACE => depth -= 1,
            _ => {}
        }
        p.bump();
    }
    m.complete(p, ERROR);
}

/// `"if" expr block [ "else" ( block | if_expr ) ]`
fn if_expr(p: &mut Parser<'_>) {
    p.bump_kind(IF_KW);
    expr(p);
    block_or_error(p);
    if p.eat(ELSE_KW) {
        if p.at(IF_KW) {
            let m = p.start();
            if_expr(p);
            m.complete(p, IF_EXPR);
        } else {
            block_or_error(p);
        }
    }
}

/// Without `{`, an expression on the same line is read as the body after one diagnostic.
fn block_or_error(p: &mut Parser<'_>) {
    if p.at(L_BRACE) {
        block(p);
        return;
    }
    p.expect(L_BRACE);
    if !matches!(p.current(), NEWLINE | R_BRACE | EOF) {
        expr(p);
    }
}

/// `"{" { arm NL } "}"`．`arm = pattern [ "if" expr ] "->" expr`
fn arm_list(p: &mut Parser<'_>) {
    let m = p.start();
    if !p.expect(L_BRACE) {
        m.complete(p, MATCH_ARM_LIST);
        return;
    }
    loop {
        while p.eat(NEWLINE) {}
        if p.at(R_BRACE) || p.at_eof() || p.at_decl_start() {
            break;
        }
        let before = p.position();
        arm(p);
        if p.position() == before {
            // A diagnostic was reported but no token was consumed (e.g. `=` at line start).
            bump_error(p);
            continue;
        }
        if !at_stmt_end(p) {
            p.error(
                DiagnosticCode::ExpectedStatementEnd,
                "expected newline or `}` after match arm",
            );
            skip_to_line_end(p);
        }
    }
    p.expect(R_BRACE);
    m.complete(p, MATCH_ARM_LIST);
}

fn arm(p: &mut Parser<'_>) {
    let m = p.start();
    pattern(p);
    if p.at(IF_KW) {
        let g = p.start();
        p.bump();
        expr(p);
        g.complete(p, MATCH_GUARD);
    }
    // Even without the arrow, an expression on the same line is read as the body.
    if p.expect(THIN_ARROW) || !matches!(p.current(), NEWLINE | R_BRACE | EOF) {
        expr(p);
    }
    m.complete(p, MATCH_ARM);
}

/// `"with" with_item { "," with_item } block`,
/// `with_item = supply [ "by" by_right ]`,
/// `by_right = type_ref [ "[" type { "," type } "]" | "(" [ args ] ")" ]`
fn with_expr(p: &mut Parser<'_>) {
    p.bump_kind(WITH_KW);
    loop {
        if p.at(TYPE_NAME) {
            let target = supply(p);
            // `AuditLog by HandlerA` plugs a provider into a label.
            if p.at_contextual_kw("by") {
                let m = target.precede(p);
                p.bump();
                if p.at(TYPE_NAME) {
                    supply(p);
                    effect_type_args(p);
                } else {
                    p.expect(TYPE_NAME);
                }
                m.complete(p, WITH_BY);
            }
        } else {
            p.expect(TYPE_NAME);
        }
        if !p.eat(COMMA) {
            break;
        }
    }
    block_or_error(p);
}

/// `supply = type_ref [ "(" [ args ] ")" ]`: a handler, a layer or a label, with the arguments
/// of a handler's init (`WorkerKv(binding: "KV")`). With arguments, the PATH and the ARG_LIST are
/// wrapped in a SUPPLY; without, the item is the PATH itself.
pub(super) fn supply(p: &mut Parser<'_>) -> CompletedMarker {
    let target = path(p);
    if !p.at(L_PAREN) {
        return target;
    }
    let m = target.precede(p);
    arg_list(p);
    m.complete(p, SUPPLY)
}

/// Higher binds tighter. Comparisons do not associate.
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

/// Pratt parsing of binary operators of strength `min` or more. Left-associative.
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

/// `( "!" | "-" ) unary | "use" type_ref [ "[" type { "," type } "]" ] | postfix`
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
                effect_type_args(p);
            } else {
                p.expect(TYPE_NAME);
            }
            let use_expr = m.complete(p, USE_EXPR);
            if !p.at(DOT) && !p.at(L_PAREN) {
                return Some(use_expr);
            }
            // Treat it as forgotten parentheses: one diagnostic, then read the rest as a call.
            p.error(
                DiagnosticCode::UnparenthesizedUse,
                "wrap `use` in parentheses to call its operations: `(use X).op()`",
            );
            Some(postfix_ops(p, use_expr))
        }
        _ => {
            let lhs = primary(p)?;
            Some(postfix_ops(p, lhs))
        }
    }
}

/// `{ "(" [ args ] ")" | "." lower_name }` after `primary`
fn postfix_ops(p: &mut Parser<'_>, mut lhs: CompletedMarker) -> CompletedMarker {
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
            _ => return lhs,
        }
    }
}

fn primary(p: &mut Parser<'_>) -> Option<CompletedMarker> {
    let m = p.start();
    let kind = match p.current() {
        INT | FLOAT | STRING_QUOTE | TRIPLE_QUOTE => {
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
        L_BRACE => {
            m.abandon(p);
            return Some(block(p));
        }
        IF_KW => {
            if_expr(p);
            IF_EXPR
        }
        // Prefix `match x { ... }` is not in the grammar (spec 6.5). It is read in that shape to
        // recover, keeping the arms for the later passes.
        MATCH_KW => {
            p.error(
                DiagnosticCode::PrefixMatch,
                "`match` comes after the value: write `value match { ... }`",
            );
            p.bump();
            expr(p);
            arm_list(p);
            MATCH_EXPR
        }
        WITH_KW => {
            with_expr(p);
            WITH_EXPR
        }
        // `lambda = "fn" "(" [ params ] ")" block`
        FN_KW => {
            p.bump();
            if p.at(L_PAREN) {
                lambda_param_list(p);
            } else {
                p.expect(L_PAREN);
            }
            block_or_error(p);
            LAMBDA_EXPR
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

fn interp(p: &mut Parser<'_>) {
    expr(p);
}

/// `()`, `(expr)`, or a tuple of two or more. `(a,)` gets a diagnostic and becomes a tuple.
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

/// `..` and the expression after it, if any.
fn rest_expr(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump_kind(DOT2);
    if !matches!(p.current(), COMMA | R_BRACK | R_PAREN) {
        expr(p);
    }
    m.complete(p, REST_EXPR);
}

/// `"(" [ args ] ")"`．
///
/// `args = arg { "," arg } [ "," ".." expr ] | ".." expr`，`arg = expr | lower_name ":" [ expr ]`
///
/// A trailing `..expr` is a partial update of a constructor (spec 5.3); whether the callee is a
/// constructor is not checked here. A bare `..` is also accepted so that `User(name:, ..)` can
/// be read as the left side of a binding.
fn arg_list(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump_kind(L_PAREN);
    let mut seen_named = false;
    let mut seen_spread = false;
    while !p.at(R_PAREN) && !p.at_eof() {
        if seen_spread {
            p.error(DiagnosticCode::RestNotLast, "`..` must come last");
        }
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
            let a = p.start();
            p.bump();
            if !p.at(COMMA) && !p.at(R_PAREN) {
                expr(p);
            }
            a.complete(p, SPREAD_ARG);
            seen_spread = true;
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
