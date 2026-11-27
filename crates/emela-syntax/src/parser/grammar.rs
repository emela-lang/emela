//! The grammar, one function per rule of spec chapter 17.

mod expressions;
mod items;
mod patterns;
mod types;

use super::{CompletedMarker, Marker, Parser};
use crate::SyntaxKind::{self, *};
use crate::diagnostic::DiagnosticCode;

pub(crate) use items::source_file as root;

/// Reads separating newlines. If a doc comment is attached, returns a marker opened at its
/// position. The declaration opens with this marker, so `sink` puts the comment inside the
/// declaration node.
fn newlines_with_doc(p: &mut Parser<'_>) -> Option<Marker> {
    let mut doc = None;
    while p.at(NEWLINE) {
        if doc.is_none() && p.at_doc_comment() {
            doc = Some(p.start());
        }
        p.bump();
    }
    doc
}

/// `local_fn = "fn" lower_name "(" [ params ] ")" [ signature ] block` (`[fn.local]`), a statement
/// of a block. `m` is opened before its doc comment, if any. It is a declaration read from inside
/// an expression, so it lives here rather than in `items` or `expressions`.
///
/// The parser also reads the attributes, `pub`, `suspend` and type parameters of a top-level fn
/// into the node, and validation reports them (E1212, E1213). A missing body is reported here.
fn local_fn(p: &mut Parser<'_>, m: Marker) {
    while p.at(AT) {
        items::annotation(p);
        while p.eat(NEWLINE) {}
    }
    p.eat(PUB_KW);
    let has_body = items::fn_head_and_body(p);
    if !has_body {
        p.error(
            DiagnosticCode::ExpectedToken,
            "expected `{` and the body of the local function",
        );
    }
    m.complete(p, LOCAL_FN);
}

/// Whether the parser is at the start of a local fn: `fn` and a name, or the attributes, `pub`
/// and `suspend` that a top-level fn can have before `fn`. A statement cannot start with any of
/// them otherwise.
fn at_local_fn_start(p: &Parser<'_>) -> bool {
    match p.current() {
        FN_KW => p.nth(1) == LOWER_NAME,
        SUSPEND_KW => p.nth(1) == FN_KW,
        PUB_KW => matches!(p.nth(1), FN_KW | SUSPEND_KW),
        AT => true,
        _ => false,
    }
}

/// A comma-separated list from `open` to `close`, with an optional trailing comma. Newlines
/// inside braces remain as separators in the token stream, so they are skipped around elements.
///
/// `element` must only report a diagnostic when it cannot read an element, without consuming a
/// separator or the closing bracket. A missing closing bracket is reported and parsing stops
/// before it.
fn delimited(
    p: &mut Parser<'_>,
    open: SyntaxKind,
    close: SyntaxKind,
    mut element: impl FnMut(&mut Parser<'_>),
) {
    p.bump_kind(open);
    while p.eat(NEWLINE) {}
    while !p.at(close) && !p.at_eof() {
        // A `|` inside brackets is not the end of a closure's parameter list.
        p.in_brackets(&mut element);
        while p.eat(NEWLINE) {}
        if !p.eat(COMMA) {
            break;
        }
        while p.eat(NEWLINE) {}
    }
    p.expect(close);
}

/// `"(" [ param { "," param } ] ")"`．`param = "self" | { annotation } lower_name [ ":" type ]`
fn param_list(p: &mut Parser<'_>) {
    let m = p.start();
    delimited(p, L_PAREN, R_PAREN, |p| {
        let param = p.start();
        // `@borrowed f: fn() -> ()` (`[ext.boundary]`).
        while p.at(AT) {
            items::annotation(p);
        }
        if p.eat(LOWER_NAME) {
            if p.eat(COLON) {
                types::type_(p);
            }
        } else if !p.eat(SELF_KW) {
            p.err_recover(
                DiagnosticCode::ExpectedToken,
                "expected parameter name",
                &[COMMA, R_PAREN, L_BRACE, NEWLINE],
            );
        }
        param.complete(p, PARAM);
    });
    m.complete(p, PARAM_LIST);
}

/// The parameters of the old form of an anonymous function, `fn(x) { ... }` (E1405, reported by
/// validation): `"(" [ lambda_param { "," lambda_param } ] ")"`. It is still read so that the
/// formatter and the quick fix can rewrite it (`[fmt.closure]`).
fn old_lambda_param_list(p: &mut Parser<'_>) {
    let m = p.start();
    delimited(p, L_PAREN, R_PAREN, lambda_param);
    m.complete(p, PARAM_LIST);
}

/// `"||" | "|" [ lambda_param { "," lambda_param } ] "|"` (`[grammar.expr]`). The list is a
/// PARAM_LIST whose PARAMs have the shape of the old form's.
///
/// The list does not span lines: outside brackets a newline inside it is a statement separator,
/// and is reported as a missing parameter.
fn closure_param_list(p: &mut Parser<'_>) {
    let m = p.start();
    if p.eat(PIPE2) {
        m.complete(p, PARAM_LIST);
        return;
    }
    p.bump_kind(PIPE);
    p.in_closure_param_list(|p| {
        while !p.at(PIPE) && !p.at_eof() {
            lambda_param(p);
            if !p.eat(COMMA) {
                break;
            }
        }
    });
    p.expect(PIPE);
    m.complete(p, PARAM_LIST);
}

/// `lambda_param = pattern [ ":" type ]` (`[fn.decl]`). A plain name keeps the shape of
/// [`param_list`] (a `LOWER_NAME` token directly under `PARAM`); any other pattern becomes a
/// pattern node under `PARAM`.
fn lambda_param(p: &mut Parser<'_>) {
    let param = p.start();
    let named = p.eat(LOWER_NAME) || p.eat(SELF_KW);
    if named {
        // The shape of `param_list`.
    } else if patterns::at_pattern_start(p) {
        patterns::pattern(p);
    } else {
        p.err_recover(
            DiagnosticCode::ExpectedToken,
            "expected parameter name or pattern",
            &[COMMA, R_PAREN, PIPE, L_BRACE, NEWLINE],
        );
    }
    if p.eat(COLON) {
        types::type_(p);
    }
    param.complete(p, PARAM);
}

/// Interpolation contents are read with `interp`.
fn literal(p: &mut Parser<'_>, interp: fn(&mut Parser<'_>)) -> CompletedMarker {
    if p.at(STRING_QUOTE) || p.at(TRIPLE_QUOTE) {
        return string(p, interp);
    }
    let m = p.start();
    p.bump();
    m.complete(p, LITERAL)
}

/// Assembles a string the lexer split into pieces. The lexer has already reported unclosed
/// strings and interpolations, so a missing closing `"` or `}` ends the string silently here.
fn string(p: &mut Parser<'_>, interp: fn(&mut Parser<'_>)) -> CompletedMarker {
    let m = p.start();
    // Close with the same kind of quote that opened it.
    let quote = p.current();
    p.bump();
    loop {
        match p.current() {
            STRING_TEXT => p.bump(),
            INTERP_START => {
                let i = p.start();
                p.bump();
                interp(p);
                p.eat(INTERP_END);
                i.complete(p, INTERP);
            }
            _ => break,
        }
    }
    p.eat(quote);
    m.complete(p, STRING)
}

#[cfg(test)]
mod tests;
