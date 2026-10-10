//! 型付き AST．構文木のノードを種類ごとの型で包み，子を名前で取り出せるようにする．
//!
//! 中身は構文木そのもので，コピーも検証もしない．木は壊れていることがあるので，子を返す
//! 関数はどれも `Option` か空になりうるイテレータを返す．宣言は全部を型にする．
//! 式は主なものだけを型にし，残りは `Expr::Other` で返す．型とパターンはまだ型にしていない．

use crate::SyntaxKind::{self, *};
use crate::{SyntaxNode, SyntaxToken};

pub trait AstNode: Sized {
    fn can_cast(kind: SyntaxKind) -> bool;
    fn cast(node: SyntaxNode) -> Option<Self>;
    fn syntax(&self) -> &SyntaxNode;
}

macro_rules! ast_node {
    ($(#[$meta:meta])* $name:ident, $kind:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name($crate::SyntaxNode);

        impl $crate::ast::AstNode for $name {
            fn can_cast(kind: $crate::SyntaxKind) -> bool {
                kind == $crate::SyntaxKind::$kind
            }
            fn cast(node: $crate::SyntaxNode) -> Option<Self> {
                Self::can_cast(node.kind()).then(|| Self(node))
            }
            fn syntax(&self) -> &$crate::SyntaxNode {
                &self.0
            }
        }
    };
}

mod items;

pub use items::{
    Annotation, ConstDecl, DeriveClause, EffectDecl, EnumDecl, ErrorDecl, Field, FieldList,
    HandlerDecl, HasAttrs, ImplDecl, Import, Item, LayerDecl, OpSig, Path, TraitDecl, TypeDecl,
    Variant,
};

fn child<N: AstNode>(node: &SyntaxNode) -> Option<N> {
    node.children().find_map(N::cast)
}

fn children<N: AstNode>(node: &SyntaxNode) -> impl Iterator<Item = N> {
    node.children().filter_map(N::cast)
}

fn token(node: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() == kind)
}

ast_node!(
    /// ファイル全体．
    SourceFile,
    ROOT
);

impl SourceFile {
    pub fn items(&self) -> impl Iterator<Item = Item> {
        self.0.children().filter_map(Item::cast)
    }
    pub fn imports(&self) -> impl Iterator<Item = Import> {
        children(&self.0)
    }
    pub fn fn_decls(&self) -> impl Iterator<Item = FnDecl> {
        children(&self.0)
    }
}

ast_node!(
    /// `pub fn name[A](x: Int) -> T fails E use R { ... }`
    FnDecl,
    FN_DECL
);

impl FnDecl {
    pub fn is_suspend(&self) -> bool {
        token(&self.0, SUSPEND_KW).is_some()
    }
    pub fn name(&self) -> Option<SyntaxToken> {
        token(&self.0, LOWER_NAME)
    }
    pub fn type_params(&self) -> Option<SyntaxNode> {
        self.0.children().find(|n| n.kind() == TYPE_PARAM_LIST)
    }
    pub fn param_list(&self) -> Option<ParamList> {
        child(&self.0)
    }
    /// 戻り値の型のノード（`-> T` の `T`）．型はまだ型付きにしていない．
    pub fn ret_type(&self) -> Option<SyntaxNode> {
        self.0
            .children()
            .find(|n| n.kind() == RET_TYPE)?
            .first_child()
    }
    /// 本体．`@external` の宣言にはない．
    pub fn body(&self) -> Option<BlockExpr> {
        child(&self.0)
    }
}

ast_node!(
    /// `(x: Int, f)`
    ParamList,
    PARAM_LIST
);

impl ParamList {
    pub fn params(&self) -> impl Iterator<Item = Param> {
        children(&self.0)
    }
}

ast_node!(
    /// `x: Int` `self`
    Param,
    PARAM
);

impl Param {
    pub fn is_self(&self) -> bool {
        token(&self.0, SELF_KW).is_some()
    }
    pub fn name(&self) -> Option<SyntaxToken> {
        token(&self.0, LOWER_NAME)
    }
    /// 型注釈のノード．型はまだ型付きにしていない．
    pub fn ty(&self) -> Option<SyntaxNode> {
        self.0.first_child()
    }
}

ast_node!(
    /// `{ a = 1⏎ a + 1 }`
    BlockExpr,
    BLOCK_EXPR
);

