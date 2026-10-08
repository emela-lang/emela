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
    /// 入力の終わり．パーサの先読みだけで使い，木には出ない．
    EOF,

    // ノード
    ROOT,

    // 宣言（17.3）
    /// `pub fn find(id: Int) -> User fails NotFound use Users { ... }`
    FN_DECL,
    /// `[A, R: Immediate]`
    TYPE_PARAM_LIST,
    /// `R: Immediate + Show`
    TYPE_PARAM,

    // 型（17.4）
    /// 型の参照．`Http.Client` `User`
    PATH,
    /// 名前の付いた型と型引数．`List[Int]`
    PATH_TYPE,
    /// `[Int, String]`
    TYPE_ARG_LIST,
    /// 型引数を指す大文字名．`A`
    TYPE_VAR,
    /// `Self`
    SELF_TYPE,
    /// `()`
    UNIT_TYPE,
    /// `(Int, String)`
    TUPLE_TYPE,
    /// `fn(A) -> B fails E use R`
    FN_TYPE,
    /// 関数型の引数の並び．`(A, B)`
    PARAM_TYPE_LIST,
    /// `-> T`
    RET_TYPE,
    /// `fails E`
    FAILS_CLAUSE,
    /// `{}` `NotFound | DbError` `E`
    ERROR_SET,
    /// `use R`
    USE_CLAUSE,
    /// `{ Io, Clock }` `Users` `R`
    EFFECT_SET,

    // リテラル（2.4）
    /// 数値のリテラル．`42` `3.14`
    LITERAL,
    /// 文字列のリテラル．`"hello #{name}"`
    STRING,
    /// 文字列の中の補間．`#{name}`
    INTERP,

    // パターン（17.6）
    /// `_`
    WILDCARD_PAT,
    /// 名前を束縛する．`x`
    IDENT_PAT,
    /// `0` `"abc"`
    LITERAL_PAT,
    /// 定数．`MAX_SIZE`
    CONST_PAT,
    /// バリアントやレコード．`Circle(radius:)` `Empty`
    VARIANT_PAT,
    /// `(radius:, ..)`
    PAT_ARG_LIST,
    /// 名前付きの引数．`radius:` `name: n`
    FIELD_PAT,
    /// `()`
    UNIT_PAT,
    /// `(a, b)`
    TUPLE_PAT,
    /// `[]` `[x, ..rest]`
    LIST_PAT,
    /// 残り．`..` `..rest`
    REST_PAT,

    // 式（17.5）
    /// 小文字名，大文字名，`self` の参照．`count` `MAX_SIZE` `self`
    NAME_REF,
    /// 型名で始まる参照．`List` `Http.Client`
    PATH_EXPR,
    /// `_`．束縛の左辺として読むために式でも受け付ける．
    UNDERSCORE_EXPR,
    /// `()`
    UNIT_EXPR,
    /// `(a + b)`
    PAREN_EXPR,
    /// `(a, b)`
    TUPLE_EXPR,
    /// `[x, y, ..rest]`
    LIST_EXPR,
    /// 残り．`..rest` `..List.tail(xs)`．束縛の左辺として読むために `..` だけも受け付ける．
    REST_EXPR,
    /// `f(x, port: 5432)`
    CALL_EXPR,
    /// `(x, port: 5432)`
    ARG_LIST,
    /// 名前付き引数．`port: 5432` `input:`
    NAMED_ARG,
    /// `user.name`
    FIELD_EXPR,
    /// `!valid` `-x`
    PREFIX_EXPR,
    /// `a + b` `xs |> f`
    BIN_EXPR,
    /// `use Logger`
    USE_EXPR,
    /// `fail NotFound(id:)`
    FAIL_EXPR,
    /// `assert x == 1`
    ASSERT_EXPR,
    /// `{ a = 1⏎ a + 1 }`
    BLOCK_EXPR,
    /// 束縛．左辺は式として読み，パターンとしての検査は木の上で行う．`(a, b): (Int, Int) = pair`
    BINDING,
    /// `if c { a } else { b }`
    IF_EXPR,
    /// `match x { ... }`
    MATCH_EXPR,
    /// match と escape の腕の並び．`{ A -> 1⏎ B -> 2 }`
    MATCH_ARM_LIST,
    /// `Circle(r) if r > 0.0 -> r`
    MATCH_ARM,
    /// `if r > 0.0`
    MATCH_GUARD,
    /// `with ConsoleLogger, FixedClock { ... }`
    WITH_EXPR,
    /// `find(id) escape { NotFound(_) -> guest() }`
    ESCAPE_EXPR,
    /// `fn(n) { n + 1 }`
    LAMBDA_EXPR,
    /// `(x: Int, f)`
    PARAM_LIST,
    /// `x: Int` `self`
    PARAM,

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

    /// 診断の文面に出す名前．記号と予約語は `` `(` `` の形，それ以外は種類の名前にする．
    pub fn describe(self) -> &'static str {
        use SyntaxKind::*;
        match self {
            WHITESPACE => "whitespace",
            NEWLINE | NEWLINE_CONT => "newline",
            COMMENT => "comment",
            DOC_COMMENT => "doc comment",
            INT => "integer literal",
            FLOAT => "float literal",
            STRING_QUOTE => "`\"`",
            STRING_TEXT => "string text",
            INTERP_START => "`#{`",
            INTERP_END => "`}`",
            LOWER_NAME => "lower name",
            TYPE_NAME => "type name",
            UPPER_NAME => "upper name",
            UNDERSCORE => "`_`",
            FN_KW => "`fn`",
            TYPE_KW => "`type`",
            ENUM_KW => "`enum`",
            ERROR_KW => "`error`",
            CONST_KW => "`const`",
            TRAIT_KW => "`trait`",
            IMPL_KW => "`impl`",
            EFFECT_KW => "`effect`",
            HANDLER_KW => "`handler`",
            LAYER_KW => "`layer`",
            IMPORT_KW => "`import`",
            PUB_KW => "`pub`",
            OPAQUE_KW => "`opaque`",
            SUSPEND_KW => "`suspend`",
            IF_KW => "`if`",
            ELSE_KW => "`else`",
            MATCH_KW => "`match`",
            FAIL_KW => "`fail`",
            ESCAPE_KW => "`escape`",
            USE_KW => "`use`",
            WITH_KW => "`with`",
            ASSERT_KW => "`assert`",
            SELF_KW => "`self`",
            SELF_TYPE_KW => "`Self`",
            AS_KW => "`as`",
            RESERVED_KW => "reserved word",
            L_PAREN => "`(`",
            R_PAREN => "`)`",
            L_BRACK => "`[`",
            R_BRACK => "`]`",
            L_BRACE => "`{`",
            R_BRACE => "`}`",
            COMMA => "`,`",
            DOT => "`.`",
            DOT2 => "`..`",
            COLON => "`:`",
            EQ => "`=`",
            THIN_ARROW => "`->`",
            PIPE => "`|`",
            PIPE_GT => "`|>`",
            PLUS => "`+`",
            MINUS => "`-`",
            STAR => "`*`",
            SLASH => "`/`",
            PERCENT => "`%`",
            BANG => "`!`",
            EQ2 => "`==`",
            NEQ => "`!=`",
            LT => "`<`",
            LTEQ => "`<=`",
            GT => "`>`",
            GTEQ => "`>=`",
            AMP2 => "`&&`",
            PIPE2 => "`||`",
            AT => "`@`",
            ERROR_TOKEN => "invalid token",
            EOF => "end of file",
            _ => "syntax node",
        }
    }
}
