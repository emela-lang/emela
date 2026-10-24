/// Token and node kinds.
///
/// Tokens are transcribed mechanically from spec chapters 2 and 3. Node kinds are added between
/// `ROOT` and `ERROR` as the parser grows.
#[allow(non_camel_case_types, clippy::manual_non_exhaustive)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum SyntaxKind {
    // Trivial tokens (spec 2.1, 2.2)
    /// Spaces and tabs.
    WHITESPACE,
    /// A statement-separating newline (`\n` or `\r\n`).
    NEWLINE,
    /// A newline that continues the previous line (spec 2.5). The lexer's last pass retags a
    /// NEWLINE as this.
    NEWLINE_CONT,
    /// `#` to the end of the line.
    COMMENT,
    /// `##` to the end of the line. Attaches to the following declaration.
    DOC_COMMENT,

    // Literals (spec 2.4)
    /// `42` `1_000_000`
    INT,
    /// `3.14` `1.0e9` `2.5E-3`
    FLOAT,
    // Strings are split into pieces and the parser assembles them.
    // `"a #{x} b"` is STRING_QUOTE STRING_TEXT INTERP_START LOWER_NAME INTERP_END STRING_TEXT STRING_QUOTE
    /// The `"` that opens or closes a string.
    STRING_QUOTE,
    /// The `"""` that opens or closes a multiline string. The closing one includes the
    /// whitespace from the start of its line, which is the indentation stripped from every line.
    /// `  """`
    TRIPLE_QUOTE,
    /// The body of a string as written, escapes (`\" \\ \n \t \r \# \u{...}`) included.
    STRING_TEXT,
    /// `#{` inside a string.
    INTERP_START,
    /// The `}` that closes an interpolation, kept apart from R_BRACE.
    INTERP_END,

    // Names (spec 2.3). Contextual words (spec 3.2: fails derive init release)
    // lex as LOWER_NAME; the parser treats them as keywords by position.
    /// Starts with a lowercase letter or `_`. `user_id` `find` `_tmp`
    LOWER_NAME,
    /// Starts with an uppercase letter, has only letters and digits, and at least one lowercase
    /// letter. `User` `Http` `True`
    TYPE_NAME,
    /// Starts with an uppercase letter and has only uppercase letters, digits and `_`.
    /// `MAX_SIZE` `A` `HTTP`
    UPPER_NAME,
    /// `_`, a pattern that binds nothing.
    UNDERSCORE,

    // Reserved words (spec 3.1)
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
    /// Words reserved for the future (spec 3.3). They have no syntax, so they share one kind.
    ///
    /// `var` `return` `while` `loop` `let` `mut` `in` `where` `do` `try` `catch` `throw`
    /// `async` `yield` `defer` `macro` `mod`
    RESERVED_KW,

    // Punctuation
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
    /// `}`. The `}` that closes an interpolation is INTERP_END instead.
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

    /// A character the lexer did not recognize.
    ERROR_TOKEN,
    /// The end of the input. Used only for parser lookahead; never appears in the tree.
    EOF,

    // Nodes
    ROOT,

    // Declarations (spec 17.3)
    /// `import Data.Json.{Json, decode}`
    IMPORT,
    /// `.{Json, decode}`
    IMPORT_LIST,
    /// `@external(js, "Math.sin")`
    ANNOTATION,
    /// `(js, "Math.sin")`
    ANNOT_ARG_LIST,
    /// `js` `"Math.sin"` `name: "x"`
    ANNOT_ARG,
    /// `Http.serve`, the argument of `@entry` (spec 17.3 `qualified`)
    ANNOT_PATH,
    /// `pub fn find(id: Int) -> User fails NotFound use Users { ... }`
    FN_DECL,
    /// `type User(id: Int, name: String) derive Eq`
    TYPE_DECL,
    /// `error NotFound(id: Int)`
    ERROR_DECL,
    /// `enum Shape { Circle(radius: Float)⏎ Empty }`
    ENUM_DECL,
    /// `(id: Int, name: String)`
    FIELD_LIST,
    /// `@json("n") name: String`
    FIELD,
    /// `{ Circle(radius: Float), Empty }`
    VARIANT_LIST,
    /// `Circle(radius: Float)` `Rect(Float, Float)` `Empty`
    VARIANT,
    /// Unnamed fields. `(Float, Float)`
    TUPLE_FIELD_LIST,
    /// `derive Eq, Show`
    DERIVE_CLAUSE,
    /// `const MAX_RETRY: Int = 3`
    CONST_DECL,
    /// `effect Clock { suspend fn sleep(ms: Int) -> () }`
    EFFECT_DECL,
    /// An effect operation. `suspend fn sleep(ms: Int) -> ()`
    OP_SIG,
    /// The base of a label (an effect declared `of` another). `of Console default`
    EFFECT_LABEL,
    /// `handler PostgresUsers(conn: Connection): Users { ... }`
    ///
    /// The `:` is a direct child, as in `trait Ord: Eq`, so the formatter puts no space before it.
    HANDLER_DECL,
    /// The effect a handler implements, after `:`. `Users`, `State[Int]`
    HANDLER_EFFECT,
    /// `init { ... }`
    HANDLER_INIT,
    /// `release { ... }`
    HANDLER_RELEASE,
    /// `layer AppLive { EnvConfig⏎ ConsoleLogger }`
    LAYER_DECL,
    /// `trait Ord: Eq { ... }`
    TRAIT_DECL,
    /// `impl User: Show { ... }`
    IMPL_DECL,
    /// The type of an impl with its type parameters, before `:`. `User`, `Map[K: Eq, V: Eq]`
    IMPL_TYPE,
    /// The `{ ... }` body of an effect, handler, layer, trait or impl.
    ITEM_LIST,
    /// `[A, R: Immediate]`
    TYPE_PARAM_LIST,
    /// `R: Immediate + Show`
    TYPE_PARAM,

    // Types (spec 17.4)
    /// A type reference. `Http.Client` `User`
    PATH,
    /// A named type with type arguments. `List[Int]`
    PATH_TYPE,
    /// `[Int, String]`
    TYPE_ARG_LIST,
    /// An uppercase name referring to a type parameter. `A`
    TYPE_VAR,
    /// `Self`
    SELF_TYPE,
    /// `()`
    UNIT_TYPE,
    /// `(Int, String)`
    TUPLE_TYPE,
    /// `(fn(Int) -> String)`
    PAREN_TYPE,
    /// `fn(A) -> B fails E use R`
    FN_TYPE,
    /// The parameters of a function type. `(A, B)`
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

    // Literals (spec 2.4)
    /// A numeric literal. `42` `3.14`
    LITERAL,
    /// `"hello #{name}"`
    STRING,
    /// `#{name}`
    INTERP,

    // Patterns (spec 17.6)
    /// `_`
    WILDCARD_PAT,
    /// Binds a name. `x`
    IDENT_PAT,
    /// `0` `"abc"`
    LITERAL_PAT,
    /// `MAX_SIZE`
    CONST_PAT,
    /// A variant or record. `Circle(radius:)` `Empty`
    VARIANT_PAT,
    /// `(radius:, ..)`
    PAT_ARG_LIST,
    /// A named argument. `radius:` `name: n`
    FIELD_PAT,
    /// `()`
    UNIT_PAT,
    /// `(a, b)`
    TUPLE_PAT,
    /// `[]` `[x, ..rest]`
    LIST_PAT,
    /// `..` `..rest`
    REST_PAT,

    // Expressions (spec 17.5)
    /// A lowercase name, uppercase name or `self`. `count` `MAX_SIZE` `self`
    NAME_REF,
    /// A reference starting with a type name. `List` `Http.Client`
    PATH_EXPR,
    /// `_`. Accepted as an expression so that it can be read as the left side of a binding.
    UNDERSCORE_EXPR,
    /// `()`
    UNIT_EXPR,
    /// `(a + b)`
    PAREN_EXPR,
    /// `(a, b)`
    TUPLE_EXPR,
    /// `[x, y, ..rest]`
    LIST_EXPR,
    /// `..rest` `..List.tail(xs)`. A bare `..` is also accepted so that it can be read as the
    /// left side of a binding.
    REST_EXPR,
    /// `f(x, port: 5432)`
    CALL_EXPR,
    /// `(x, port: 5432)`
    ARG_LIST,
    /// `port: 5432` `input:`
    NAMED_ARG,
    /// A trailing `..expr` in an argument list: a partial update of a constructor (spec 5.3).
    /// `User(name: "b", ..user)`
    ///
    /// A bare `..` is also accepted so that it can be read as the left side of a binding.
    /// `User(name:, ..) = user`
    ///
    /// Kept apart from a list's REST_EXPR. Whether the callee is a constructor is not checked by
    /// the parser.
    SPREAD_ARG,
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
    /// The left side is parsed as an expression and checked as a pattern on the tree.
    /// `(a, b): (Int, Int) = pair`
    BINDING,
    /// `if c { a } else { b }`
    IF_EXPR,
    /// `x match { ... }`
    ///
    /// Children: the scrutinee, `match`, MATCH_ARM_LIST. The scrutinee is still the first
    /// expression child, as in the old prefix form, so nothing after the parser had to change.
    MATCH_EXPR,
    /// The arms of `match` and `escape`. `{ A -> 1⏎ B -> 2 }`
    MATCH_ARM_LIST,
    /// `Circle(r) if r > 0.0 -> r`
    MATCH_ARM,
    /// `if r > 0.0`
    MATCH_GUARD,
    /// `with ConsoleLogger, FixedClock { ... }`
    WITH_EXPR,
    /// An item of `with` that plugs a provider into a label. `AuditLog by HandlerA`
    WITH_BY,
    /// A handler with the arguments of its init, as an item of `with` or `layer` or the right
    /// of `by`. `WorkerKv(binding: "KV")`. Without arguments, the item is a bare PATH.
    SUPPLY,
    /// `find(id) escape { NotFound(_) -> guest() }`
    ESCAPE_EXPR,
    /// `fn(n) { n + 1 }`
    LAMBDA_EXPR,
    /// `(x: Int, f)`
    PARAM_LIST,
    /// `x: Int` `self`
    PARAM,

    /// A range the parser skipped during error recovery.
    ERROR,

    #[doc(hidden)]
    __LAST,
}

