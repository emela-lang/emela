//! 文法．17章の規則ごとに関数を1つ置く．

mod patterns;
mod types;

use super::Parser;
use crate::SyntaxKind::{self, *};

/// 文法はまだ途中．読めないトークンは1つずつ ERROR に包んで ROOT の下に置く．
pub(crate) fn root(p: &mut Parser<'_>) {
    let root = p.start();
    while !p.at_eof() {
        let e = p.start();
        p.bump();
        e.complete(p, ERROR);
    }
    root.complete(p, ROOT);
}

/// `open` で始まり `close` で終わる，カンマ区切りの並び．末尾のカンマを許す．
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
    while !p.at(close) && !p.at_eof() {
        element(p);
        if !p.eat(COMMA) {
            break;
        }
    }
    p.expect(close);
}

/// 数値か文字列のリテラル．補間の中身は `interp` で読む．
fn literal(p: &mut Parser<'_>, interp: fn(&mut Parser<'_>)) {
    if p.at(STRING_QUOTE) {
        string(p, interp);
        return;
    }
    let m = p.start();
    p.bump();
    m.complete(p, LITERAL);
}

/// 字句が部品に分けた文字列を組み立てる．閉じていない文字列と補間は字句が診断を出しているので，
/// ここでは閉じる `"` と `}` がなくても黙って終える．
fn string(p: &mut Parser<'_>, interp: fn(&mut Parser<'_>)) {
    let m = p.start();
    p.bump_kind(STRING_QUOTE);
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
    p.eat(STRING_QUOTE);
    m.complete(p, STRING);
}

#[cfg(test)]
mod tests;