impl BlockExpr {
    pub fn stmts(&self) -> impl Iterator<Item = Stmt> {
        self.0.children().filter_map(Stmt::cast)
    }
}

/// ブロックの中の文．
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Stmt {
    Binding(Binding),
    Expr(Expr),
}

impl Stmt {
    fn cast(node: SyntaxNode) -> Option<Self> {
        if node.kind() == BINDING {
            return Some(Stmt::Binding(Binding(node)));
        }
        Expr::cast(node).map(Stmt::Expr)
    }
}

ast_node!(
    /// `pairs: List[(String, Int)] = Map.to_list(scores)`
    Binding,
    BINDING
);

impl Binding {
    /// 左辺．式として読んであり，パターンとしての形は検査済み．
    pub fn lhs(&self) -> Option<Expr> {
        self.0.children().next().and_then(Expr::cast)
    }
    /// 型注釈のノード．
    pub fn ty(&self) -> Option<SyntaxNode> {
        let mut children = self.0.children().skip(1);
        token(&self.0, COLON).and_then(|_| children.next())
    }
    /// 右辺．
    pub fn value(&self) -> Option<Expr> {
        let skip = if token(&self.0, COLON).is_some() {
            2
        } else {
            1
        };
        self.0.children().nth(skip).and_then(Expr::cast)
    }
}

/// 式．主なものだけを型にし，残りは `Other` に入れる．
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Expr {
    Literal(Literal),
    NameRef(NameRef),
    BinExpr(BinExpr),
    CallExpr(CallExpr),
    BlockExpr(BlockExpr),
    IfExpr(IfExpr),
    Other(SyntaxNode),
}

impl Expr {
    pub fn cast(node: SyntaxNode) -> Option<Self> {
        Some(match node.kind() {
            LITERAL | STRING => Expr::Literal(Literal(node)),
            NAME_REF => Expr::NameRef(NameRef(node)),
            BIN_EXPR => Expr::BinExpr(BinExpr(node)),
            CALL_EXPR => Expr::CallExpr(CallExpr(node)),
            BLOCK_EXPR => Expr::BlockExpr(BlockExpr(node)),
            IF_EXPR => Expr::IfExpr(IfExpr(node)),
            PATH_EXPR | UNDERSCORE_EXPR | UNIT_EXPR | PAREN_EXPR | TUPLE_EXPR | LIST_EXPR
            | REST_EXPR | FIELD_EXPR | PREFIX_EXPR | USE_EXPR | FAIL_EXPR | ASSERT_EXPR
            | MATCH_EXPR | WITH_EXPR | ESCAPE_EXPR | LAMBDA_EXPR => Expr::Other(node),
            _ => return None,
        })
    }

    pub fn syntax(&self) -> &SyntaxNode {
        match self {
            Expr::Literal(e) => e.syntax(),
            Expr::NameRef(e) => e.syntax(),
            Expr::BinExpr(e) => e.syntax(),
            Expr::CallExpr(e) => e.syntax(),
            Expr::BlockExpr(e) => e.syntax(),
            Expr::IfExpr(e) => e.syntax(),
            Expr::Other(node) => node,
        }
    }
}

/// 数値か文字列のリテラル．
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Literal(SyntaxNode);

impl Literal {
    pub fn is_string(&self) -> bool {
        self.0.kind() == STRING
    }
    /// 数値のトークン．文字列なら `None`．
    pub fn number(&self) -> Option<SyntaxToken> {
        self.0
            .first_token()
            .filter(|t| matches!(t.kind(), INT | FLOAT))
    }
    pub fn syntax(&self) -> &SyntaxNode {
        &self.0
    }
}

ast_node!(
    /// `count` `MAX_SIZE` `self`
    NameRef,
    NAME_REF
);

impl NameRef {
    pub fn token(&self) -> Option<SyntaxToken> {
        self.0.first_token()
    }
}

ast_node!(
    /// `a + b`
    BinExpr,
    BIN_EXPR
);

impl BinExpr {
    pub fn lhs(&self) -> Option<Expr> {
        self.0.children().next().and_then(Expr::cast)
    }
    pub fn rhs(&self) -> Option<Expr> {
        self.0.children().nth(1).and_then(Expr::cast)
    }
    /// 演算子のトークン．
    pub fn op(&self) -> Option<SyntaxToken> {
        self.0
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| !t.kind().is_trivia())
    }
}

ast_node!(
    /// `f(x, port: 5432)`
    CallExpr,
    CALL_EXPR
);