impl SyntaxKind {
    pub fn from_raw(raw: u16) -> Self {
        assert!(raw < Self::__LAST as u16, "SyntaxKind out of range: {raw}");
        // SAFETY: repr(u16), with no gaps from 0 to __LAST.
        unsafe { std::mem::transmute::<u16, SyntaxKind>(raw) }
    }

    pub fn is_trivia(self) -> bool {
        matches!(
            self,
            Self::WHITESPACE | Self::NEWLINE_CONT | Self::COMMENT | Self::DOC_COMMENT
        )
    }

    /// The name shown in diagnostics: `` `(` `` for punctuation and reserved words, the kind's
    /// name otherwise.
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
            TRIPLE_QUOTE => "`\"\"\"`",
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

    /// Whether the node absorbs the preceding doc comment `##`.
    pub fn takes_doc(self) -> bool {
        matches!(
            self,
            Self::FN_DECL
                | Self::TYPE_DECL
                | Self::ERROR_DECL
                | Self::ENUM_DECL
                | Self::CONST_DECL
                | Self::EFFECT_DECL
                | Self::OP_SIG
                | Self::HANDLER_DECL
                | Self::HANDLER_INIT
                | Self::HANDLER_RELEASE
                | Self::LAYER_DECL
                | Self::TRAIT_DECL
                | Self::IMPL_DECL
                | Self::FIELD
                | Self::VARIANT
                | Self::DERIVE_CLAUSE
        )
    }
}
