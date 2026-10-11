//! 型の書き方の変換（`TyTable::lower`）をスナップショットで確かめる．
//!
//! 1つのモジュール（`main.emel`）のトップレベルの fn ごとに，宣言の型（`Signatures`）を
//! `∀ A. fn(List[A]) -> Option[A]` の形で出す．型引数は fn の型引数．

mod common;

use emela_resolve::hir::DefKind;

use common::render_signatures;

/// `main.emel` のトップレベルの fn ごとに，引数と戻り値の型の変換結果と診断を返す．
/// テストの fn は戻り値の型を書く（書かない fn は宣言の型を作らないので出ない）．
fn run(src: &str) -> String {
    render_signatures(src, false, |program, def| {
        program.defs[def].kind == DefKind::Fn
    })
}

#[test]
fn smoke() {
    insta::assert_snapshot!(run("fn f(x: Int) -> Int { x }\n"));
}

#[test]
fn builtin_types() {
    insta::assert_snapshot!(run(r#"
fn prims(a: Int, b: Int64, c: Float, d: String, e: Bool) -> () { todo() }
fn never() -> Never { todo() }
fn tuple(p: (Int, String)) -> (Int, (Bool, Float)) { todo() }
fn func(f: fn(Int, String) -> Bool) -> fn() -> () { todo() }
fn list(xs: List[Int]) -> List[List[String]] { todo() }
"#));
}

#[test]
fn declared_types() {
    insta::assert_snapshot!(run(r#"
type User(id: Int, name: String)
type Pair[A, B](first: A, second: B)
enum Shape {
  Circle(radius: Float)
  Empty
}
error NotFound(id: Int)

fn user(u: User) -> Pair[User, Shape] { todo() }
fn option(x: Option[Int]) -> Option[Option[String]] { todo() }
fn err(e: NotFound) -> NotFound { todo() }
"#));
}

#[test]
fn type_params() {
    insta::assert_snapshot!(run(r#"
fn id[A](x: A) -> A { x }
fn swap[A, B](p: (A, B)) -> (B, A) { todo() }
fn map[A, B](xs: List[A], f: fn(A) -> B) -> List[B] { todo() }
"#));
}

#[test]
fn wrong_type_arg_count() {
    insta::assert_snapshot!(run(r#"
type Pair[A, B](first: A, second: B)

fn too_many(x: List[Int, Int]) -> Int[String] { todo() }
fn too_few(x: List) -> Option { todo() }
fn pair(p: Pair[Int]) -> Bool[Int] { todo() }
fn param[Item](x: Item[Int]) -> Int { todo() }
"#));
}

#[test]
fn errors_do_not_cascade() {
    insta::assert_snapshot!(run(r#"
fn undefined(x: Nope) -> List[Nope] { todo() }
"#));
}
