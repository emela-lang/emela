//! The grammar, one function per rule of spec chapter 17.

mod expressions;
mod items;
mod patterns;
mod types;

use super::{CompletedMarker, Parser};
use crate::SyntaxKind::{self, *};
use crate::diagnostic::DiagnosticCode;

pub(crate) use items::source_file as root;

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
        element(p);
        while p.eat(NEWLINE) {}
        if !p.eat(COMMA) {
            break;
        }
        while p.eat(NEWLINE) {}
    }
    p.expect(close);
}

/// `"(" [ param { "," param } ] ")"`．`param = "self" | lower_name [ ":" type ]`
fn param_list(p: &mut Parser<'_>) {
    let m = p.start();
    delimited(p, L_PAREN, R_PAREN, |p| {
        let param = p.start();
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
