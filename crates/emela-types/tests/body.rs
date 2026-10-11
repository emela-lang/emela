//! fn の本体の推論（`infer_fn_body`）をスナップショットで確かめる．
//!
//! fn ごとに，fn の型と，引数と束縛した変数の型を出す．その下に型の診断を出す．

mod common;

use std::fmt::Write;

use emela_resolve::hir::{DefKind, DefModule};
use emela_types::{Signatures, TyTable, infer_fn_body};

use common::{describe, resolve_main};

fn run(src: &str) -> String {
    let (program, module, resolve_diagnostics) = resolve_main(src);
    let table = TyTable::new(&program);
    let mut diags = Vec::new();
    let sigs = Signatures::collect(&program, &table, &mut diags);

    let mut out = String::new();
    for (def, data) in program.defs.iter() {
        if data.module != DefModule::Source(module) || data.kind != DefKind::Fn {
            continue;
        }
        let Some(body) = infer_fn_body(&program, &table, &sigs, def) else {
            continue;
        };
        writeln!(out, "{}: {}", data.name, body.fn_ty.display(&table.cons)).unwrap();
        for (local, ty) in body.locals.iter() {
            writeln!(
                out,
                "  {}: {}",
                program.locals[local].name,
                ty.display(&table.cons)
            )
            .unwrap();
        }
        diags.extend(body.diags);
    }
    for d in &resolve_diagnostics {
        writeln!(out, "resolve: {d}").unwrap();
    }
    for d in &diags {
        writeln!(out, "{}", describe(src, &table.cons, d)).unwrap();
    }
    out
}

#[test]
fn literals_and_bindings() {
    insta::assert_snapshot!(run(r#"
fn f(n: Int) -> (Int, String) {
  a = 1
  b = 2.5
  c = "s"
  d = (a, c)
  e = ()
  (n, c)
}
"#));
}

#[test]
fn inferred_return() {
    insta::assert_snapshot!(run(r#"
fn g(x: Int, s: String) {
  y = x
  (y, s, 1.0)
}
fn unit() {
  x = 1
}
"#));
}

#[test]
fn annotations() {
    insta::assert_snapshot!(run(r#"
fn f() -> Int {
  x: Int = 1
  y: String = 2
  x
}
"#));
}

#[test]
fn return_mismatch() {
    insta::assert_snapshot!(run(r#"
fn f(x: Int) -> String {
  x
}
fn id[A](x: A) -> A {
  1
}
fn ok[A](x: A) -> A {
  y = x
  y
}
"#));
}

#[test]
fn rebinding() {
    insta::assert_snapshot!(run(r#"
fn f() -> String {
  x = 1
  x = "now a string"
  x
}
"#));
}
