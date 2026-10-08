//! 文法の単体テスト．`fn` 宣言が読めるまでは，規則ごとの入口を直接呼ぶ．

use std::fmt::Write;

use super::{expressions, patterns, types};
use crate::SyntaxKind::*;
use crate::parser::{Parser, build};
use crate::{SyntaxNode, debug_tree, lex, validation};

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
    let root = SyntaxNode::new_root(green);
    let mut out = debug_tree(&root);
    let checked = validation::validate(&root);
    for d in lexed.diagnostics.iter().chain(&diagnostics).chain(&checked) {
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
            "(Int)",
            "fn() -> (fn(Int) -> String) fails ParseError",
            "fn() -> fn(Int) -> String fails ParseError",
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
            "(Int,)",
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

#[test]
fn 式() {
    insta::assert_snapshot!(dump_all(
        &[
            "count + 1",
            "a + b * c - d",
            "-x * y",
            "a || b && !c",
            "radius > 0.0",
            "List.partition(xs, is_even)",
            "fail Invalid(input:)",
            "connect(\"localhost\", port: 5432)",
            "scores\n  |> List.filter(is_valid)\n  |> List.fold(init: 0, f: add)",
            "use Logger",
            "(use Clock).sleep(1000)",
            "users.find(id) |> Option.or_fail(NotFound(id:))",
            "log.info(\"hello #{user.name}\")",
            "assert x == 1",
            "self.name",
            "MAX_SIZE",
            "()",
            "(a + b) * c",
            "(a, b,)",
            "[]",
            "[x, y, ..rest]",
            "[x, ..List.tail(xs)]",
        ],
        |p| {
            expressions::expr(p);
        },
    ));
}

#[test]
fn 式のエラー() {
    insta::assert_snapshot!(dump_all(
        &[
            "a < b < c",
            "f(x: 1, 2)",
            "use Clock.sleep(1)",
            "[..a]",
            "[x, ..a, ..b]",
            "f(1, ",
            "1 +",
            "(1",
            "x.",
            "\"a #{} b\"",
            "(a,)",
            "1 + ) + 2",
        ],
        |p| {
            expressions::expr(p);
        },
    ));
}

/// ブロックを入口にして読む．
fn block(p: &mut Parser<'_>) {
    expressions::block(p);
}

#[test]
fn ブロックと文() {
    insta::assert_snapshot!(dump_all(
        &[
            "{\n  count = 0\n  count = count + 1\n  (evens, odds) = List.partition(xs, is_even)\n  pairs: List[(String, Int)] = Map.to_list(scores)\n  label = if count > 0 { \"あり\" } else { \"なし\" }\n  if !valid { fail Invalid(input:) }\n  connect(\"localhost\", port: 5432)\n}",
            "{\n  match shape {\n    Circle(radius:) if radius > 0.0 -> 3.14 * radius * radius\n    Circle(_)                       -> 0.0\n    Empty                           -> 0.0\n  }\n}",
            "{\n  user = find_user(id) escape {\n    NotFound(_) -> guest_user()\n  }\n  data = load(id)\n    escape { DbError(message:) -> fail Unavailable(reason: message) }\n}",
            "{\n  with AppTest, FixedClock { greet(1) }\n  List.map(names, fn(n) { log.info(n) })\n  if a { 1 } else if b { 2 } else { 3 }\n}",
            "{\n  [x, ..rest] = xs\n  User(name:, ..) = user\n  [first, ..] = xs\n  _ = run()\n}",
            "{}",
        ],
        block,
    ));
}

#[test]
fn ブロックと文のエラー() {
    insta::assert_snapshot!(dump_all(
        &[
            "{\n  a b\n  c\n}",
            "{\n  match x {\n    A 1\n    B -> 2\n  }\n}",
            "{\n  a + b = 1\n  f(x) = 2\n  self = 3\n}",
            "{\n  f(_)\n  g([x, ..])\n}",
            "{\n  if x { 1 } else 2\n}",
            "{\n  x = (1\n  y = 2\n}",
            "{\n  a = 1",
        ],
        block,
    ));
}

mod robustness {
    use proptest::prelude::*;

    use super::*;

    /// 入口ごとに，panic せず木のテキストが入力と一致することを確かめる．
    fn lossless(src: &str, entry: fn(&mut Parser<'_>)) -> Result<(), TestCaseError> {
        let lexed = lex(src);
        let mut p = Parser::new(src, &lexed.tokens);
        let root = p.start();
        entry(&mut p);
        while !p.at_eof() {
            p.bump();
        }
        root.complete(&mut p, ROOT);
        let (green, _) = build(src, &lexed.tokens, p.finish());
        let tree = SyntaxNode::new_root(green);
        validation::validate(&tree);
        prop_assert_eq!(tree.text().to_string(), src);
        Ok(())
    }

    const CODE: &str = r##"([a-zA-Z_0-9 \n(){}\[\],.:=|>+\-*/%!<&"#]|fn |if |else |match |with |escape |use |fail |->|\.\.){0,48}"##;

    proptest! {
        #[test]
        fn 型の入口で無損失(src in CODE) {
            lossless(&src, types::type_)?;
        }

        #[test]
        fn パターンの入口で無損失(src in CODE) {
            lossless(&src, patterns::pattern)?;
        }

        #[test]
        fn ブロックの入口で無損失(body in CODE) {
            lossless(&format!("{{{body}"), block)?;
        }

        #[test]
        fn 式の入口で無損失(src in CODE) {
            lossless(&src, |p| {
                expressions::stmt(p);
            })?;
        }
    }
}
