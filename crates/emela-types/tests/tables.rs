//! lowering に渡す表（フィールドの添字，並べ直した引数，具体化した型引数，
//! 構成子のパターンのフィールド）をスナップショットで確かめる．
//!
//! `main.emel` の式とパターンだけを，ソースの位置の順に出す．

mod common;

use std::fmt::Write;

use emela_resolve::hir::{ExprId, Program, Span};
use emela_types::check_program;

use common::resolve_main;

fn text(src: &str, span: Span) -> &str {
    &src[usize::from(span.range.start())..usize::from(span.range.end())]
}

fn run(src: &str) -> String {
    let (program, module, _) = resolve_main(src);
    let types = check_program(&program);
    let cons = &types.ty_table.cons;
    let in_main = |span: Span| span.module == module;
    let expr_span = |program: &Program, e: ExprId| program.exprs[e].span;

    let mut lines: Vec<(u32, String)> = Vec::new();
    for (expr, index) in types.fields.iter() {
        let span = expr_span(&program, expr);
        if in_main(span) {
            lines.push((
                span.range.start().into(),
                format!("field `{}` -> {index}", text(src, span)),
            ));
        }
    }
    for (expr, slots) in types.calls.iter() {
        let span = expr_span(&program, expr);
        if in_main(span) {
            let args: Vec<String> = slots
                .iter()
                .map(|s| match s {
                    Some(e) => text(src, expr_span(&program, *e)).to_string(),
                    None => "_".to_string(),
                })
                .collect();
            lines.push((
                span.range.start().into(),
                format!("call `{}` -> [{}]", text(src, span), args.join(", ")),
            ));
        }
    }
    for (expr, args) in types.type_args.iter() {
        let span = expr_span(&program, expr);
        if in_main(span) {
            let args: Vec<String> = args.iter().map(|t| t.display(cons).to_string()).collect();
            lines.push((
                span.range.start().into(),
                format!("type args `{}` -> [{}]", text(src, span), args.join(", ")),
            ));
        }
    }
    for (pat, slots) in types.pat_fields.iter() {
        let span = program.pats[pat].span;
        if in_main(span) {
            let fields: Vec<String> = slots
                .iter()
                .map(|s| match s {
                    Some(p) => text(src, program.pats[*p].span).to_string(),
                    None => "_".to_string(),
                })
                .collect();
            lines.push((
                span.range.start().into(),
                format!("pattern `{}` -> [{}]", text(src, span), fields.join(", ")),
            ));
        }
    }
    lines.sort();
    let mut out = String::new();
    for (_, line) in lines {
        writeln!(out, "{line}").unwrap();
    }
    out
}

#[test]
fn tables() {
    insta::assert_snapshot!(run(r#"
type User(id: Int, name: String)
fn connect(host: String, port: Int) -> Bool { todo() }
fn add(x: Int, y: Int) -> Int { x }

fn f(u: User, o: Option[Int]) -> Int {
  a = u.name
  b = connect(port: 1, host: "h")
  c = 1 |> add(2)
  d = User(name: "b", ..u)
  e = Some("s")
  g = dbg(u)
  User(name: n, ..) = u
  h = match o {
    Some(x) -> x
    None -> 0
  }
  u.id
}
"#));
}
