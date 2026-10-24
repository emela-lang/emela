//! Typed AST for declarations (spec 17.3).

use super::{AstNode, BlockExpr, Expr, FnDecl, ParamList, child, children, token};
use crate::SyntaxKind::*;
use crate::{SyntaxNode, SyntaxToken};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Item {
    Import(Import),
    Fn(FnDecl),
    Type(TypeDecl),
    Enum(EnumDecl),
    Error(ErrorDecl),
    Const(ConstDecl),
    Effect(EffectDecl),
    Handler(HandlerDecl),
    Layer(LayerDecl),
    Trait(TraitDecl),
    Impl(ImplDecl),
}

impl Item {
    pub fn cast(node: SyntaxNode) -> Option<Self> {
        Some(match node.kind() {
            IMPORT => Item::Import(Import(node)),
            FN_DECL => Item::Fn(FnDecl::cast(node)?),
            TYPE_DECL => Item::Type(TypeDecl(node)),
            ENUM_DECL => Item::Enum(EnumDecl(node)),
            ERROR_DECL => Item::Error(ErrorDecl(node)),
            CONST_DECL => Item::Const(ConstDecl(node)),
            EFFECT_DECL => Item::Effect(EffectDecl(node)),
            HANDLER_DECL => Item::Handler(HandlerDecl(node)),
            LAYER_DECL => Item::Layer(LayerDecl(node)),
            TRAIT_DECL => Item::Trait(TraitDecl(node)),
            IMPL_DECL => Item::Impl(ImplDecl(node)),
            _ => return None,
        })
    }

    pub fn syntax(&self) -> &SyntaxNode {
        match self {
            Item::Import(it) => it.syntax(),
            Item::Fn(it) => it.syntax(),
            Item::Type(it) => it.syntax(),
            Item::Enum(it) => it.syntax(),
            Item::Error(it) => it.syntax(),
            Item::Const(it) => it.syntax(),
            Item::Effect(it) => it.syntax(),
            Item::Handler(it) => it.syntax(),
            Item::Layer(it) => it.syntax(),
            Item::Trait(it) => it.syntax(),
            Item::Impl(it) => it.syntax(),
        }
    }
}

/// A declaration with annotations, doc comments and `pub`.
pub trait HasAttrs: AstNode {
    fn annotations(&self) -> impl Iterator<Item = Annotation> {
        children(self.syntax())
    }
    /// One token per line, with the `##` still included.
    fn doc_comments(&self) -> impl Iterator<Item = SyntaxToken> {
        self.syntax()
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| t.kind() == DOC_COMMENT)
    }
    fn is_pub(&self) -> bool {
        token(self.syntax(), PUB_KW).is_some()
    }
    fn is_opaque(&self) -> bool {
        token(self.syntax(), OPAQUE_KW).is_some()
    }
    fn has_annotation(&self, name: &str) -> bool {
        self.annotations()
            .any(|a| a.name().is_some_and(|n| n.text() == name))
    }
}

macro_rules! has_attrs {
    ($($name:ident),*) => { $(impl HasAttrs for $name {})* };
}

has_attrs!(
    FnDecl,
    TypeDecl,
    EnumDecl,
    ErrorDecl,
    ConstDecl,
    EffectDecl,
    HandlerDecl,
    LayerDecl,
    TraitDecl,
    ImplDecl,
    OpSig,
    Field,
    Variant
);

/// The first name token after the keyword.
fn name(node: &SyntaxNode) -> Option<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| matches!(t.kind(), LOWER_NAME | TYPE_NAME | UPPER_NAME))
}

fn paths(node: &SyntaxNode) -> impl Iterator<Item = Path> {
    children(node)
}

ast_node!(
    /// `import Data.Json.{Json, decode}`
    Import,
    IMPORT
);

impl Import {
    pub fn path(&self) -> Option<Path> {
        child(&self.0)
    }
    /// The names imported with `.{...}`; empty without `.{...}`.
    pub fn names(&self) -> impl Iterator<Item = SyntaxToken> {
        self.0
            .children()
            .find(|n| n.kind() == IMPORT_LIST)
            .into_iter()
            .flat_map(|list| list.children_with_tokens())
            .filter_map(|e| e.into_token())
            .filter(|t| matches!(t.kind(), LOWER_NAME | TYPE_NAME | UPPER_NAME))
    }
}

ast_node!(
    /// `Http.Client`
    Path,
    PATH
);

