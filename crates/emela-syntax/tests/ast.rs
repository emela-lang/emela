//! Uses the typed AST through the public API.

use emela_syntax::ast::{AstNode, Expr, ParamList, Stmt};
use emela_syntax::parse;

#[test]
fn 部分更新の_spread() {
    let parse = parse("fn f() {\n  User(name: \"b\", ..user)\n  g(x)\n}");
    assert!(parse.diagnostics().is_empty(), "{}", parse.debug_dump());
    let body = parse.tree().fn_decls().next().unwrap().body().unwrap();
    let calls: Vec<_> = body
        .stmts()
        .map(|s| match s {
            Stmt::Expr(Expr::CallExpr(call)) => call,
            _ => panic!("expected a call"),
        })
        .collect();
    let spread = calls[0].spread().expect("expected `..user`");
    assert_eq!(spread.syntax().text().to_string(), "user");
    assert_eq!(calls[0].args().count(), 2);
    assert!(calls[1].spread().is_none());
}

#[test]
fn 無名関数の引数のパターン() {
    let parse = parse("fn f() {\n  g = |x: Int, (a, b): (Int, Int), _| a\n}");
    assert!(parse.diagnostics().is_empty(), "{}", parse.debug_dump());
    let list = parse
        .tree()
        .syntax()
        .descendants()
        .filter_map(ParamList::cast)
        .last()
        .unwrap();
    let params: Vec<(String, String, String)> = list
        .params()
        .map(|p| {
            let text = |n: Option<String>| n.unwrap_or_default();
            (
                text(p.name().map(|t| t.text().to_owned())),
                text(p.pattern().map(|n| n.text().to_string())),
                text(p.ty().map(|n| n.text().to_string())),
            )
        })
        .collect();
    let s = |a: &str, b: &str, c: &str| (a.to_owned(), b.to_owned(), c.to_owned());
    assert_eq!(
        params,
        [
            s("x", "", "Int"),
            s("", "(a, b)", "(Int, Int)"),
            s("", "_", ""),
        ]
    );
}

#[test]
fn 局所関数は文になる() {
    let src = "fn f() -> Int {\n  ## 説明\n  fn go(n: Int) -> Int { n }\n  go(1)\n}";
    let parse = parse(src);
    assert!(parse.diagnostics().is_empty(), "{}", parse.debug_dump());
    let body = parse.tree().fn_decls().next().unwrap().body().unwrap();
    let stmts: Vec<_> = body.stmts().collect();
    let Stmt::LocalFn(local) = &stmts[0] else {
        panic!("expected a local fn: {stmts:?}");
    };
    assert_eq!(local.name().unwrap().text(), "go");
    assert_eq!(local.param_list().unwrap().params().count(), 1);
    assert_eq!(local.ret_type().unwrap().text().to_string(), "Int");
    assert_eq!(local.body().unwrap().syntax().text().to_string(), "{ n }");
    assert!(local.syntax().text().to_string().starts_with("## 説明"));
    assert!(matches!(stmts[1], Stmt::Expr(Expr::CallExpr(_))));
    // A local fn is not a declaration of the module.
    assert_eq!(parse.tree().fn_decls().count(), 1);
}
