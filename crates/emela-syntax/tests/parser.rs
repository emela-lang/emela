//! パーサのテスト．`test_data/parser/{ok,err}/*.emel` の木と診断をスナップショットで比べる．
//! ok は診断が0件であることも確かめる．

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
    /// どんな入力でも panic せず，木のテキストが入力と一致する．
    #[test]
    fn 任意の文字列で無損失(src in any::<String>()) {
        let parse = parse(&src);
        prop_assert_eq!(parse.syntax().text().to_string(), src);
    }

    /// 字句の部品になりやすい文字に偏らせた入力でも同じことが成り立つ．
    #[test]
    fn コードらしい文字列で無損失(src in r##"[a-zA-Z_0-9 \t\n(){}\[\],.:=|>+\-*/%!<&"#\\]{0,64}"##) {
        let parse = parse(&src);
        prop_assert_eq!(parse.syntax().text().to_string(), src);
    }

    /// 宣言のキーワードと区切りを多めに混ぜた入力．宣言の読み方と回復を通す．
    #[test]
    fn 宣言らしい文字列で無損失(
        src in r##"(fn |pub |opaque |suspend |type |enum |error |const |effect |handler |layer |trait |impl |import |implements |for |derive |init |release |@external|@test|## d\n|[A-Z][a-z]+|[a-z]+|[A-Z]|[(){}\[\],.:=|+\->]| |\n){0,40}"##
    ) {
        let parse = parse(&src);
        prop_assert_eq!(parse.syntax().text().to_string(), src);
    }
}