impl Path {
    pub fn segments(&self) -> impl Iterator<Item = SyntaxToken> {
        self.0
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| t.kind() == TYPE_NAME)
    }
}

ast_node!(
    /// `@external(js, "Math.sin")`
    Annotation,
    ANNOTATION
);

impl Annotation {
    pub fn name(&self) -> Option<SyntaxToken> {
        token(&self.0, LOWER_NAME)
    }
    /// The argument nodes (ANNOT_ARG).
    pub fn args(&self) -> impl Iterator<Item = SyntaxNode> {
        self.0
            .children()
            .find(|n| n.kind() == ANNOT_ARG_LIST)
            .into_iter()
            .flat_map(|list| list.children())
    }
}

ast_node!(
    /// `type User(id: Int) derive Eq`
    TypeDecl,
    TYPE_DECL
);

impl TypeDecl {
    pub fn name(&self) -> Option<SyntaxToken> {
        name(&self.0)
    }
    /// `None` for an `@external` type.
    pub fn field_list(&self) -> Option<FieldList> {
        child(&self.0)
    }
    pub fn derive(&self) -> Option<DeriveClause> {
        child(&self.0)
    }
}

ast_node!(
    /// `error NotFound(id: Int)`
    ErrorDecl,
    ERROR_DECL
);

impl ErrorDecl {
    pub fn name(&self) -> Option<SyntaxToken> {
        name(&self.0)
    }
    pub fn field_list(&self) -> Option<FieldList> {
        child(&self.0)
    }
    pub fn derive(&self) -> Option<DeriveClause> {
        child(&self.0)
    }
}

ast_node!(
    /// `enum Shape { ... }`
    EnumDecl,
    ENUM_DECL
);

impl EnumDecl {
    pub fn name(&self) -> Option<SyntaxToken> {
        name(&self.0)
    }
    pub fn variants(&self) -> impl Iterator<Item = Variant> {
        self.0
            .children()
            .find(|n| n.kind() == VARIANT_LIST)
            .into_iter()
            .flat_map(|list| children(&list).collect::<Vec<_>>())
    }
    pub fn derive(&self) -> Option<DeriveClause> {
        child(&self.0)
    }
}

ast_node!(
    /// `Circle(radius: Float)` `Rect(Float, Float)` `Empty`
    Variant,
    VARIANT
);

impl Variant {
    pub fn name(&self) -> Option<SyntaxToken> {
        name(&self.0)
    }
    pub fn field_list(&self) -> Option<FieldList> {
        child(&self.0)
    }
    /// The type nodes of unnamed fields.
    pub fn tuple_fields(&self) -> impl Iterator<Item = SyntaxNode> {
        self.0
            .children()
            .find(|n| n.kind() == TUPLE_FIELD_LIST)
            .into_iter()
            .flat_map(|list| list.children())
    }
}

ast_node!(
    /// `(id: Int, name: String)`
    FieldList,
    FIELD_LIST
);

impl FieldList {
    pub fn fields(&self) -> impl Iterator<Item = Field> {
        children(&self.0)
    }
}

ast_node!(
    /// `@json("n") name: String`
    Field,
    FIELD
);

impl Field {
    pub fn name(&self) -> Option<SyntaxToken> {
        token(&self.0, LOWER_NAME)
    }
    pub fn ty(&self) -> Option<SyntaxNode> {
        self.0.children().find(|n| n.kind() != ANNOTATION)
    }
}

ast_node!(
    /// `derive Eq, Show`
    DeriveClause,
    DERIVE_CLAUSE
);

impl DeriveClause {
    pub fn traits(&self) -> impl Iterator<Item = Path> {
        paths(&self.0)
    }
}

ast_node!(
    /// `const MAX_RETRY: Int = 3`
    ConstDecl,
    CONST_DECL
);

impl ConstDecl {
    pub fn name(&self) -> Option<SyntaxToken> {
        token(&self.0, UPPER_NAME)
    }
    pub fn ty(&self) -> Option<SyntaxNode> {
        token(&self.0, COLON)?;
        self.0.children().find(|n| n.kind() != ANNOTATION)
    }
    pub fn value(&self) -> Option<Expr> {
        self.0.children().filter_map(Expr::cast).last()
    }
}

ast_node!(
    /// `effect Clock { ... }`
    EffectDecl,
    EFFECT_DECL
);

