//! 文法．17章の規則ごとに関数を1つ置く．

mod expressions;
mod items;
mod patterns;
mod types;

use super::{CompletedMarker, Parser};
use crate::SyntaxKind::{self, *};
use crate::diagnostic::DiagnosticCode;

pub(crate) use items::source_file as root;

/// `open` で始まり `close` で終わる，カンマ区切りの並び．末尾のカンマを許す．
/// 波括弧の中の改行は区切りとして字句に残るので，要素の前後で読み飛ばす．
///
/// 要素を読む `element` は，読めなければ診断を出すだけで，区切りや閉じ括弧を消費しないこと．
/// 閉じ括弧がなければ診断を出し，その手前で止まる．
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

/// 数値か文字列のリテラル．補間の中身は `interp` で読む．
fn literal(p: &mut Parser<'_>, interp: fn(&mut Parser<'_>)) -> CompletedMarker {
    if p.at(STRING_QUOTE) || p.at(TRIPLE_QUOTE) {
        return string(p, interp);
    }
    let m = p.start();
    p.bump();
    m.complete(p, LITERAL)
}

/// 字句が部品に分けた文字列を組み立てる．閉じていない文字列と補間は字句が診断を出しているので，
/// ここでは閉じる `"` と `}` がなくても黙って終える．
fn string(p: &mut Parser<'_>, interp: fn(&mut Parser<'_>)) -> CompletedMarker {
    let m = p.start();
    // 開いた引用符と同じ種類で閉じる．
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
