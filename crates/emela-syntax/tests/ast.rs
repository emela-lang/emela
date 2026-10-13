//! Uses the typed AST through the public API.

use emela_syntax::ast::{Expr, Stmt};
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
