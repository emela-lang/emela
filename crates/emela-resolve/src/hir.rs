//! HIR: a tree in which every name points to a definition or a local. Type inference sees only
//! this.
//!
//! Definitions ([`DefId`]), locals ([`LocalId`]), expressions, patterns and types live in one
//! arena for the whole program, so references across modules are plain IDs. Every node has a
//! position ([`Span`]). Prelude and built-in module definitions have no source, so their position
//! is `None`.

use la_arena::{Arena, ArenaMap, Idx};
use rowan::TextRange;
use smol_str::SmolStr;

use crate::module_name::ModuleName;
use crate::source::ModuleId;

pub type DefId = Idx<DefData>;
pub type LocalId = Idx<LocalData>;
pub type ExprId = Idx<Expr>;
pub type PatId = Idx<Pat>;
pub type TypeId = Idx<TypeRef>;

/// A module is one file, so the file is identified by its module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub module: ModuleId,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DefModule {
    Source(ModuleId),
    /// Prelude (spec 15.1), visible without an import.
    Prelude,
    /// A built-in module (`String`, `List`, ...), looked up through [`crate::BuiltinModules`].
    Builtin(SmolStr),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DefKind {
    Fn,
    /// `type`. Its same-named constructor is a separate [`DefKind::Ctor`].
    Type,
    Enum,
    /// The constructor of a `type`, an enum variant, or construction of a handler from its
    /// fields.
    Ctor,
    /// `error`. In the type namespace, the error tag; in the constructor namespace, the
    /// constructor that builds its value.
    Error,
    Const,
    Effect,
    /// An effect operation.
    Op,
    Handler,
    Layer,
    Trait,
    /// A trait function.
    TraitFn,
    Impl,
    /// A function of an impl ([`ImplFn`]).
    ImplFn,
    /// An uppercase name (`A`) or a type name (`Item`).
    TypeParam,
    /// `Int`, `String`, `List`, ...
    BuiltinType,
}

