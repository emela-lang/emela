//! Snapshot tests for declared types (`Signatures::collect`).
//!
//! Prints the types and parameter labels of fns, constructors and consts. Examples come from spec
//! chapter 5, 6.2 and 7.1.

mod common;

use common::render_signatures;

fn run(src: &str) -> String {
    render_signatures(src, true, |_, _| true)
}

#[test]
fn fn_labels() {
    insta::assert_snapshot!(run(r#"
fn connect(host: String, port: Int) -> () use {} { todo() }
fn map[A, B](xs: List[A], f: fn(A) -> B) -> List[B] use {} { todo() }
fn inferred(x: Int) { x }
"#));
}

#[test]
fn type_ctors() {
    insta::assert_snapshot!(run(r#"
type User(id: Int, name: String)
type Pair[A, B](first: A, second: B)
"#));
}

#[test]
fn enum_variants() {
    insta::assert_snapshot!(run(r#"
enum Shape {
  Circle(radius: Float)
  Rect(Float, Float)
  Empty
}
enum Tree[A] {
  Leaf
  Node(Tree[A], A, Tree[A])
}
"#));
}

#[test]
fn errors_and_consts() {
    insta::assert_snapshot!(run(r#"
error NotFound(id: Int)
error Timeout
const MAX_RETRY: Int = 3
const INFERRED = 3
"#));
}

#[test]
fn prelude_ctors() {
    // Prelude constructors are not definitions of main.emel, so look them up directly.
    use emela_types::{Signatures, TyTable};
    let (program, _, _) = common::resolve_main("");
    let table = TyTable::new(&program);
    let sigs = Signatures::collect(&program, &table, &mut Vec::new());
    let mut out = String::new();
    for name in ["Some", "None", "True", "False"] {
        let def = program.prelude_def(name).unwrap();
        let sig = sigs.get(def).unwrap();
        out.push_str(&format!("{name}: {}\n", sig.scheme.display(&table.cons)));
    }
    insta::assert_snapshot!(out);
}
