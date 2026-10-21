//! Types (spec 17.4).

use super::delimited;
use crate::SyntaxKind::*;
use crate::diagnostic::DiagnosticCode;
use crate::parser::{CompletedMarker, Parser};

/// When a type fails to parse, stop without consuming the next token if it belongs to what
/// follows.
const RECOVERY: &[crate::SyntaxKind] = &[
    R_PAREN, R_BRACK, L_BRACE, R_BRACE, COMMA, EQ, THIN_ARROW, PIPE, NEWLINE,
];

pub(crate) fn type_(p: &mut Parser<'_>) {
    match p.current() {
        TYPE_NAME => path_type(p),
        UPPER_NAME => {
            type_var(p);
        }
        SELF_TYPE_KW => {
            let m = p.start();
            p.bump();
            m.complete(p, SELF_TYPE);
        }
        L_PAREN => paren_type(p),
        FN_KW => fn_type(p),
        _ => p.err_recover(DiagnosticCode::ExpectedType, "expected type", RECOVERY),
    }
}

/// `type_ref [ "[" type_arg { "," type_arg } "]" ]`
fn path_type(p: &mut Parser<'_>) {
    let m = p.start();
    path(p);
    if p.at(L_BRACK) {
        let args = p.start();
        delimited(p, L_BRACK, R_BRACK, type_arg);
        args.complete(p, TYPE_ARG_LIST);
    }
    m.complete(p, PATH_TYPE);
}

/// `type_arg = type | "{" [ effect_item { "," effect_item } ] "}" | error_item "|" error_item { "|" error_item }`
///
/// A set passed as a type argument to a set type parameter of a named type
/// (`Handler[NotFound | Timeout, { Db, Log }]`, spec 9.2). Braces are the effect set notation
/// (`{}` is the empty set and may also stand where an error set goes); `|` joins an error set.
/// A single element is read as a type (`Handler[NotFound, Db]`). Which type arguments take a set
/// is not checked here; type inference does (E0323, E0324).
fn type_arg(p: &mut Parser<'_>) {
    if p.at(L_BRACE) {
        effect_set(p);
        return;
    }
    // `A | B` or `Mod.A | B`.
    let mut n = 0;
    match p.current() {
        TYPE_NAME => {
            n = 1;
            while p.nth(n) == DOT && p.nth(n + 1) == TYPE_NAME {
                n += 2;
            }
        }
        UPPER_NAME => n = 1,
        _ => {}
    }
    if n > 0 && p.nth(n) == PIPE {
        error_set(p);
    } else {
        type_(p);
    }
}

/// Effect type arguments `[Int]` (`handler H: State[Int]` in spec 8.1, `use State[Int]` in 17.5).
/// Becomes a TYPE_ARG_LIST sibling of the preceding type_ref. Does nothing without `[`.
pub(crate) fn effect_type_args(p: &mut Parser<'_>) {
    if p.at(L_BRACK) {
        let args = p.start();
        delimited(p, L_BRACK, R_BRACK, type_arg);
        args.complete(p, TYPE_ARG_LIST);
    }
}

/// `type_ref = [ module_path "." ] type_name`. The caller checks that it is at TYPE_NAME.
pub(crate) fn path(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    p.bump_kind(TYPE_NAME);
    while p.at(DOT) && p.nth(1) == TYPE_NAME {
        p.bump();
        p.bump();
    }
    m.complete(p, PATH)
}

fn type_var(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    p.bump_kind(UPPER_NAME);
    m.complete(p, TYPE_VAR)
}