impl DefKind {
    /// The English name used in diagnostic messages.
    pub fn describe(self) -> &'static str {
        match self {
            DefKind::Fn => "function",
            DefKind::Type => "type",
            DefKind::Enum => "enum",
            DefKind::Ctor => "constructor",
            DefKind::Error => "error",
            DefKind::Const => "constant",
            DefKind::Effect => "effect",
            DefKind::Op => "effect operation",
            DefKind::Handler => "handler",
            DefKind::Layer => "layer",
            DefKind::Trait => "trait",
            DefKind::TraitFn => "trait function",
            DefKind::Impl => "impl",
            DefKind::ImplFn => "impl function",
            DefKind::TypeParam => "type parameter",
            DefKind::BuiltinType => "type",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefData {
    pub name: SmolStr,
    pub kind: DefKind,
    pub module: DefModule,
    /// The enclosing definition: the enum of a variant, the effect of an operation, the
    /// declaration a type parameter belongs to.
    pub parent: Option<DefId>,
    /// `None` for definitions without source.
    pub span: Option<Span>,
    /// Variants and constructors follow their parent type.
    pub is_pub: bool,
    /// A `pub opaque` type (spec 4.4). Constructors follow their parent type.
    pub is_opaque: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LocalKind {
    /// A parameter of a function (top-level, trait, impl, or handler operation).
    Param,
    /// `self`. Written as a parameter in impls and traits; implicit in handlers.
    SelfParam,
    LambdaParam,
    /// The parameter in place of a pattern of a lambda (`fn((a, b)) { ... }`, spec 6.1). It has
    /// no name in scope; the body starts with a binding of the pattern to it. The name is the
    /// pattern's text, for display.
    PatParam,
    /// A binding `x = ...` and the variables in its pattern.
    Binding,
    /// Variables in the patterns of match and escape arms.
    ArmBinding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalData {
    pub name: SmolStr,
    pub kind: LocalKind,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Res {
    Def(DefId),
    Local(LocalId),
    /// Already reported.
    Err,
}

/// A name reference written in source, for LSP go-to-definition and find-references.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NameRef {
    /// The name token's position. `Http.Client.get` gives one reference per segment.
    pub span: Span,
    pub target: RefTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RefTarget {
    Def(DefId),
    Local(LocalId),
    /// A source module (import paths and the left of `.`).
    Module(ModuleId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeRef {
    pub kind: TypeKind,
    /// `None` only for types inside Prelude declarations (the `A` of `Some(A)`), which have no
    /// source.
    pub span: Option<Span>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeKind {
    /// Named types and type parameters: `List[Int]`, `Email.Email`, `A`. `res` points to a type
    /// (type, enum, error, built-in type) or a type parameter.
    Path {
        res: Res,
        args: Vec<TypeId>,
    },
    /// `Self` (inside traits and impls).
    SelfType,
    Unit,
    Tuple(Vec<TypeId>),
    Fn {
        params: Vec<TypeId>,
        ret: Option<TypeId>,
        fails: Option<ErrorSet>,
        uses: Option<EffectSet>,
    },
    /// An error set written as a type argument of a named type (`Handler[NotFound | Timeout, R]`,
    /// spec 9.2). Appears only in type argument positions.
    ErrorSet(ErrorSet),
    /// A braced set written as a type argument of a named type (`Handler[E, { Db, Log }]`, `{}`).
    /// Appears only in type argument positions; an empty `{}` may also appear where an error set
    /// goes.
    EffectSet(EffectSet),
    /// A type that could not be parsed.
    Missing,
}

/// The `fails` clause. Elements point to errors or type parameters. `fails {}` is empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorSet {
    pub items: Vec<(Res, Span)>,
    pub span: Span,
}

/// The `use` clause. Elements point to effects or type parameters. `use {}` is empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectSet {
    pub items: Vec<EffectRef>,
    pub span: Span,
}

/// An element of an effect set, pointing to an effect or a type parameter. `args` are the
/// effect's type arguments (the `Int` of `State[Int]`, spec 8.1); empty if none are written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectRef {
    pub res: Res,
    pub args: Vec<TypeId>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pat {
    pub kind: PatKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatKind {
    Wild,
    /// Matches anything and binds it.
    Bind(LocalId),
    Lit(Literal),
    /// Matching against a const; `res` points to the const.
    Const(Res),
    /// `args` is `None` without parentheses (`Empty`).
    Ctor {
        res: Res,
        args: Option<Vec<PatArg>>,
        /// `..` ignores the remaining fields.
        rest: bool,
    },
    Unit,
    Tuple(Vec<PatId>),
    /// `[]`，`[x, ..rest]`，`[x, ..]`．
    List {
        elems: Vec<PatId>,
        rest: Option<ListRest>,
    },
    Missing,
}

/// Named arguments carry a label. `User(name:)` binds `name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatArg {
    pub label: Option<Label>,
    pub pat: PatId,
}

/// `..rest` binds a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListRest {
    pub binding: Option<LocalId>,
    pub span: Span,
}

/// Labels of named arguments and field patterns. Not resolved here, since that needs the callee's
/// signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub name: SmolStr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Literal {
    /// Digits with `_` removed. Range checks are left to type inference. A pattern's literal may
    /// start with `-` (`-1`, `-0x10`, spec 6.5); in expressions `-` is unary minus.
    Int(SmolStr),
    /// May start with `-` in a pattern, like [`Literal::Int`].
    Float(SmolStr),
    /// A string without interpolation, escapes already resolved.
    String(String),
}

/// The value of an integer literal's text (`42`, `0xFF`, `0b1010`, `-0x10`), or `None` if it does
/// not fit `i128`. The `-` and the digits are one value (spec 6.5), so `-2147483648` is Int's
/// minimum.
pub fn int_literal_value(text: &str) -> Option<i128> {
    let (negative, text) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (digits, radix) = if let Some(hex) = text.strip_prefix("0x") {
        (hex, 16)
    } else if let Some(bin) = text.strip_prefix("0b") {
        (bin, 2)
    } else {
        (text, 10)
    };
    let n = i128::from_str_radix(digits, radix).ok()?;
    Some(if negative { -n } else { n })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    /// `|>`. The left side becomes the first argument of the call on the right (spec 6.7).
    Pipe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    Not,
    Neg,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StrPart {
    /// Escapes already resolved.
    Text(String),
    Interp(ExprId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExprKind {
    Missing,
    Lit(Literal),
    Interpolated(Vec<StrPart>),
    /// Locals, functions, consts, constructors, `Trait.f`, `Module.f`.
    Path(Res),
    /// A field or capability operation `x.name`. The name is looked up by type inference.
    Field {
        base: ExprId,
        name: Label,
    },
    Call {
        callee: ExprId,
        args: Vec<Arg>,
    },
    Unary {
        op: UnaryOp,
        operand: ExprId,
    },
    Binary {
        op: BinaryOp,
        lhs: ExprId,
        rhs: ExprId,
    },
    Unit,
    Tuple(Vec<ExprId>),
    List {
        elems: Vec<ExprId>,
        rest: Option<ExprId>,
    },
    Block(Block),
    If {
        cond: ExprId,
        then_branch: ExprId,
        else_branch: Option<ExprId>,
    },
    Match {
        scrutinee: ExprId,
        arms: Vec<Arm>,
    },
    /// `e escape { ... }`. The top-level pattern of each arm is an error constructor.
    Escape {
        expr: ExprId,
        arms: Vec<Arm>,
    },
    Fail(ExprId),
    Assert(ExprId),
    /// A record update `C(f: e, ..base)` (spec 5.3). `ctor` refers to the constructor of
    /// something with a single constructor (a type, an error, or a handler with fields).
    /// `fields` holds only named arguments.
    Update {
        ctor: ExprId,
        fields: Vec<Arg>,
        base: ExprId,
    },
    /// `use X`. `effect` points to the effect; `args` are its type arguments (`use State[Int]`,
    /// spec 8.1).
    Use {
        effect: Res,
        args: Vec<TypeId>,
    },
    /// `with H1, H2 { body }`. `handlers` are the listed items: handlers, layers, labels, and
    /// `L by P` (issue #255).
    With {
        handlers: Vec<WithItem>,
        body: ExprId,
    },
    Lambda {
        params: Vec<Param>,
        body: ExprId,
    },
}

/// An item listed in `with`. `res` is a handler, a layer, or a label (an effect declared `of`
/// another); with `by`, `res` is the label and `by` the handler or effect plugged into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithItem {
    pub res: Res,
    pub span: Span,
    /// The arguments written after `res` (`WorkerKv(binding: "KV")`). With `by`, they follow the
    /// label, which takes none.
    pub args: Option<SupplyArgs>,
    pub by: Option<(Res, Span)>,
    /// Type arguments of an effect right of `by` (`Counter by State[Int]`).
    pub by_args: Vec<TypeId>,
    /// The arguments written after the right of `by` (`AuditLog by FileLog(path: "a.log")`).
    pub by_init_args: Option<SupplyArgs>,
}

impl WithItem {
    /// The handler or layer that supplies the item (the right of `by`, else `res`), with the
    /// arguments written after it.
    pub fn supplier(&self) -> (Res, Option<&SupplyArgs>) {
        match self.by {
            Some((by, _)) => (by, self.by_init_args.as_ref()),
            None => (self.res, self.args.as_ref()),
        }
    }
}

/// The arguments of a handler's init written on an item of `with` or `layer`, or on the right of
/// `by` (`WorkerKv(binding: "KV")`, `[effect.handler]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupplyArgs {
    pub args: Vec<Arg>,
    /// The parenthesized list.
    pub span: Span,
}

/// A call argument. Named arguments (with `label`) are not resolved, since that needs the callee's
/// signature; only the name and position are kept. The shorthand `NotFound(id:)` makes `value` a reference to the local `id` and sets
/// `punned`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arg {
    pub label: Option<Label>,
    pub value: ExprId,
    pub punned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    /// The value of the last statement if it is an expression. A block ending in a binding, or an
    /// empty block, has value `()`.
    pub tail: Option<ExprId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stmt {
    /// `pat: ty = value`
    Let {
        pat: PatId,
        ty: Option<TypeId>,
        value: ExprId,
    },
    Expr(ExprId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arm {
    pub pat: PatId,
    pub guard: Option<ExprId>,
    pub body: ExprId,
}

/// Types are required in top-level fns and optional in lambdas, handlers and impls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub local: LocalId,
    pub ty: Option<TypeId>,
}

/// Shared by top-level fns, trait fns, and impl and handler fns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnSig {
    pub type_params: Vec<TypeParam>,
    pub params: Vec<Param>,
    pub ret: Option<TypeId>,
    pub fails: Option<ErrorSet>,
    pub uses: Option<EffectSet>,
    pub is_suspend: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeParam {
    pub def: DefId,
    /// Constraints. Point to a trait (`Ord`) or a built-in constraint (`Immediate`).
    pub bounds: Vec<(Res, Span)>,
}

/// Used by type, enum variants, error and handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDef {
    pub name: SmolStr,
    pub ty: TypeId,
    pub span: Span,
}

/// The shape of a constructor's fields (spec 5.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fields {
    None,
    Named(Vec<FieldDef>),
    Positional(Vec<TypeId>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnItem {
    pub sig: FnSig,
    /// Absent for `@external` functions and trait functions without a default implementation.
    pub body: Option<ExprId>,
    pub is_external: bool,
    /// The arguments of `@external(js, "console.log")`, in source order. An `@external` without
    /// arguments is not included.
    pub externals: Vec<ExternalAttr>,
    /// A built-in function of the bundled core sources.
    pub is_intrinsic: bool,
    /// The arguments of `@export(js, "default", "fetch")` (spec 12.7), in source order, in the
    /// same shape as [`FnItem::externals`]. Malformed ones are left out (E0146).
    pub exports: Vec<ExternalAttr>,
    /// The `@entry(Http.serve)` attributes (spec 12.8), in source order.
    pub entries: Vec<EntryAttr>,
}

/// One `@entry(T)` on a fn (spec 12.8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryAttr {
    /// What `T` names. `None` when the argument is not a single name or path (`@entry`,
    /// `@entry(1)`, `@entry(a, b)`), which is reported as not naming an entry adapter (E0439).
    pub adapter: Option<Res>,
    /// The whole attribute.
    pub span: Span,
}

/// The arguments of `@external(target, "a", "b")` (spec 12.1). `target` is the first argument's
/// name (`js`, `wasm`); `args` are the remaining strings. Malformed arguments (a first argument
/// that is not a name, non-string remaining ones) are left out; the parser reports them
/// (E0138 to E0141).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalAttr {
    pub target: SmolStr,
    pub args: Vec<String>,
    /// The whole attribute.
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeItem {
    pub type_params: Vec<TypeParam>,
    /// The same-named constructor. Absent for an `@external` or `@intrinsic` type without
    /// fields.
    pub ctor: Option<DefId>,
    pub fields: Fields,
    pub derives: Vec<(Res, Span)>,
    /// A built-in type of the bundled sources, marked with `@intrinsic` (`Std.Bytes.Bytes`,
    /// `[kw.annotation]`). Its values are represented by the runtime.
    pub is_intrinsic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumItem {
    pub type_params: Vec<TypeParam>,
    /// [`DefKind::Ctor`]s, in declaration order.
    pub variants: Vec<DefId>,
    pub derives: Vec<(Res, Span)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CtorItem {
    /// The type the constructor builds (type, enum or handler).
    pub owner: DefId,
    pub fields: Fields,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorItem {
    pub fields: Fields,
    /// `error DbError(message: String) derive Show` (spec 7.1, 10.5).
    pub derives: Vec<(Res, Span)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstItem {
    pub ty: Option<TypeId>,
    pub value: ExprId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectItem {
    /// Type parameters (the `S` of `effect State[S]`, spec 8.1), usable in operation signatures.
    pub type_params: Vec<TypeParam>,
    pub ops: Vec<DefId>,
    /// A built-in effect of the bundled sources, marked with `@intrinsic` (`Scope`, spec 11.2).
    /// Only built-in effects may have set type parameters and operation type parameters
    /// (spec 8.1).
    pub is_intrinsic: bool,
    /// For a label (`effect AuditLog of Console [default]`), the effect it stands for. A label
    /// has no operations of its own; it has those of `base`.
    pub label: Option<LabelOf>,
}

/// The `of Console default` of a label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelOf {
    pub base: Res,
    pub span: Span,
    /// Type arguments of the base (`of State[Int]`).
    pub args: Vec<TypeId>,
    /// `default`: a `with` providing the base also provides the label (issue #255).
    pub default: bool,
}

/// No body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpItem {
    pub sig: FnSig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandlerItem {
    /// The effect after `:`.
    pub effect: Res,
    /// Its type arguments (the `Int` of `handler H: State[Int]`, spec 8.1).
    pub effect_args: Vec<TypeId>,
    pub fields: Vec<FieldDef>,
    /// The constructor when there are fields, used in `init`.
    pub ctor: Option<DefId>,
    pub init: Option<ExprId>,
    /// The parameters of init (`init(binding: String)`). Empty without them.
    pub init_params: Vec<Param>,
    /// `release` and its implicit `self`.
    pub release: Option<(LocalId, ExprId)>,
    pub ops: Vec<HandlerOp>,
}

/// Parameter types come from the effect declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandlerOp {
    pub name: Label,
    /// The effect operation implemented.
    pub op: Res,
    pub is_suspend: bool,
    /// The implicit `self`.
    pub self_local: LocalId,
    pub params: Vec<Param>,
    pub body: Option<ExprId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerItem {
    /// Handlers or layers.
    pub members: Vec<LayerMember>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerMember {
    pub res: Res,
    pub span: Span,
    /// The arguments of the handler's init (`WorkerKv(binding: "KV")`).
    pub args: Option<SupplyArgs>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraitItem {
    pub supertraits: Vec<(Res, Span)>,
    /// [`DefKind::TraitFn`]s.
    pub fns: Vec<DefId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImplItem {
    pub trait_ref: Res,
    /// The type parameters in the head (`K` and `V` of `impl Map[K: Eq, V: Eq]: Eq`, spec 10.2).
    pub type_params: Vec<TypeParam>,
    /// The type before `:`, applied to the type parameters (`Map[K, V]`).
    pub self_ty: Option<TypeId>,
    /// The head is not a type name with distinct type parameters (E0328, reported by name
    /// resolution). Type checking skips the impl's head.
    pub malformed_head: bool,
    pub fns: Vec<ImplFn>,
}

/// The signature comes from the trait declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImplFn {
    /// A [`DefKind::ImplFn`] whose parent is the impl. Not in any namespace: it is called through
    /// the trait function (`Show.show`), and monomorphization picks it (spec 10.3). It is still a
    /// definition so that type checking and lowering handle it per definition, like an fn.
    pub def: DefId,
    pub name: Label,
    /// The trait function implemented.
    pub trait_fn: Res,
    pub params: Vec<Param>,
    pub body: Option<ExprId>,
}

/// Type parameters and built-in definitions have no item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Fn(FnItem),
    Type(TypeItem),
    Enum(EnumItem),
    Ctor(CtorItem),
    Error(ErrorItem),
    Const(ConstItem),
    Effect(EffectItem),
    Op(OpItem),
    Handler(HandlerItem),
    Layer(LayerItem),
    Trait(TraitItem),
    /// With a body, it is the default implementation.
    TraitFn(FnItem),
    Impl(ImplItem),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleItems {
    /// The module's name (`Std.Fiber`). Used to name definitions in diagnostics (`Fiber.scope`).
    pub name: ModuleName,
    /// In source order.
    pub items: Vec<DefId>,
    /// A core module bundled with the compiler (`ModuleMap::is_core`). Its impls of the Prelude's
    /// traits for the Prelude's types are the built-in impls (spec 10.6).
    pub is_core: bool,
}

#[derive(Debug, Default, Clone)]
pub struct Program {
    pub defs: Arena<DefData>,
    pub items: ArenaMap<DefId, Item>,
    pub locals: Arena<LocalData>,
    pub exprs: Arena<Expr>,
    pub pats: Arena<Pat>,
    pub types: Arena<TypeRef>,
    pub modules: ArenaMap<ModuleId, ModuleItems>,
    /// Looked up by name.
    pub prelude: Prelude,
    /// Resolved name references, in source order (not module order). The names of definitions
    /// and the places where locals are bound are not included (they are in [`DefData::span`] and
    /// [`LocalData::span`]). Effect operations (`log.info`) are looked up by type inference, so
    /// they are not here either.
    pub refs: Vec<NameRef>,
    /// The exports made by `@entry` (spec 12.8), added after name resolution by
    /// `emela_types::add_entry_exports`, which knows the entry module. Empty until then.
    pub entry_exports: Vec<EntryExport>,
}

/// The fn that `@entry(T)` on `f` exposes for each `@export` of `T` (spec 12.8): a top-level fn
/// [`EntryExport::def`] in the entry module, with the parameters of `T` after the first and the
/// body `T(f, ...)`. It has no name in scope and no span. Its expressions and parameters have an
/// empty range at the end of the attribute, and a type error in it is reported at the attribute
/// (E0440).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryExport {
    pub def: DefId,
    /// `f`.
    pub entry: DefId,
    /// `T`, an entry adapter.
    pub adapter: DefId,
    /// The `@entry(T)` attribute.
    pub span: Span,
}

/// Used by type inference to look up built-in types and constructors.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Prelude {
    /// The names of spec 15.1, plus the built-in trait (`Immediate`) and effect (`Async`).
    pub defs: Vec<DefId>,
}

impl Program {
    /// Constructors such as `Some` can be looked up too.
    pub fn prelude_def(&self, name: &str) -> Option<DefId> {
        self.prelude
            .defs
            .iter()
            .copied()
            .find(|&def| self.defs[def].name == name)
    }

    pub fn item(&self, def: DefId) -> Option<&Item> {
        self.items.get(def)
    }

    /// The impl of a [`DefKind::ImplFn`] and the function. `None` for other definitions.
    pub fn impl_fn(&self, def: DefId) -> Option<(DefId, &ImplFn)> {
        if self.defs[def].kind != DefKind::ImplFn {
            return None;
        }
        let parent = self.defs[def].parent?;
        let Some(Item::Impl(item)) = self.item(parent) else {
            return None;
        };
        item.fns.iter().find(|f| f.def == def).map(|f| (parent, f))
    }

    /// The effect a label stands for (`Console` for `effect AuditLog of Console`). `None` for
    /// effects that are not labels, and for labels whose base did not resolve.
    pub fn label_base(&self, effect: DefId) -> Option<DefId> {
        match self.item(effect) {
            Some(Item::Effect(EffectItem {
                label:
                    Some(LabelOf {
                        base: Res::Def(base),
                        ..
                    }),
                ..
            })) => Some(*base),
            _ => None,
        }
    }

    /// The operations of an effect. A label has those of its base (one level: the base of a
    /// label is not a label, E0429).
    pub fn effect_ops(&self, effect: DefId) -> &[DefId] {
        let effect = self.label_base(effect).unwrap_or(effect);
        match self.item(effect) {
            Some(Item::Effect(e)) if e.label.is_none() => &e.ops,
            _ => &[],
        }
    }
}
