//! 宣言（17.3）の型付き AST．

use super::{AstNode, BlockExpr, Expr, FnDecl, ParamList, child, children, token};
use crate::SyntaxKind::*;
use crate::{SyntaxNode, SyntaxToken};

/// トップレベルの項目．
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

/// 注釈，ドキュメントコメント，`pub` を持つ宣言．
pub trait HasAttrs: AstNode {
    fn annotations(&self) -> impl Iterator<Item = Annotation> {
        children(self.syntax())
    }
    /// ドキュメントコメントを行ごとに．`##` は含めたまま返す．
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
    /// `@name` の注釈が付いているか．
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

/// 宣言の名前．キーワードの後ろの最初の名前のトークン．
fn name(node: &SyntaxNode) -> Option<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| matches!(t.kind(), LOWER_NAME | TYPE_NAME | UPPER_NAME))
}

/// 直下の PATH を順に．
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
    /// `.{...}` で取り込む名前．`.{...}` がなければ空．
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
    /// `.` で区切られた名前．
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
    /// 引数のノード（ANNOT_ARG）．
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
    /// フィールドの並び．`@external` の型にはない．
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
    /// 名前付きのフィールド．
    pub fn field_list(&self) -> Option<FieldList> {
        child(&self.0)
    }
    /// 名前なしのフィールドの型のノード．
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
    /// 型のノード．
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
    /// 型注釈のノード．
    pub fn ty(&self) -> Option<SyntaxNode> {
        token(&self.0, COLON)?;
        self.0.children().find(|n| n.kind() != ANNOTATION)
    }
    /// 右辺．
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
    pub fn ops(&self) -> impl Iterator<Item = OpSig> {
        items(&self.0)
    }
}

ast_node!(
    /// effect の操作．`suspend fn sleep(ms: Int) -> ()`
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
    pub fn param_list(&self) -> Option<ParamList> {
        child(&self.0)
    }
    /// 戻り値の型のノード．
    pub fn ret_type(&self) -> Option<SyntaxNode> {
        self.0
            .children()
            .find(|n| n.kind() == RET_TYPE)?
            .first_child()
    }
}

