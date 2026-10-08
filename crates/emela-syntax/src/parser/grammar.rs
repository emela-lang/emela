//! 文法．17章の規則ごとに関数を1つ置く．

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

#[cfg(test)]
mod tests;
