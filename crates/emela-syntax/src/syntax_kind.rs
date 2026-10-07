/// トークンとノードの種類．
///
/// トークンは仕様の 2章と 3章から機械的に起こしたもの．ノードの種類はパーサを書きながら
/// `ROOT` と `ERROR` の間に足していく．
#[allow(non_camel_case_types, clippy::manual_non_exhaustive)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum SyntaxKind {
    // 自明なトークン（2.1, 2.2）
    /// スペースとタブの並び．
    WHITESPACE,
    /// 文を区切る改行（`\n` または `\r\n`）．
    NEWLINE,
    /// 前の行が続く改行（2.5）．字句解析の最後のパスが NEWLINE から付け替える．
    NEWLINE_CONT,
    /// `#` から行末まで．`# 前の行の続き`
    COMMENT,
    /// `##` から行末まで．直後の宣言に付く．`## メールアドレスを検査する`
    DOC_COMMENT,

    // リテラル（2.4）
    /// 整数．`42` `1_000_000`
    INT,
    /// 浮動小数点．`3.14` `1.0e9` `2.5E-3`
    FLOAT,
    // 文字列は部品に分け，組み立てはパーサが行う．
    // `"a #{x} b"` は STRING_QUOTE STRING_TEXT INTERP_START LOWER_NAME INTERP_END STRING_TEXT STRING_QUOTE
    /// 文字列を開く，または閉じる `"`．
    STRING_QUOTE,
    /// 文字列の本文．エスケープ（`\" \\ \n \#`）もこの中に含む．
    STRING_TEXT,
    /// 文字列の中の `#{`．
    INTERP_START,
    /// 補間を閉じる `}`．R_BRACE とは別にする．
    INTERP_END,

    // 名前（2.3）．文脈依存の語（3.2: implements for fails derive init release）は
    // 字句の段階では LOWER_NAME で，パーサが位置を見てキーワードとして扱う．
    /// 小文字名．小文字か `_` で始まる．`user_id` `find` `_tmp`
    LOWER_NAME,
    /// 型名．大文字で始まり，英字と数字だけで，小文字を1つ以上含む．`User` `Http` `True`
    TYPE_NAME,
    /// 大文字名．大文字で始まり，大文字，数字，`_` だけ．`MAX_SIZE` `A` `HTTP`
    UPPER_NAME,
    /// `_`．束縛しないパターン．
    UNDERSCORE,

    // 予約語（3.1）
    /// `fn`
    FN_KW,
    /// `type`
    TYPE_KW,
    /// `enum`
    ENUM_KW,
    /// `error`
    ERROR_KW,
    /// `const`
    CONST_KW,
    /// `trait`
    TRAIT_KW,
    /// `impl`
    IMPL_KW,
    /// `effect`
    EFFECT_KW,
    /// `handler`
    HANDLER_KW,
    /// `layer`
    LAYER_KW,
    /// `import`
    IMPORT_KW,
    /// `pub`
    PUB_KW,
    /// `opaque`
    OPAQUE_KW,
    /// `suspend`
    SUSPEND_KW,
    /// `if`
    IF_KW,
    /// `else`
    ELSE_KW,
    /// `match`
    MATCH_KW,
    /// `fail`
    FAIL_KW,
    /// `escape`
    ESCAPE_KW,
    /// `use`
    USE_KW,
    /// `with`
    WITH_KW,
    /// `assert`
    ASSERT_KW,
    /// `self`
    SELF_KW,
    /// `Self`
    SELF_TYPE_KW,
    /// `as`
    AS_KW,
    /// 将来のための予約語（3.3）．構文を持たないので1種類にまとめる．
    ///
    /// `var` `return` `while` `loop` `let` `mut` `in` `where` `do` `try` `catch` `throw`
    /// `async` `await` `yield` `defer` `macro` `mod`
    RESERVED_KW,

    // 記号
    /// `(`
    L_PAREN,
    /// `)`
    R_PAREN,
    /// `[`
    L_BRACK,
    /// `]`
    R_BRACK,
    /// `{`
    L_BRACE,
    /// `}`．補間を閉じる `}` は INTERP_END になる．
    R_BRACE,
    /// `,`
    COMMA,
    /// `.`
    DOT,
    /// `..`
    DOT2,
    /// `:`
    COLON,
    /// `=`
    EQ,
    /// `->`
    THIN_ARROW,
    /// `|`
    PIPE,
    /// `|>`
    PIPE_GT,
    /// `+`
    PLUS,
    /// `-`
    MINUS,
    /// `*`
    STAR,
    /// `/`
    SLASH,
    /// `%`
    PERCENT,
    /// `!`
    BANG,
    /// `==`
    EQ2,
    /// `!=`
    NEQ,
    /// `<`
    LT,
    /// `<=`
    LTEQ,
    /// `>`
    GT,
    /// `>=`
    GTEQ,
    /// `&&`
    AMP2,
    /// `||`
    PIPE2,
    /// `@`
    AT,

    /// 字句解析で認識できなかった文字．
    ERROR_TOKEN,

    // ノード
    ROOT,
    /// パーサがエラー回復で読み飛ばした範囲．
    ERROR,

    #[doc(hidden)]
    __LAST,
}

impl SyntaxKind {
    pub fn from_raw(raw: u16) -> Self {
        assert!(raw < Self::__LAST as u16, "SyntaxKind の範囲外: {raw}");
        // SAFETY: repr(u16) で，0 から __LAST まで隙間なく並んでいる．
        unsafe { std::mem::transmute::<u16, SyntaxKind>(raw) }
    }

    pub fn is_trivia(self) -> bool {
        matches!(
            self,
            Self::WHITESPACE | Self::NEWLINE_CONT | Self::COMMENT | Self::DOC_COMMENT
        )
    }
}
