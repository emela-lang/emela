//! パーサの性質のテスト．スナップショットは文法を書き始めてから足す．

use emela_syntax::parse;
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
}
