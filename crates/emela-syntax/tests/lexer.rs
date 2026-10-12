//! Lexes `test_data/lexer/*.emel` and snapshots the tokens and diagnostics.

use std::fmt::Write;

use emela_syntax::lex;

#[test]
fn lexer_snapshots() {
    insta::glob!("../test_data/lexer", "*.emel", |path| {
        let src = std::fs::read_to_string(path).unwrap();
        insta::assert_snapshot!(dump(&src));
    });
}

/// One token per line as `KIND@start..end "text"`, followed by the diagnostics as
/// `error[E0101]@start..end: message`.
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
        writeln!(
            out,
            "error[{}]@{range:?}: {}",
            diagnostic.code, diagnostic.message
        )
        .unwrap();
    }
    out
}