impl EffectDecl {
    pub fn name(&self) -> Option<SyntaxToken> {
        name(&self.0)
    }
    /// The type parameter list (`[S]` in `effect State[S]`, spec 8.1).
    pub fn type_params(&self) -> Option<SyntaxNode> {
        self.0.children().find(|n| n.kind() == TYPE_PARAM_LIST)
    }
    pub fn ops(&self) -> impl Iterator<Item = OpSig> {
        items(&self.0)
    }
    /// `of Console default` of a label (an effect declared `of` another).
    pub fn label(&self) -> Option<EffectLabel> {
        child(&self.0)
    }
}

ast_node!(
    /// `of Console default`: the effect a label stands for, and whether it falls back to it.
    EffectLabel,
    EFFECT_LABEL
);

impl EffectLabel {
    /// The effect after `of`.
    pub fn base(&self) -> Option<Path> {
        child(&self.0)
    }
    /// The type argument list of the base (`[Int]` in `of State[Int]`).
    pub fn base_type_args(&self) -> Option<SyntaxNode> {
        self.0.children().find(|n| n.kind() == TYPE_ARG_LIST)
    }
    /// Whether `default` is written.
    pub fn is_default(&self) -> bool {
        self.0
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .any(|t| t.kind() == LOWER_NAME && t.text() == "default")
    }
}

ast_node!(
    /// An effect operation. `suspend fn sleep(ms: Int) -> ()`
    OpSig,
    OP_SIG
);

impl OpSig {
    pub fn is_suspend(&self) -> bool {
        token(&self.0, SUSPEND_KW).is_some()
    }
    pub fn name(&self) -> Option<SyntaxToken> {
        token(&self.0, LOWER_NAME)
    }
    /// The type parameter list (`[A]` in `fn fork[A](...)`, spec 8.1, 17.3).
    pub fn type_params(&self) -> Option<SyntaxNode> {
        self.0.children().find(|n| n.kind() == TYPE_PARAM_LIST)
    }
    pub fn param_list(&self) -> Option<ParamList> {
        child(&self.0)
    }
    pub fn ret_type(&self) -> Option<SyntaxNode> {
        self.0
            .children()
            .find(|n| n.kind() == RET_TYPE)?
            .first_child()
    }
}

ast_node!(
    /// `handler PostgresUsers(conn: Connection): Users { ... }`
    HandlerDecl,
    HANDLER_DECL
);

impl HandlerDecl {
    pub fn name(&self) -> Option<SyntaxToken> {
        name(&self.0)
    }
    pub fn field_list(&self) -> Option<FieldList> {
        child(&self.0)
    }
    /// The effect after `:`.
    pub fn effect(&self) -> Option<Path> {
        child(&self.0.children().find(|n| n.kind() == HANDLER_EFFECT)?)
    }
    /// The type argument list of the implemented effect (`[Int]` in `handler H: State[Int]`,
    /// spec 8.1).
    pub fn effect_type_args(&self) -> Option<SyntaxNode> {
        self.0
            .children()
            .find(|n| n.kind() == HANDLER_EFFECT)?
            .children()
            .find(|n| n.kind() == TYPE_ARG_LIST)
    }
    pub fn init(&self) -> Option<BlockExpr> {
        item_block(&self.0, HANDLER_INIT)
    }
    /// The parameters of init (`init(binding: String) { ... }`). `None` without parentheses.
    pub fn init_params(&self) -> Option<ParamList> {
        let list = self.0.children().find(|n| n.kind() == ITEM_LIST)?;
        child(&list.children().find(|n| n.kind() == HANDLER_INIT)?)
    }
    pub fn release(&self) -> Option<BlockExpr> {
        item_block(&self.0, HANDLER_RELEASE)
    }
    /// The operation implementations.
    pub fn fns(&self) -> impl Iterator<Item = FnDecl> {
        items(&self.0)
    }
}

ast_node!(
    /// `layer AppLive { EnvConfig⏎ ConsoleLogger }`
    LayerDecl,
    LAYER_DECL
);

impl LayerDecl {
    pub fn name(&self) -> Option<SyntaxToken> {
        name(&self.0)
    }
    /// The handlers and layers listed, with the arguments of init.
    pub fn members(&self) -> impl Iterator<Item = Supply> {
        items(&self.0)
    }
}

/// A handler, a layer or a label named as an item of `with` or `layer`, or on the right of
/// `by`: a PATH, or a SUPPLY when the arguments of init are written (`WorkerKv(binding: "KV")`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Supply(SyntaxNode);

impl AstNode for Supply {
    fn can_cast(kind: crate::SyntaxKind) -> bool {
        matches!(kind, PATH | SUPPLY)
    }
    fn cast(node: SyntaxNode) -> Option<Self> {
        Self::can_cast(node.kind()).then(|| Self(node))
    }
    fn syntax(&self) -> &SyntaxNode {
        &self.0
    }
}

