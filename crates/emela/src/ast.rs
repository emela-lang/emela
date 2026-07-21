use crate::error::Span;

// The type-system types are part of the IR contract and live in
// `emela-codegen`; the frontend AST re-uses them.
pub(crate) use emela_codegen::{BinaryOp, EffectRow, FunctionType, Type};

#[derive(Debug, Clone)]
pub(crate) struct Program {
    pub(crate) module: Option<String>,
    pub(crate) imports: Vec<Import>,
    pub(crate) functions: Vec<Function>,
    pub(crate) externs: Vec<Extern>,
    pub(crate) enums: Vec<EnumDecl>,
    /// `record` declarations (spec 0006).
    pub(crate) records: Vec<RecordDecl>,
    /// `trait` declarations (spec 0020).
    pub(crate) traits: Vec<TraitDecl>,
    /// `impl Trait for Type` blocks (spec 0020).
    pub(crate) impls: Vec<ImplDecl>,
    /// `effect` declarations (spec 0037). The operations themselves are
    /// desugared into `functions`/`externs`; this records the effect names in
    /// scope, for validating `uses { ... }` rows and `Name.op(...)` heads.
    pub(crate) effects: Vec<EffectDecl>,
}

/// An `effect Name { ... }` declaration (spec 0037): a capitalized, first-class
/// effect that owns a set of operations. An item within a module, like an enum.
#[derive(Debug, Clone)]
pub(crate) struct EffectDecl {
    pub(crate) name: String,
    #[allow(dead_code)]
    pub(crate) name_span: Span,
}

/// A `record` declaration (spec 0006): named fields, no variants. Records are
/// non-generic in this first cut (0006 defines declaration, literal
/// construction, and field access only).
#[derive(Debug, Clone)]
pub(crate) struct RecordDecl {
    pub(crate) name: String,
    pub(crate) name_span: Span,
    /// The declaring file's module path, mirroring `EnumDecl::module`.
    #[allow(dead_code)]
    pub(crate) module: Option<String>,
    pub(crate) fields: Vec<RecordFieldDef>,
}

/// One field of a `record` declaration (spec 0006).
#[derive(Debug, Clone)]
pub(crate) struct RecordFieldDef {
    pub(crate) name: String,
    pub(crate) name_span: Span,
    pub(crate) ty: Type,
}

/// An `enum` declaration (spec 0005).
#[derive(Debug, Clone)]
pub(crate) struct EnumDecl {
    pub(crate) name: String,
    pub(crate) name_span: Span,
    /// The type parameters of a generic enum (spec 0028), e.g. `T` in
    /// `enum List<T>`. Empty for a non-generic enum. They are in scope as types
    /// (`Type::Var`) within the variant payload types.
    pub(crate) type_params: Vec<String>,
    /// The declaring file's module path (spec 0020 orphan rule): a `trait` may be
    /// implemented for this type only in this module or the trait's module.
    pub(crate) module: Option<String>,
    pub(crate) variants: Vec<EnumVariant>,
    /// The lang-item role this enum is bound to via `@lang("<role>")` (specs
    /// 0039/0041), e.g. `Some("option")` for the `Option` type. `None` for an
    /// ordinary enum. Set by `apply_attributes`; consumed where the compiler
    /// resolves lang-item roles (spec 0042 builds the `option` table).
    pub(crate) lang_item: Option<String>,
}

/// A `trait` declaration (spec 0020): a named set of method signatures a type
/// may implement. `Self` inside the trait is `Type::Var("Self")`.
#[derive(Debug, Clone)]
pub(crate) struct TraitDecl {
    pub(crate) name: String,
    pub(crate) name_span: Span,
    /// The declaring file's module path (orphan rule).
    pub(crate) module: Option<String>,
    pub(crate) methods: Vec<TraitMethodSig>,
}

