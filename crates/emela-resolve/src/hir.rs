//! HIR．名前がすべて定義か局所変数を指している木．型推論はこれだけを見る．
//!
//! 定義（[`DefId`]），局所変数（[`LocalId`]），式，パターン，型はプログラム全体で1つの
//! アリーナに持つ．モジュールをまたぐ参照も ID のまま引ける．位置（[`Span`]）は全ノードが持つ．
//! Prelude と組み込みのモジュールの定義はソースを持たないので，位置が `None` になる．

use la_arena::{Arena, ArenaMap, Idx};
use rowan::TextRange;
use smol_str::SmolStr;

use crate::source::ModuleId;

pub type DefId = Idx<DefData>;
pub type LocalId = Idx<LocalData>;
pub type ExprId = Idx<Expr>;
pub type PatId = Idx<Pat>;
pub type TypeId = Idx<TypeRef>;

/// ソースの中の位置．モジュールが1つのファイルなので，ファイルはモジュールで指す．
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub module: ModuleId,
    pub range: TextRange,
}

/// 定義の持ち主のモジュール．
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DefModule {
    /// ソースのモジュール．
    Source(ModuleId),
    /// Prelude（15.1）．import なしで見える．
    Prelude,
    /// 組み込みのモジュール（`String`，`List` など）．[`crate::BuiltinModules`] から引く．
    Builtin(SmolStr),
}

/// 定義の種類．
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DefKind {
    Fn,
    /// `type`．同じ名前の構成子は [`DefKind::Ctor`] として別に持つ．
    Type,
    Enum,
    /// 構成子．`type` の構成子，enum のバリアント，handler のフィールドからの構築．
    Ctor,
    /// `error`．型の名前空間ではエラーのタグ，構成子の名前空間ではその値を作る構成子．
    Error,
    Const,
    Effect,
    /// effect の操作．
    Op,
    Handler,
    Layer,
    Trait,
    /// trait の関数．
    TraitFn,
    Impl,
    /// 型引数．大文字名（`A`）か型名（`Item`）．
    TypeParam,
    /// 組み込みの型（`Int`，`String`，`List` など）．
    BuiltinType,
}

impl DefKind {
    /// 診断の文面に使う英語の名前．
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
    /// 外側の定義．バリアントなら enum，操作なら effect，型引数なら付いている宣言．
    pub parent: Option<DefId>,
    /// 名前の位置．ソースのないものは `None`．
    pub span: Option<Span>,
    /// `pub` が付いているか．バリアントと構成子は親の型に従う．
    pub is_pub: bool,
    /// `pub opaque` の型（4.4）．構成子は親の型に従う．
    pub is_opaque: bool,
}