/// `()`, a parenthesized type `(T)`, or a tuple of two or more.
///
/// Parentheses tell where fails and use attach in a function type that returns a function
/// (spec 5.5). `(T,)` would be a one-element tuple, so the closing parenthesis gets a diagnostic
/// and it is read as a tuple.
fn paren_type(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump_kind(L_PAREN);
    let mut count = 0;
    let mut trailing_comma = false;
    while !p.at(R_PAREN) && !p.at_eof() {
        count += 1;
        type_(p);
        trailing_comma = p.eat(COMMA);
        if !trailing_comma {
            break;
        }
    }
    if count == 1 && trailing_comma {
        p.error(
            DiagnosticCode::SingleElementTuple,
            "a tuple type needs at least two elements",
        );
    }
    p.expect(R_PAREN);
    let kind = match (count, trailing_comma) {
        (0, _) => UNIT_TYPE,
        (1, false) => PAREN_TYPE,
        _ => TUPLE_TYPE,
    };
    m.complete(p, kind);
}

/// `"fn" "(" [ type { "," type } ] ")" "->" type [ "fails" error_set ] [ "use" effect_set ]`
fn fn_type(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump_kind(FN_KW);
    let has_params = p.at(L_PAREN);
    if has_params {
        let params = p.start();
        delimited(p, L_PAREN, R_PAREN, type_);
        params.complete(p, PARAM_TYPE_LIST);
    } else {
        p.expect(L_PAREN);
    }
    // Without a parameter list, do not pile a `->` diagnostic on top.
    if p.at(THIN_ARROW) {
        ret_type(p);
    } else if has_params {
        p.expect(THIN_ARROW);
    }
    effects_and_errors(p);
    m.complete(p, FN_TYPE);
}

/// `"->" type`
pub(crate) fn ret_type(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump_kind(THIN_ARROW);
    type_(p);
    m.complete(p, RET_TYPE);
}

/// `[ "fails" error_set ] [ "use" effect_set ]`
pub(crate) fn effects_and_errors(p: &mut Parser<'_>) {
    if p.at_contextual_kw("fails") {
        let m = p.start();
        p.bump();
        error_set(p);
        m.complete(p, FAILS_CLAUSE);
    }
    if p.at(USE_KW) {
        let m = p.start();
        p.bump();
        effect_set(p);
        m.complete(p, USE_CLAUSE);
    }
}

/// `"{" "}" | error_item { "|" error_item }`．`error_item = type_ref | upper_name`
///
/// Error tags (`NotFound`) and set type parameters (`E`) may be mixed (`Timeout | E`, spec 7.4).
fn error_set(p: &mut Parser<'_>) {
    let m = p.start();
    if p.at(L_BRACE) {
        p.bump();
        p.expect(R_BRACE);
    } else {
        set_element(p);
        while p.eat(PIPE) {
            set_element(p);
        }
    }
    m.complete(p, ERROR_SET);
}

/// `"{" [ effect_item { "," effect_item } ] "}" | effect_item`．
/// `effect_item = type_ref [ "[" type { "," type } "]" ] | upper_name`
///
/// Effects (`Quiet`) and set type parameters (`R`) may be mixed (`{ Quiet, R }`, spec 8.3).
/// Newlines inside braces remain as separators in the token stream, so they are skipped here.
fn effect_set(p: &mut Parser<'_>) {
    let m = p.start();
    if p.at(L_BRACE) {
        p.bump();
        while p.eat(NEWLINE) {}
        while !p.at(R_BRACE) && !p.at_eof() {
            set_element(p);
            while p.eat(NEWLINE) {}
            if !p.eat(COMMA) {
                break;
            }
            while p.eat(NEWLINE) {}
        }
        p.expect(R_BRACE);
    } else {
        set_element(p);
    }
    m.complete(p, EFFECT_SET);
}

/// A type_ref or an uppercase name. An effect with type arguments (`State[Int]`, spec 8.1)
/// wraps the type_ref and the TYPE_ARG_LIST in a PATH_TYPE.
fn set_element(p: &mut Parser<'_>) {
    match p.current() {
        TYPE_NAME => {
            let path = path(p);
            if p.at(L_BRACK) {
                let m = path.precede(p);
                effect_type_args(p);
                m.complete(p, PATH_TYPE);
            }
        }
        UPPER_NAME => {
            type_var(p);
        }
        _ => p.err_recover(DiagnosticCode::ExpectedType, "expected type", RECOVERY),
    }
}
