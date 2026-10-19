//! Parser tests. Snapshots the tree and diagnostics for `test_data/parser/{ok,err}/*.emel`, and
//! checks that the ok inputs have no diagnostics.

use emela_syntax::parse;

#[test]
fn ok_snapshots() {
    insta::glob!("../test_data/parser/ok", "*.emel", |path| {
        let src = std::fs::read_to_string(path).unwrap();
        let parse = parse(&src);
        assert!(parse.diagnostics().is_empty(), "{}", parse.debug_dump());
        insta::assert_snapshot!(parse.debug_dump());
    });
}

#[test]
fn err_snapshots() {
    insta::glob!("../test_data/parser/err", "*.emel", |path| {
        let src = std::fs::read_to_string(path).unwrap();
        insta::assert_snapshot!(parse(&src).debug_dump());
    });
}
use proptest::prelude::*;

proptest! {
    #[test]
    fn 任意の文字列で無損失(src in any::<String>()) {
        let parse = parse(&src);
        prop_assert_eq!(parse.syntax().text().to_string(), src);
    }

    /// Input biased toward characters that tend to form tokens.
    #[test]
    fn コードらしい文字列で無損失(src in r##"[a-zA-Z_0-9 \t\n(){}\[\],.:=|>+\-*/%!<&"#\\]{0,64}"##) {
        let parse = parse(&src);
        prop_assert_eq!(parse.syntax().text().to_string(), src);
    }

    /// Input rich in declaration keywords and separators, to exercise declarations and recovery.
    #[test]
    fn 宣言らしい文字列で無損失(
        src in r##"(fn |pub |opaque |suspend |type |enum |error |const |effect |handler |layer |trait |impl |import |derive |init |release |@external|@test|## d\n|[A-Z][a-z]+|[a-z]+|[A-Z]|[(){}\[\],.:=|+\->]| |\n){0,40}"##
    ) {
        let parse = parse(&src);
        prop_assert_eq!(parse.syntax().text().to_string(), src);
    }

    /// Input biased toward pieces of multiline strings, interpolations and escapes.
    #[test]
    fn 文字列らしい文字列で無損失(
        src in r##"("""|"|#\{|\}|\\|u\{|0x|0b|\n|  |=|\(|\)|fn |[a-z0-9]){0,40}"##
    ) {
        let parse = parse(&src);
        prop_assert_eq!(parse.syntax().text().to_string(), src);
    }
}
