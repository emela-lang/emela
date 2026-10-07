/// トークンとノードの種類．
///
/// トークンは仕様の 2章と 3章から機械的に起こしたもの．ノードの種類はパーサを書きながら
/// `ROOT` と `ERROR` の間に足していく．
#[allow(non_camel_case_types, clippy::manual_non_exhaustive)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum SyntaxKind {
    // 自明なトークン（2.1, 2.2）
    WHITESPACE,
    /// 文を終わらせうる改行．継続かどうか（2.5）はパーサの手前で判定する．
    NEWLINE,
    /// `#` から行末まで．
    COMMENT,
    /// `##` から行末まで．直後の宣言に付く．
    DOC_COMMENT,

    // リテラル（2.4）
    INT,
    FLOAT,
    /// 補間 `#{式}` をどう分割するかは字句解析器の設計で決める．
    STRING,

    // 名前（2.3）．文脈依存の語（3.2: implements for fails derive init release）は
    // 字句の段階では LOWER_NAME で，パーサが位置を見てキーワードとして扱う．
    /// 小文字で始まる snake_case．
    LOWER_NAME,
    /// 大文字で始まり，小文字を1つ以上含む PascalCase．
    TYPE_NAME,
    /// 大文字，数字，アンダースコアだけ．
    UPPER_NAME,
    /// `_`
    UNDERSCORE,

    // 予約語（3.1）
    FN_KW,
    TYPE_KW,
    ENUM_KW,
    ERROR_KW,
    CONST_KW,
    TRAIT_KW,
    IMPL_KW,
    EFFECT_KW,
    HANDLER_KW,
    LAYER_KW,
    IMPORT_KW,
    PUB_KW,
    OPAQUE_KW,
    SUSPEND_KW,
    IF_KW,
    ELSE_KW,
    MATCH_KW,
    FAIL_KW,
    ESCAPE_KW,
    USE_KW,
    WITH_KW,
    ASSERT_KW,
    SELF_KW,
    SELF_TYPE_KW,
    AS_KW,
    /// 将来のための予約語（3.3）．構文を持たないので1種類にまとめる．
    RESERVED_KW,

    // 記号
    L_PAREN,
    R_PAREN,
    L_BRACK,
    R_BRACK,
    L_BRACE,
    R_BRACE,
    COMMA,
    DOT,
    DOT2,
    COLON,
    EQ,
    THIN_ARROW,
    PIPE,
    PIPE_GT,
    PLUS,
    MINUS,
    STAR,
    SLASH,
    PERCENT,
    BANG,
    EQ2,
    NEQ,
    LT,
    LTEQ,
    GT,
    GTEQ,
    AMP2,
    PIPE2,
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
        matches!(self, Self::WHITESPACE | Self::COMMENT | Self::DOC_COMMENT)
    }
}
