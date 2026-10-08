//! 文法の単体テスト．`fn` 宣言が読めるまでは，規則ごとの入口を直接呼ぶ．

use std::fmt::Write;

use super::{patterns, types};
use crate::SyntaxKind::*;
use crate::parser::{Parser, build};
use crate::{SyntaxNode, debug_tree, lex};

/// `entry` で読み，残りを ERROR に包んだ木と診断をダンプする．
fn dump(src: &str, entry: fn(&mut Parser<'_>)) -> String {
    let lexed = lex(src);
    let mut p = Parser::new(src, &lexed.tokens);
    let root = p.start();
    entry(&mut p);
    if !p.at_eof() {
        let e = p.start();
        while !p.at_eof() {
            p.bump();
        }
        e.complete(&mut p, ERROR);
    }
    root.complete(&mut p, ROOT);
    let (green, diagnostics) = build(src, &lexed.tokens, p.finish());
    let mut out = debug_tree(&SyntaxNode::new_root(green));
    for d in lexed.diagnostics.iter().chain(&diagnostics) {
        writeln!(out, "error[{}]@{:?}: {}", d.code, d.range, d.message).unwrap();
    }
    out
}

/// ケースごとに `=== 入力` の見出しを付けて並べる．
fn dump_all(cases: &[&str], entry: fn(&mut Parser<'_>)) -> String {
    let mut out = String::new();
    for src in cases {
        writeln!(out, "=== {src}").unwrap();
        out.push_str(&dump(src, entry));
    }
    out
}

#[test]
fn 型() {
    insta::assert_snapshot!(dump_all(
        &[
            "Int",
            "Http.Client",
            "List[A]",
            "Map[String, List[(Int, Bool)]]",
            "Self",
            "()",
            "(Int, String,)",
            "fn(A) -> B",
            "fn() -> () fails NotFound | DbError use Users",
            "fn(A) -> A use R",
            "fn(A) -> A fails {} use { Io, Clock }",
            "fn(A) -> A fails E use {\n  Io,\n  Clock,\n}",
        ],
        types::type_,
    ));
}

#[test]
fn 型のエラー() {
    insta::assert_snapshot!(dump_all(
        &[
            "List[Int",
            "(Int)",
            "List[, Int]",
            "fn(A) B",
            "fn A -> B",
            "fn() -> A fails",
            "fn() -> A use { Io, 1 }",
            "42",
        ],
        types::type_,
    ));
}

#[test]
fn パターン() {
    insta::assert_snapshot!(dump_all(
        &[
            "_",
            "x",
            "MAX_SIZE",
            "42",
            "\"abc\"",
            "Empty",
            "Circle(radius:)",
            "Circle(_)",
            "Rect(w, h)",
            "User(name:, ..)",
            "User(name: n, age: 0)",
            "Option.Some(..)",
            "()",
            "(a, b,)",
            "[]",
            "[x, ..rest]",
            "[x, ..]",
            "[[a, b], (c, d)]",
        ],
        patterns::pattern,
    ));
}

#[test]
fn パターンのエラー() {
    insta::assert_snapshot!(dump_all(
        &[
            "[..rest]",
            "[..]",
            "[x, ..r, y]",
            "User(.., name:)",
            "-1",
            "\"a#{x}b\"",
            "(a)",
            "Some(",
            "[1 2]",
            "+",
        ],
        patterns::pattern,
    ));
}