impl Supply {
    pub fn path(&self) -> Option<Path> {
        match self.0.kind() {
            PATH => Path::cast(self.0.clone()),
            _ => child(&self.0),
        }
    }
    /// The ARG_LIST of the arguments of init. `None` without parentheses.
    pub fn arg_list(&self) -> Option<SyntaxNode> {
        self.0.children().find(|n| n.kind() == ARG_LIST)
    }
}

ast_node!(
    /// `trait Ord: Eq { ... }`
    TraitDecl,
    TRAIT_DECL
);

impl TraitDecl {
    pub fn name(&self) -> Option<SyntaxToken> {
        name(&self.0)
    }
    /// The `Eq` in `trait Ord: Eq`.
    pub fn supertraits(&self) -> impl Iterator<Item = Path> {
        paths(&self.0)
    }
    pub fn fns(&self) -> impl Iterator<Item = FnDecl> {
        items(&self.0)
    }
}

ast_node!(
    /// `impl User: Show { ... }`
    ImplDecl,
    IMPL_DECL
);

impl ImplDecl {
    /// The trait after `:`.
    pub fn trait_ref(&self) -> Option<Path> {
        child(&self.0)
    }
    /// The IMPL_TYPE before `:`, with its type parameters (`Map[K: Eq, V: Eq]`).
    pub fn self_ty(&self) -> Option<SyntaxNode> {
        self.0.children().find(|n| n.kind() == IMPL_TYPE)
    }
    /// The type name of [`Self::self_ty`] (`Map`).
    pub fn self_path(&self) -> Option<Path> {
        child(&self.self_ty()?)
    }
    /// The type parameters of [`Self::self_ty`] (`[K: Eq, V: Eq]`).
    pub fn type_params(&self) -> Option<SyntaxNode> {
        self.self_ty()?
            .children()
            .find(|n| n.kind() == TYPE_PARAM_LIST)
    }
    pub fn fns(&self) -> impl Iterator<Item = FnDecl> {
        items(&self.0)
    }
}

/// Items inside a declaration body (ITEM_LIST).
fn items<N: AstNode>(node: &SyntaxNode) -> impl Iterator<Item = N> {
    node.children()
        .find(|n| n.kind() == ITEM_LIST)
        .into_iter()
        .flat_map(|list| children(&list).collect::<Vec<_>>())
}