/// One method signature of a trait (spec 0020). `default_body` is `Some` for a
/// method with a default implementation, which an `impl` may omit.
#[derive(Debug, Clone)]
pub(crate) struct TraitMethodSig {
    pub(crate) name: String,
    pub(crate) name_span: Span,
    pub(crate) params: Vec<Param>,
    pub(crate) ret: Type,
    pub(crate) throws: Option<Type>,
    pub(crate) effects: EffectRow,
    pub(crate) default_body: Option<Block>,
}

/// An `impl Trait for Type { ... }` block (spec 0020). It asserts that `target`
/// satisfies `trait_name` and supplies the method bodies. `type_params`/`bounds`
/// are the impl's own parameters for a parameterized instance
/// (`impl<T: Show> Show for Array<T>`). Inside, `Self` and the impl's parameters
/// appear as `Type::Var`.
#[derive(Debug, Clone)]
pub(crate) struct ImplDecl {
    pub(crate) trait_name: String,
    pub(crate) trait_span: Span,
    pub(crate) target: Type,
    pub(crate) target_span: Span,
    pub(crate) type_params: Vec<String>,
    pub(crate) bounds: Vec<Bound>,
    /// The declaring file's module path (orphan rule).
    pub(crate) module: Option<String>,
    pub(crate) methods: Vec<Function>,
}

/// A bound on a type parameter (spec 0020): `T: Add + Show` becomes
/// `Bound { param: "T", traits: ["Add", "Show"] }`.
#[derive(Debug, Clone)]
pub(crate) struct Bound {
    pub(crate) param: String,
    pub(crate) traits: Vec<String>,
    pub(crate) span: Span,
}

/// One variant of an enum, with its payload field types (possibly empty).
#[derive(Debug, Clone)]
pub(crate) struct EnumVariant {
    pub(crate) name: String,
    pub(crate) name_span: Span,
    pub(crate) fields: Vec<Type>,
}

/// A no-body function declaration whose implementation the compiler does not
/// hold. Two kinds share this shape. An `extern fn` (spec 0013) is a platform
/// function: the Runtime supplies the implementation and the call lowers to a
/// Platform node. An `intrinsic fn` (spec 0021) is a pure primitive: the backend
/// inlines it to a native instruction and the call lowers to an Intrinsic node.
/// `module` is the declaring file's module path, used with `name` to form the
/// canonical platform name (externs only).
#[derive(Debug, Clone)]
pub(crate) struct Extern {
    pub(crate) name: String,
    pub(crate) name_span: Span,
    pub(crate) module: Option<String>,
    /// Type parameters of a generic `intrinsic fn` (spec 0021), e.g. `T` in
    /// `array_get<T>(a: Array<T>, i: Int) -> T`. Their names appear as
    /// `Type::Var` in the signature and are monomorphized at each call site,
    /// like a generic function (spec 0014). Empty for a non-generic
    /// intrinsic/extern; `extern fn` (platform functions) are never generic.
    pub(crate) type_params: Vec<String>,
    /// Trait bounds on the type parameters (spec 0020). Always empty in v1: an
    /// intrinsic is pure and its backend lowering is uniform in the element
    /// type, so it needs no bounds. Carried for shape parity with `Function`.
    pub(crate) bounds: Vec<Bound>,
    pub(crate) params: Vec<Param>,
    pub(crate) ret: Type,
    pub(crate) throws: Option<Type>,
    pub(crate) effects: EffectRow,
    /// `true` for `intrinsic fn` (spec 0021), `false` for `extern fn` (spec 0013).
    pub(crate) is_intrinsic: bool,
    /// `Some(effect)` when this extern is a backing operation of an `effect`
    /// block (spec 0037), e.g. `write_stdout` inside `effect Io { ... }`. A
    /// backing operation is private to its effect: only sibling operations may
    /// call it, by bare name.
    pub(crate) effect_name: Option<String>,
}