/// 局所変数の種類．
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LocalKind {
    /// 関数（トップレベル，trait，impl，handler の操作）の引数．
    Param,
    /// `self`．impl と trait では引数として書き，handler では暗黙に入る．
    SelfParam,
    /// ラムダの引数．
    LambdaParam,
    /// 束縛 `x = ...` とそのパターンの中の変数．
    Binding,
    /// match と escape の腕のパターンの変数．
    ArmBinding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalData {
    pub name: SmolStr,
    pub kind: LocalKind,
    pub span: Span,
}

/// 名前の参照の解決先．
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Res {
    Def(DefId),
    Local(LocalId),
    /// 解決できなかった．診断は出してある．
    Err,
}

// ---------------------------------------------------------------------------
// 型

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeRef {
    pub kind: TypeKind,
    /// Prelude の宣言の中の型（`Some(A)` の `A`）だけはソースがないので `None`．
    pub span: Option<Span>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeKind {
    /// 名前の付いた型と型引数．`List[Int]`，`Email.Email`，`A`．
    /// `res` は型（type，enum，error，組み込みの型）か型引数を指す．
    Path {
        res: Res,
        args: Vec<TypeId>,
    },
    /// `Self`（trait と impl の中）．
    SelfType,
    Unit,
    Tuple(Vec<TypeId>),
    Fn {
        params: Vec<TypeId>,
        ret: Option<TypeId>,
        fails: Option<ErrorSet>,
        uses: Option<EffectSet>,
    },
    /// 読めなかった型．
    Missing,
}

/// `fails` 節．要素はエラーか型引数を指す．`fails {}` は空．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorSet {
    pub items: Vec<(Res, Span)>,
    pub span: Span,
}

/// `use` 節．要素はエフェクトか型引数を指す．`use {}` は空．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectSet {
    pub items: Vec<(Res, Span)>,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// パターン

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pat {
    pub kind: PatKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatKind {
    Wild,
    /// 変数．何にでも合い，束縛する．
    Bind(LocalId),
    Lit(Literal),
    /// const との照合．`res` は const を指す．
    Const(Res),
    /// 構成子のパターン．`args` が `None` なら括弧なし（`Empty`）．
    Ctor {
        res: Res,
        args: Option<Vec<PatArg>>,
        /// `..` で残りのフィールドを無視する．
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

/// 構成子のパターンの引数．名前付きならラベルを持つ．`User(name:)` は `name` を束縛する．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatArg {
    pub label: Option<Label>,
    pub pat: PatId,
}

/// リストのパターンの残り．`..rest` なら名前を束縛する．
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListRest {
    pub binding: Option<LocalId>,
    pub span: Span,
}

/// 名前付き引数とフィールドのパターンのラベル．解決はしない（呼び出し先のシグネチャが要る）．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub name: SmolStr,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// 式

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Literal {
    /// `_` を取り除いた数字の列．範囲の検査は型推論が行う．
    Int(SmolStr),
    Float(SmolStr),
    /// 補間のない文字列．エスケープは解いてある．
    String(String),
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
    /// `|>`．右辺の呼び出しの第1引数に左辺が入る（6.7）．
    Pipe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    Not,
    Neg,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StrPart {
    /// エスケープを解いた文字列．
    Text(String),
    Interp(ExprId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExprKind {
    Missing,
    Lit(Literal),
    /// 補間を含む文字列．
    Interpolated(Vec<StrPart>),
    /// 名前の参照．局所変数，関数，const，構成子，`Trait.f`，`Module.f`．
    Path(Res),
    /// フィールドか能力の操作 `x.name`．名前は型推論で引く．
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
    /// `e escape { ... }`．腕のパターンの最上位はエラーの構成子．
    Escape {
        expr: ExprId,
        arms: Vec<Arm>,
    },
    Fail(ExprId),
    Assert(ExprId),
    /// `use X`．`effect` はエフェクトを指す．
    Use {
        effect: Res,
    },
    /// `with H1, H2 { body }`．`handlers` は handler か layer を指す．
    With {
        handlers: Vec<(Res, Span)>,
        body: ExprId,
    },
    Lambda {
        params: Vec<Param>,
        body: ExprId,
    },
}

/// 呼び出しの引数．`label` のある名前付き引数は解決せず，名前と位置だけを残す．
/// `NotFound(id:)` の省略形は `value` が変数 `id` の参照になり，`punned` が立つ．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arg {
    pub label: Option<Label>,
    pub value: ExprId,
    pub punned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    /// 最後の文が式なら，その値．束縛で終わるか空ならブロックの値は `()`．
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

// ---------------------------------------------------------------------------
// 宣言

/// 関数の引数．トップレベルの fn では型が必須，ラムダ，handler，impl では省ける．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub local: LocalId,
    pub ty: Option<TypeId>,
}

/// 関数のシグネチャと本体．トップレベルの fn，trait の fn，impl と handler の fn で共有する．
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
    /// 制約．Trait（`Ord`）か組み込みの制約（`Immediate`）を指す．
    pub bounds: Vec<(Res, Span)>,
}

/// フィールド．type，enum のバリアント，error，handler で使う．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDef {
    pub name: SmolStr,
    pub ty: TypeId,
    pub span: Span,
}

/// 構成子のフィールドの形（5.4）．
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fields {
    None,
    Named(Vec<FieldDef>),
    Positional(Vec<TypeId>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnItem {
    pub sig: FnSig,
    /// 本体．`@external` の関数と，既定の実装のない trait の関数にはない．
    pub body: Option<ExprId>,
    /// `@external` が付いている．
    pub is_external: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeItem {
    pub type_params: Vec<TypeParam>,
    /// 同じ名前の構成子．`@external` のフィールドのない型にはない．
    pub ctor: Option<DefId>,
    pub fields: Fields,
    pub derives: Vec<(Res, Span)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumItem {
    pub type_params: Vec<TypeParam>,
    /// バリアント（[`DefKind::Ctor`]）．宣言の順．
    pub variants: Vec<DefId>,
    pub derives: Vec<(Res, Span)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CtorItem {
    /// 構成子が作る型（type か enum か handler）．
    pub owner: DefId,
    pub fields: Fields,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorItem {
    pub fields: Fields,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstItem {
    pub ty: Option<TypeId>,
    pub value: ExprId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectItem {
    pub ops: Vec<DefId>,
}

/// effect の操作のシグネチャ．本体はない．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpItem {
    pub sig: FnSig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandlerItem {
    /// `implements` のエフェクト．
    pub effect: Res,
    pub fields: Vec<FieldDef>,
    /// フィールドがあるときの構成子（init で使う）．
    pub ctor: Option<DefId>,
    pub init: Option<ExprId>,
    /// `release` と，その中の暗黙の `self`．
    pub release: Option<(LocalId, ExprId)>,
    pub ops: Vec<HandlerOp>,
}

/// handler の操作の実装．引数の型は effect の宣言から決まる．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandlerOp {
    pub name: Label,
    /// 実装する effect の操作．
    pub op: Res,
    pub is_suspend: bool,
    /// 暗黙の `self`．
    pub self_local: LocalId,
    pub params: Vec<Param>,
    pub body: Option<ExprId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerItem {
    /// handler か layer．
    pub members: Vec<(Res, Span)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraitItem {
    pub supertraits: Vec<(Res, Span)>,
    /// trait の関数（[`DefKind::TraitFn`]）．
    pub fns: Vec<DefId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImplItem {
    pub trait_ref: Res,
    pub self_ty: Option<TypeId>,
    pub fns: Vec<ImplFn>,
}

/// impl の関数．シグネチャは trait の宣言から決まる．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImplFn {
    pub name: Label,
    /// 実装する trait の関数．
    pub trait_fn: Res,
    pub params: Vec<Param>,
    pub body: Option<ExprId>,
}

/// 定義の中身．型引数と組み込みの定義は中身を持たない．
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
    /// trait の関数．本体があれば既定の実装．
    TraitFn(FnItem),
    Impl(ImplItem),
}

/// 1つのモジュールの宣言．書かれた順．
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleItems {
    pub items: Vec<DefId>,
}

/// 名前解決の結果．
#[derive(Debug, Default, Clone)]
pub struct Program {
    pub defs: Arena<DefData>,
    pub items: ArenaMap<DefId, Item>,
    pub locals: Arena<LocalData>,
    pub exprs: Arena<Expr>,
    pub pats: Arena<Pat>,
    pub types: Arena<TypeRef>,
    pub modules: ArenaMap<ModuleId, ModuleItems>,
    /// Prelude の定義．名前から引ける．
    pub prelude: Prelude,
}

/// Prelude の定義の ID．型推論が組み込みの型と構成子を引くのに使う．
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Prelude {
    /// 15.1 の名前と，組み込みの Trait（`Immediate`）とエフェクト（`Async`）．
    pub defs: Vec<DefId>,
}

impl Program {
    /// Prelude の定義を名前で引く．`Some` などの構成子も引ける．
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
}