/// The `init` or `release` block of a handler.
fn item_block(node: &SyntaxNode, kind: crate::SyntaxKind) -> Option<BlockExpr> {
    let list = node.children().find(|n| n.kind() == ITEM_LIST)?;
    child(&list.children().find(|n| n.kind() == kind)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    fn items(src: &str) -> Vec<Item> {
        let parse = parse(src);
        assert!(parse.diagnostics().is_empty(), "{}", parse.debug_dump());
        parse.tree().items().collect()
    }

    fn text(t: Option<SyntaxToken>) -> String {
        t.map(|t| t.text().to_owned()).unwrap_or_default()
    }

    #[test]
    fn import_と注釈とドキュメントコメント() {
        let items = items(
            "import Data.Json.{Json, decode}\n## 足す\n## 2行目\n@external(js, \"add\")\npub fn add(x: Int) -> Int",
        );
        let Item::Import(import) = &items[0] else {
            panic!("expected `import`")
        };
        let segments: Vec<_> = import
            .path()
            .unwrap()
            .segments()
            .map(|t| t.to_string())
            .collect();
        assert_eq!(segments, ["Data", "Json"]);
        let names: Vec<_> = import.names().map(|t| t.to_string()).collect();
        assert_eq!(names, ["Json", "decode"]);

        let Item::Fn(add) = &items[1] else {
            panic!("expected `fn`")
        };
        assert!(add.is_pub() && add.has_annotation("external"));
        assert_eq!(add.annotations().next().unwrap().args().count(), 2);
        let docs: Vec<_> = add.doc_comments().map(|t| t.to_string()).collect();
        assert_eq!(docs, ["## 足す", "## 2行目"]);
    }

    #[test]
    fn データの宣言() {
        let items = items(
            "pub opaque type User(id: Int, name: String)\n  derive Eq, Show\nenum Shape {\n  Circle(radius: Float)\n  Rect(Float, Float)\n  Empty\n}\nerror NotFound(id: Int) derive Eq, Show\nconst MAX: Int = 3",
        );
        let Item::Type(user) = &items[0] else {
            panic!("expected `type`")
        };
        assert!(user.is_pub() && user.is_opaque());
        assert_eq!(text(user.name()), "User");
        let fields: Vec<_> = user.field_list().unwrap().fields().collect();
        assert_eq!(text(fields[1].name()), "name");
        assert_eq!(fields[1].ty().unwrap().text().to_string(), "String");
        assert_eq!(user.derive().unwrap().traits().count(), 2);

        let Item::Enum(shape) = &items[1] else {
            panic!("expected `enum`")
        };
        let variants: Vec<_> = shape.variants().collect();
        assert_eq!(variants.len(), 3);
        assert_eq!(variants[0].field_list().unwrap().fields().count(), 1);
        assert_eq!(variants[1].tuple_fields().count(), 2);
        assert!(variants[2].field_list().is_none());

        let Item::Error(not_found) = &items[2] else {
            panic!("expected `error`")
        };
        assert_eq!(text(not_found.name()), "NotFound");
        assert_eq!(not_found.derive().unwrap().traits().count(), 2);

        let Item::Const(max) = &items[3] else {
            panic!("expected `const`")
        };
        assert_eq!(text(max.name()), "MAX");
        assert_eq!(max.ty().unwrap().text().to_string(), "Int");
        assert!(matches!(max.value(), Some(Expr::Literal(_))));
    }

    #[test]
    fn derive_の前のドキュメントコメントは型に付かない() {
        let items = items(
            "## 利用者\ntype User(id: Int)\n  # 比較\n\n  ## 導出\n  derive Eq, Show\nenum Color {\n  Red\n}\n  # 表示\n  derive Show\nerror Down(message: String)\n  derive Show",
        );
        let Item::Type(user) = &items[0] else {
            panic!("expected `type`")
        };
        let docs: Vec<_> = user.doc_comments().map(|t| t.to_string()).collect();
        assert_eq!(docs, ["## 利用者"]);
        assert_eq!(user.derive().unwrap().traits().count(), 2);

        let Item::Enum(e) = &items[1] else {
            panic!("expected `enum`")
        };
        assert_eq!(e.derive().unwrap().traits().count(), 1);

        let Item::Error(down) = &items[2] else {
            panic!("expected `error`")
        };
        assert_eq!(down.derive().unwrap().traits().count(), 1);
    }

    #[test]
    fn 効果とハンドラ() {
        let items = items(
            "effect Clock {\n  suspend fn sleep(ms: Int) -> ()\n  fn now() -> Instant\n}\nhandler Pg(conn: Conn): Users {\n  init { Pg(conn: open()) }\n  release { close(self.conn) }\n  fn find(id) { 1 }\n}\nlayer App { Env, Pg }",
        );
        let Item::Effect(clock) = &items[0] else {
            panic!("expected `effect`")
        };
        let ops: Vec<_> = clock.ops().collect();
        assert!(ops[0].is_suspend() && !ops[1].is_suspend());
        assert_eq!(ops[1].ret_type().unwrap().text().to_string(), "Instant");

        let Item::Handler(pg) = &items[1] else {
            panic!("expected `handler`")
        };
        assert_eq!(pg.effect().unwrap().syntax().text().to_string(), "Users");
        assert!(pg.init().is_some() && pg.release().is_some());
        assert_eq!(text(pg.fns().next().unwrap().name()), "find");

        let Item::Layer(app) = &items[2] else {
            panic!("expected `layer`")
        };
        assert_eq!(app.members().count(), 2);
    }

    #[test]
    fn トレイトと実装() {
        let items = items(
            "trait Ord: Eq + Show {\n  fn compare(self, other: Self) -> Ordering\n  fn max(self, other: Self) -> Self { self }\n}\nimpl List[A]: Show {\n  fn show(self) { \"\" }\n}",
        );
        let Item::Trait(ord) = &items[0] else {
            panic!("expected `trait`")
        };
        assert_eq!(ord.supertraits().count(), 2);
        let fns: Vec<_> = ord.fns().collect();
        assert!(fns[0].body().is_none() && fns[1].body().is_some());

        let Item::Impl(show) = &items[1] else {
            panic!("expected `impl`")
        };
        assert_eq!(
            show.trait_ref().unwrap().syntax().text().to_string(),
            "Show"
        );
        assert_eq!(show.self_ty().unwrap().text().to_string(), "List[A]");
        assert_eq!(show.fns().count(), 1);
    }
}
