//! Snapshot tests for converting type expressions (`TyTable::lower`).
//!
//! For each top-level fn of one module (`main.emel`), prints the declared type (`Signatures`) as
//! `∀ A. fn(List[A]) -> Option[A]`, quantifying the fn's type parameters.

mod common;

use emela_resolve::hir::DefKind;

use common::render_signatures;

/// Test fns write a return type and `use {}`; fns without them get no declared type and are not
/// shown (spec 9.3).
fn run(src: &str) -> String {
    render_signatures(src, false, |program, def| {
        program.defs[def].kind == DefKind::Fn
    })
}

#[test]
fn smoke() {
    insta::assert_snapshot!(run("fn f(x: Int) -> Int use {} { x }\n"));
}

#[test]
fn builtin_types() {
    insta::assert_snapshot!(run(r#"
fn prims(a: Int, b: Int64, c: Float, d: String, e: Bool) -> () use {} { todo() }
fn never() -> Never use {} { todo() }
fn tuple(p: (Int, String)) -> (Int, (Bool, Float)) use {} { todo() }
fn func(f: fn(Int, String) -> Bool) -> fn() -> () fails {} use {} use {} { todo() }
fn list(xs: List[Int]) -> List[List[String]] use {} { todo() }
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

fn user(u: User) -> Pair[User, Shape] use {} { todo() }
fn option(x: Option[Int]) -> Option[Option[String]] use {} { todo() }
fn err(e: NotFound) -> NotFound use {} { todo() }
"#));
}

#[test]
fn type_params() {
    insta::assert_snapshot!(run(r#"
fn id[A](x: A) -> A use {} { x }
fn swap[A, B](p: (A, B)) -> (B, A) use {} { todo() }
fn map[A, B](xs: List[A], f: fn(A) -> B) -> List[B] use {} { todo() }
"#));
}

#[test]
fn wrong_type_arg_count() {
    insta::assert_snapshot!(run(r#"
type Pair[A, B](first: A, second: B)

fn too_many(x: List[Int, Int]) -> Int[String] use {} { todo() }
fn too_few(x: List) -> Option use {} { todo() }
fn pair(p: Pair[Int]) -> Bool[Int] use {} { todo() }
fn param[Item](x: Item[Int]) -> Int use {} { todo() }
"#));
}

#[test]
fn errors_do_not_cascade() {
    insta::assert_snapshot!(run(r#"
fn undefined(x: Nope) -> List[Nope] use {} { todo() }
"#));
}