impl CallExpr {
    pub fn callee(&self) -> Option<Expr> {
        self.0.children().next().and_then(Expr::cast)
    }
    /// 部分更新の `..式` の式．`User(name: "b", ..user)` の `user`．
    pub fn spread(&self) -> Option<Expr> {
        self.0
            .children()
            .find(|n| n.kind() == ARG_LIST)?
            .children()
            .find(|n| n.kind() == SPREAD_ARG)?
            .first_child()
            .and_then(Expr::cast)
    }

    /// 引数のノード．位置引数は式，名前付き引数は NAMED_ARG，末尾の `..式` は SPREAD_ARG．
    pub fn args(&self) -> impl Iterator<Item = SyntaxNode> {
        self.0
            .children()
            .find(|n| n.kind() == ARG_LIST)
            .into_iter()
            .flat_map(|list| list.children())
    }
}

ast_node!(
    /// `if c { a } else { b }`
    IfExpr,
    IF_EXPR
);

impl IfExpr {
    pub fn condition(&self) -> Option<Expr> {
        self.0.children().next().and_then(Expr::cast)
    }
    pub fn then_branch(&self) -> Option<BlockExpr> {
        child(&self.0)
    }
    /// `else` の後ろ．ブロックか，`else if` の IfExpr．
    pub fn else_branch(&self) -> Option<Expr> {
        token(&self.0, ELSE_KW)?;
        self.0.children().skip(2).find_map(Expr::cast)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    fn source_file(src: &str) -> SourceFile {
        let parse = parse(src);
        assert!(parse.diagnostics().is_empty(), "{}", parse.debug_dump());
        parse.tree()
    }

    #[test]
    fn fn_宣言の名前と引数と本体() {
        let file = source_file(
            "pub fn find(id: Int, self) -> User fails NotFound use Users { todo() }\n@external\nsuspend fn tick()",
        );
        let fns: Vec<_> = file.fn_decls().collect();
        assert_eq!(fns.len(), 2);

        let find = &fns[0];
        assert!(find.is_pub() && !find.is_suspend());
        assert_eq!(find.name().unwrap().text(), "find");
        let params: Vec<_> = find.param_list().unwrap().params().collect();
        assert_eq!(params[0].name().unwrap().text(), "id");
        assert_eq!(params[0].ty().unwrap().text().to_string(), "Int");
        assert!(params[1].is_self());
        assert_eq!(find.ret_type().unwrap().text().to_string(), "User");
        assert!(find.body().is_some());

        let tick = &fns[1];
        assert!(tick.is_suspend() && !tick.is_pub());
        assert!(tick.body().is_none());
    }

    #[test]
    fn 文と式() {
        let file = source_file(
            "fn f() {\n  pairs: List[Int] = g(x, port: 1)\n  if a > 0 { 1 } else if b { 2 }\n  count + 1\n}",
        );
        let body = file.fn_decls().next().unwrap().body().unwrap();
        let stmts: Vec<_> = body.stmts().collect();
        assert_eq!(stmts.len(), 3);

        let Stmt::Binding(binding) = &stmts[0] else {
            panic!("束縛のはず")
        };
        assert!(matches!(binding.lhs(), Some(Expr::NameRef(_))));
        assert_eq!(binding.ty().unwrap().text().to_string(), "List[Int]");
        let Some(Expr::CallExpr(call)) = binding.value() else {
            panic!("呼び出しのはず")
        };
        assert_eq!(call.callee().unwrap().syntax().text().to_string(), "g");
        assert_eq!(call.args().count(), 2);

        let Stmt::Expr(Expr::IfExpr(if_expr)) = &stmts[1] else {
            panic!("if のはず")
        };
        assert!(matches!(if_expr.condition(), Some(Expr::BinExpr(_))));
        assert!(if_expr.then_branch().is_some());
        assert!(matches!(if_expr.else_branch(), Some(Expr::IfExpr(_))));

        let Stmt::Expr(Expr::BinExpr(bin)) = &stmts[2] else {
            panic!("二項演算のはず")
        };
        assert_eq!(bin.op().unwrap().text(), "+");
        assert!(matches!(bin.lhs(), Some(Expr::NameRef(_))));
        let Some(Expr::Literal(one)) = bin.rhs() else {
            panic!("リテラルのはず")
        };
        assert_eq!(one.number().unwrap().text(), "1");
    }
}
