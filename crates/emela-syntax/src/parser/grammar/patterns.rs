//! Patterns (spec 17.6).

use super::literal;
use super::types::path;
use crate::SyntaxKind::{self, *};
use crate::diagnostic::DiagnosticCode;
use crate::parser::Parser;

/// When a pattern fails to parse, stop without consuming the next token if it belongs to what
/// follows.
const RECOVERY: &[SyntaxKind] = &[
    R_PAREN, R_BRACK, L_BRACE, R_BRACE, COMMA, EQ, COLON, THIN_ARROW, IF_KW, NEWLINE,
];

/// Whether [`pattern`] can read something here.
pub(crate) fn at_pattern_start(p: &Parser<'_>) -> bool {
    match p.current() {
        UNDERSCORE | LOWER_NAME | UPPER_NAME | INT | FLOAT | STRING_QUOTE | TRIPLE_QUOTE
        | TYPE_NAME | L_PAREN | L_BRACK => true,
        MINUS => matches!(p.nth(1), INT | FLOAT),
        _ => false,
    }
}

pub(crate) fn pattern(p: &mut Parser<'_>) {
    match p.current() {
        UNDERSCORE => token_node(p, WILDCARD_PAT),
        LOWER_NAME => token_node(p, IDENT_PAT),
        UPPER_NAME => token_node(p, CONST_PAT),
        INT | FLOAT | STRING_QUOTE | TRIPLE_QUOTE => {
            let m = p.start();
            literal(p, interp_in_pattern);
            m.complete(p, LITERAL_PAT);
        }
        // A negative number is a pattern-only form (spec 6.5): the `-` belongs to the literal.
        MINUS if matches!(p.nth(1), INT | FLOAT) => {
            let m = p.start();
            p.bump();
            literal(p, interp_in_pattern);
            m.complete(p, LITERAL_PAT);
        }
        TYPE_NAME => variant_pat(p),
        L_PAREN => paren_pat(p),
        L_BRACK => list_pat(p),
        _ => p.err_recover(
            DiagnosticCode::ExpectedPattern,
            "expected pattern",
            RECOVERY,
        ),
    }
}

fn token_node(p: &mut Parser<'_>, kind: SyntaxKind) {
    let m = p.start();
    p.bump();
    m.complete(p, kind);
}

/// Interpolation in a pattern string has no meaning, so report it and skip to `}`.
fn interp_in_pattern(p: &mut Parser<'_>) {
    p.error(
        DiagnosticCode::InterpolationInPattern,
        "string interpolation is not allowed in a pattern",
    );
    let m = p.start();
    let mut depth = 0;
    while !(p.at_eof() || depth == 0 && p.at(INTERP_END)) {
        match p.current() {
            INTERP_START => depth += 1,
            INTERP_END => depth -= 1,
            _ => {}
        }
        p.bump();
    }
    m.complete(p, ERROR);
}

/// `type_ref [ "(" pat_args ")" ]`
fn variant_pat(p: &mut Parser<'_>) {
    let m = p.start();
    path(p);
    if p.at(L_PAREN) {
        pat_args(p);
    }
    m.complete(p, VARIANT_PAT);
}

/// `".." | pat_arg { "," pat_arg } [ "," ".." ]`．`pat_arg = pattern | lower_name ":" [ pattern ]`
fn pat_args(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump_kind(L_PAREN);
    let mut seen_rest = false;
    while !p.at(R_PAREN) && !p.at_eof() {
        if seen_rest {
            p.error(DiagnosticCode::RestNotLast, "`..` must come last");
        }
        if p.at(DOT2) {
            rest_pat(p, false);
            seen_rest = true;
        } else if p.at(LOWER_NAME) && p.nth(1) == COLON {
            let f = p.start();
            p.bump();
            p.bump();
            if !p.at(COMMA) && !p.at(R_PAREN) {
                pattern(p);
            }
            f.complete(p, FIELD_PAT);
        } else {
            pattern(p);
        }
        if !p.eat(COMMA) {
            break;
        }
    }
    p.expect(R_PAREN);
    m.complete(p, PAT_ARG_LIST);
}

/// `()` or a tuple of two or more.
fn paren_pat(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump_kind(L_PAREN);
    let mut count = 0;
    while !p.at(R_PAREN) && !p.at_eof() {
        count += 1;
        pattern(p);
        if !p.eat(COMMA) {
            break;
        }
    }
    if count == 1 {
        p.error(
            DiagnosticCode::SingleElementTuple,
            "a tuple pattern needs at least two elements",
        );
    }
    p.expect(R_PAREN);
    m.complete(p, if count == 0 { UNIT_PAT } else { TUPLE_PAT });
}

/// `"[" "]" | "[" pattern { "," pattern } [ "," ".." [ lower_name ] ] "]"`
fn list_pat(p: &mut Parser<'_>) {
    let m = p.start();
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
            rest_pat(p, true);
            seen_rest = true;
        } else {
            count += 1;
            pattern(p);
        }
        if !p.eat(COMMA) {
            break;
        }
    }
    p.expect(R_BRACK);
    m.complete(p, LIST_PAT);
}

/// In a list, a name may follow.
fn rest_pat(p: &mut Parser<'_>, named: bool) {
    let m = p.start();
    p.bump_kind(DOT2);
    if named && p.at(LOWER_NAME) {
        p.bump();
    }
    m.complete(p, REST_PAT);
}
