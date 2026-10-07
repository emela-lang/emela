//! `test_data/lexer/*.emel` を字句解析し，トークン列と診断をスナップショットで比べる．

use std::fmt::Write;

use emela_syntax::lex;

#[test]
fn lexer_snapshots() {
    insta::glob!("../test_data/lexer", "*.emel", |path| {
        let src = std::fs::read_to_string(path).unwrap();
        insta::assert_snapshot!(dump(&src));
    });
}

/// 1トークン1行で `KIND@start..end "テキスト"`，最後に診断を `error@start..end: メッセージ` で並べる．
fn dump(src: &str) -> String {
    let lexed = lex(src);
    let mut out = String::new();
    for token in &lexed.tokens {
        let range = token.range;
        let text = &src[range];
        writeln!(out, "{:?}@{:?} {text:?}", token.kind, range).unwrap();
    }
    for diagnostic in &lexed.diagnostics {
        let range = diagnostic.range;
        writeln!(out, "error@{range:?}: {}", diagnostic.message).unwrap();
    }
    out
}