impl Extern {
    /// The canonical platform name, e.g. `io.write_stdout`.
    pub(crate) fn canonical(&self) -> String {
        match &self.module {
            Some(module) if !module.is_empty() => format!("{module}.{}", self.name),
            _ => self.name.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Import {
    pub(crate) path: Vec<String>,
    pub(crate) span: Span,
}

impl Import {
    pub(crate) fn item_name(&self) -> &str {
        self.path.last().map(String::as_str).unwrap_or("")
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Function {
    pub(crate) name: String,
    pub(crate) name_span: Span,
    pub(crate) is_public: bool,
    /// The qualifier this function is addressed by (spec 0037). For a function
    /// of an imported module this is the written import path (`["std", "list"]`
    /// for `import std.list`), stamped by `imports.rs` on every function of the
    /// module (public or not) so bare names never cross module boundaries. For
    /// an effect operation it is `[EffectName]`, stamped at parse time, so the
    /// operation is addressed as `Io.print` wherever the effect is declared.
    /// Empty only for the compilation root's own functions. The full qualified
    /// path is `module_path + [name]`; public functions are callable by any
    /// suffix of that path ending at `name`.
    pub(crate) module_path: Vec<String>,
    /// The declaring file's `module` header, if any (spec 0037): the visibility
    /// domain for bare `extern`/`intrinsic` references, matching
    /// `Extern::module`. `None` for a headerless compilation root.
    pub(crate) declared_module: Option<String>,
    /// `Some(effect)` when this function is an operation of an `effect` block
    /// (spec 0037), e.g. `print` inside `effect Io { ... }`. Operations carry
    /// the effect implicitly in `effects` and are callable only as
    /// `Io.print(...)` from inside a `uses { Io }` scope; sibling operations of
    /// the same effect may also call each other by bare name.
    pub(crate) effect_name: Option<String>,
    /// Declared type parameters (spec 0014), e.g. `["T", "U"]`. Empty for a
    /// non-generic function. Their names appear as `Type::Var` in this
    /// function's signature and body.
    pub(crate) type_params: Vec<String>,
    /// Trait bounds on the type parameters (spec 0020), e.g. `<T: Add>`. Empty
    /// when no parameter is bounded. Consulted only when resolving bare trait
    /// method calls in the body; the monomorphization machinery needs only the
    /// parameter names in `type_params`.
    pub(crate) bounds: Vec<Bound>,
    pub(crate) params: Vec<Param>,
    pub(crate) ret: Type,
    pub(crate) throws: Option<Type>,
    pub(crate) effects: EffectRow,
    pub(crate) body: Block,
    /// `true` when the function is declared `@test` (specs 0039/0040). A test
    /// function is excluded from normal build artifacts (T8), may not be
    /// referenced by any source code (T5), and its body is an implicit-try
    /// scope (T3): bare throwing calls propagate to the test harness.
    pub(crate) is_test: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Param {
    pub(crate) name: String,
    pub(crate) name_span: Span,
    pub(crate) ty: Type,
}

#[derive(Debug, Clone)]
pub(crate) struct Block {
    pub(crate) items: Vec<BlockItem>,
    pub(crate) span: Span,
}

#[derive(Debug, Clone)]
pub(crate) enum BlockItem {
    Let {
        name: String,
        name_span: Span,
        ty: Option<Type>,
        value: Expr,
    },
    Expr(Expr),
}

#[derive(Debug, Clone)]
pub(crate) enum Expr {
    Int(i32, Span),
    Float(f64, Span),
    Bool(bool, Span),
    String(String, Span),
    Char(char, Span),
    Array(Vec<Expr>, Span),
    Unit(Span),
    Var(String, Span),
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        span: Span,
    },
    Fn {
        params: Vec<Param>,
        ret: Type,
        throws: Option<Type>,
        effects: EffectRow,
        body: Block,
        span: Span,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },
    Block(Block),
    /// `if cond { then } else { els }` (spec 0015).
    If {
        cond: Box<Expr>,
        then: Block,
        els: Block,
        span: Span,
    },
    /// `throw e` (spec 0011).
    Throw {
        value: Box<Expr>,
        span: Span,
    },
    /// `panic(msg)` (spec 0011).
    Panic {
        message: Box<Expr>,
        span: Span,
    },
    /// `expr?` (spec 0011): error / `None` propagation.
    Question {
        value: Box<Expr>,
        span: Span,
    },
    /// `match scrutinee { arms }` (spec 0005).
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
        span: Span,
    },
    /// `try { body } catch { arms }` (spec 0011).
    Try {
        body: Block,
        arms: Vec<MatchArm>,
        span: Span,
    },
    /// A dotted (`.`) path used as a value or call target (spec 0018):
    /// `int.to_string`, `std.int.to_string`, a receiver call `recv.method`, or a
    /// qualified trait method `Show.to_string`. Has at least two segments (a
    /// single identifier is `Var`). Its meaning — receiver call, qualified trait
    /// method, or a (possibly qualified) function — is resolved in the type
    /// checker / lowering. Enum variants and built-in conversions are `::` type
    /// paths (`TypePath`), never `.` paths.
    Path {
        segments: Vec<String>,
        span: Span,
    },
    /// A record literal `Name { field: expr ... }` (spec 0006). Field order is
    /// as written; the type checker validates the set against the declaration.
    RecordLiteral {
        name: String,
        name_span: Span,
        fields: Vec<(String, Span, Expr)>,
        span: Span,
    },
    /// A postfix field access on a non-path expression (spec 0006), e.g.
    /// `get(url).body`. Field access on a dotted identifier chain
    /// (`user.name`) parses as `Path` and is resolved in the checker.
    Field {
        target: Box<Expr>,
        name: String,
        name_span: Span,
        span: Span,
    },
    /// A `::` type path (specs 0005/0018 R7): a name resolved through an enum
    /// type — an enum variant (`Color::Red`, `Either::Left`). The head is a type
    /// name. Has at least two segments. When followed by `(...)` it is the callee
    /// of a `Call`; on its own it is a value (a no-payload variant). Kept
    /// distinct from `Path` so `.` never resolves to a variant (`Color.Red` is an
    /// error). The former `Char::from_code` / `String::from_char` conversions are
    /// now bare intrinsics (spec 0021), not type paths.
    TypePath {
        segments: Vec<String>,
        span: Span,
    },
}

impl Expr {
    pub(crate) fn span(&self) -> Span {
        match self {
            Expr::Int(_, span)
            | Expr::Float(_, span)
            | Expr::Bool(_, span)
            | Expr::String(_, span)
            | Expr::Char(_, span)
            | Expr::Array(_, span)
            | Expr::Unit(span)
            | Expr::Var(_, span) => span.clone(),
            Expr::Call { span, .. }
            | Expr::Fn { span, .. }
            | Expr::Binary { span, .. }
            | Expr::Throw { span, .. }
            | Expr::Panic { span, .. }
            | Expr::Question { span, .. }
            | Expr::Match { span, .. }
            | Expr::Try { span, .. }
            | Expr::If { span, .. }
            | Expr::Path { span, .. }
            | Expr::RecordLiteral { span, .. }
            | Expr::Field { span, .. }
            | Expr::TypePath { span, .. } => span.clone(),
            Expr::Block(block) => block.span.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct MatchArm {
    pub(crate) pattern: Pattern,
    pub(crate) guard: Option<Expr>,
    pub(crate) body: Expr,
    #[allow(dead_code)]
    pub(crate) span: Span,
}

#[derive(Debug, Clone)]
pub(crate) enum Pattern {
    /// A variant pattern, optionally qualified by enum name via `::`: `Some(v)`,
    /// `None`, `Color::Red`. The `::` qualifier is optional; bare `Red` also
    /// matches. `Color.Red` (dotted) is not a variant pattern.
    Variant {
        enum_name: Option<String>,
        variant: String,
        fields: Vec<FieldBinding>,
        span: Span,
    },
    /// `_`: ignore the scrutinee.
    Wildcard(Span),
    /// Bind the whole scrutinee to a name (catch-all).
    Binding { name: String, span: Span },
}

#[derive(Debug, Clone)]
pub(crate) enum FieldBinding {
    /// Bind the payload field to a name.
    Name(String),
    /// `_`: ignore the payload field.
    Ignore,
}