ast_node!(
    /// `handler PostgresUsers(conn: Connection) implements Users { ... }`
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
    /// `implements` の対象．
    pub fn effect(&self) -> Option<Path> {
        child(&self.0.children().find(|n| n.kind() == IMPLEMENTS_CLAUSE)?)
    }
    pub fn init(&self) -> Option<BlockExpr> {
        item_block(&self.0, HANDLER_INIT)
    }
    pub fn release(&self) -> Option<BlockExpr> {
        item_block(&self.0, HANDLER_RELEASE)
    }
    /// 操作の実装．
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
    pub fn handlers(&self) -> impl Iterator<Item = Path> {
        items(&self.0)
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
    /// 前提とする Trait．`trait Ord: Eq` の `Eq`．
    pub fn supertraits(&self) -> impl Iterator<Item = Path> {
        paths(&self.0)
    }
    pub fn fns(&self) -> impl Iterator<Item = FnDecl> {
        items(&self.0)
    }
}

ast_node!(
    /// `impl Show for User { ... }`
    ImplDecl,
    IMPL_DECL
);

impl ImplDecl {
    /// 実装する Trait．
    pub fn trait_ref(&self) -> Option<Path> {
        child(&self.0)
    }
    /// 実装する型のノード（`for` の後ろ）．
    pub fn self_ty(&self) -> Option<SyntaxNode> {
        self.0
            .children()
            .find(|n| !matches!(n.kind(), PATH | ITEM_LIST | ANNOTATION))
    }
    pub fn fns(&self) -> impl Iterator<Item = FnDecl> {
        items(&self.0)
    }
}

/// 宣言の本体（ITEM_LIST）の中の項目．
fn items<N: AstNode>(node: &SyntaxNode) -> impl Iterator<Item = N> {
    node.children()
        .find(|n| n.kind() == ITEM_LIST)
        .into_iter()
        .flat_map(|list| children(&list).collect::<Vec<_>>())
}

/// handler の `init` と `release` のブロック．
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
            panic!("import のはず")
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
            panic!("fn のはず")
        };
        assert!(add.is_pub() && add.has_annotation("external"));
        assert_eq!(add.annotations().next().unwrap().args().count(), 2);
        let docs: Vec<_> = add.doc_comments().map(|t| t.to_string()).collect();
        assert_eq!(docs, ["## 足す", "## 2行目"]);
    }

    #[test]
    fn データの宣言() {
        let items = items(
            "pub opaque type User(id: Int, name: String)\n  derive Eq, Show\nenum Shape {\n  Circle(radius: Float)\n  Rect(Float, Float)\n  Empty\n}\nerror NotFound(id: Int)\nconst MAX: Int = 3",
        );
        let Item::Type(user) = &items[0] else {
            panic!("type のはず")
        };
        assert!(user.is_pub() && user.is_opaque());
        assert_eq!(text(user.name()), "User");
        let fields: Vec<_> = user.field_list().unwrap().fields().collect();
        assert_eq!(text(fields[1].name()), "name");
        assert_eq!(fields[1].ty().unwrap().text().to_string(), "String");
        assert_eq!(user.derive().unwrap().traits().count(), 2);

        let Item::Enum(shape) = &items[1] else {
            panic!("enum のはず")
        };
        let variants: Vec<_> = shape.variants().collect();
        assert_eq!(variants.len(), 3);
        assert_eq!(variants[0].field_list().unwrap().fields().count(), 1);
        assert_eq!(variants[1].tuple_fields().count(), 2);
        assert!(variants[2].field_list().is_none());

        let Item::Error(not_found) = &items[2] else {
            panic!("error のはず")
        };
        assert_eq!(text(not_found.name()), "NotFound");

        let Item::Const(max) = &items[3] else {
            panic!("const のはず")
        };
        assert_eq!(text(max.name()), "MAX");
        assert_eq!(max.ty().unwrap().text().to_string(), "Int");
        assert!(matches!(max.value(), Some(Expr::Literal(_))));
    }

    #[test]
    fn derive_の前のドキュメントコメントは型に付かない() {
        let items = items(
            "## 利用者\ntype User(id: Int)\n  # 比較\n\n  ## 導出\n  derive Eq, Show\nenum Color {\n  Red\n}\n  # 表示\n  derive Show",
        );
        let Item::Type(user) = &items[0] else {
            panic!("type のはず")
        };
        let docs: Vec<_> = user.doc_comments().map(|t| t.to_string()).collect();
        assert_eq!(docs, ["## 利用者"]);
        assert_eq!(user.derive().unwrap().traits().count(), 2);

        let Item::Enum(e) = &items[1] else {
            panic!("enum のはず")
        };
        assert_eq!(e.derive().unwrap().traits().count(), 1);
    }

    #[test]
    fn 効果とハンドラ() {
        let items = items(
            "effect Clock {\n  suspend fn sleep(ms: Int) -> ()\n  fn now() -> Instant\n}\nhandler Pg(conn: Conn) implements Users {\n  init { Pg(conn: open()) }\n  release { close(self.conn) }\n  fn find(id) { 1 }\n}\nlayer App { Env, Pg }",
        );
        let Item::Effect(clock) = &items[0] else {
            panic!("effect のはず")
        };
        let ops: Vec<_> = clock.ops().collect();
        assert!(ops[0].is_suspend() && !ops[1].is_suspend());
        assert_eq!(ops[1].ret_type().unwrap().text().to_string(), "Instant");

        let Item::Handler(pg) = &items[1] else {
            panic!("handler のはず")
        };
        assert_eq!(pg.effect().unwrap().syntax().text().to_string(), "Users");
        assert!(pg.init().is_some() && pg.release().is_some());
        assert_eq!(text(pg.fns().next().unwrap().name()), "find");

        let Item::Layer(app) = &items[2] else {
            panic!("layer のはず")
        };
        assert_eq!(app.handlers().count(), 2);
    }

    #[test]
    fn トレイトと実装() {
        let items = items(
            "trait Ord: Eq + Show {\n  fn compare(self, other: Self) -> Ordering\n  fn max(self, other: Self) -> Self { self }\n}\nimpl Show for List[Int] {\n  fn show(self) { \"\" }\n}",
        );
        let Item::Trait(ord) = &items[0] else {
            panic!("trait のはず")
        };
        assert_eq!(ord.supertraits().count(), 2);
        let fns: Vec<_> = ord.fns().collect();
        assert!(fns[0].body().is_none() && fns[1].body().is_some());

        let Item::Impl(show) = &items[1] else {
            panic!("impl のはず")
        };
        assert_eq!(
            show.trait_ref().unwrap().syntax().text().to_string(),
            "Show"
        );
        assert_eq!(show.self_ty().unwrap().text().to_string(), "List[Int]");
        assert_eq!(show.fns().count(), 1);
    }
}
