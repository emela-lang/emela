//! 型の書き方の変換（`TyTable::lower`）をスナップショットで確かめる．
//!
//! 1つのモジュール（`main.emel`）のトップレベルの fn ごとに，引数と戻り値の型を変換して
//! `∀ A. fn(List[A]) -> Option[A]` の形で出す．型引数は fn の型引数．

use std::fmt::Write;
use std::path::Path;

use emela_resolve::hir::{Item, Program};
use emela_resolve::{MemoryFs, NoBuiltins, build_import_graph, collect_modules, imports, resolve};
use emela_types::{Diagnostic, Scheme, Ty, TyParam, TyTable};
use la_arena::ArenaMap;

/// `main.emel` を名前解決し，fn のシグネチャの変換結果と診断を返す．構文の誤りは許さない．
fn run(src: &str) -> String {
    let fs = MemoryFs::new(["src/main.emel".to_string()]);
    let (modules, diagnostics) = collect_modules(&fs, Path::new("src"));
    assert_eq!(diagnostics, []);
    let (module, _) = modules.iter().next().unwrap();
    let parse = emela_syntax::parse(src);
    assert!(parse.diagnostics().is_empty(), "{:?}", parse.diagnostics());
    let mut trees = ArenaMap::default();
    let mut import_lists = ArenaMap::default();
    import_lists.insert(module, imports(&parse.tree()));
    trees.insert(module, parse.tree());
    let (_, graph_diagnostics) = build_import_graph(&modules, &import_lists);
    assert_eq!(graph_diagnostics, []);
    let (program, resolve_diagnostics) = resolve(&modules, &trees, &NoBuiltins);

    let table = TyTable::new(&program);
    let mut diags = Vec::new();
    let mut out = String::new();
    for &def in &program.modules[module].items {
        let Some(Item::Fn(item)) = program.item(def) else {
            continue;
        };
        let sig = &item.sig;
        let params: Vec<_> = sig.type_params.iter().map(|tp| tp.def).collect();
        let arg_tys: Vec<Ty> = sig
            .params
            .iter()
            .map(|p| {
                let ty = p.ty.expect("トップレベルの fn の引数は型を書く");
                table.lower(&program, &params, ty, &mut diags)
            })
            .collect();
        let ret = sig.ret.expect("テストの fn は戻り値の型を書く");
        let ret_ty = table.lower(&program, &params, ret, &mut diags);
        let scheme = Scheme::new(
            params
                .iter()
                .map(|&tp| TyParam::new(program.defs[tp].name.clone()))
                .collect(),
            Ty::func(arg_tys, ret_ty),
        );
        writeln!(
            out,
            "{}: {}",
            program.defs[def].name,
            scheme.display(&table.cons)
        )
        .unwrap();
    }

    for d in &resolve_diagnostics {
        writeln!(out, "resolve: {d}").unwrap();
    }
    for d in &diags {
        writeln!(out, "{}", describe(&program, src, d)).unwrap();
    }
    out
}

/// 診断を `` `List[Int, Int]` WrongTypeArgCount { ... } `` の形にする．
fn describe(_program: &Program, src: &str, d: &Diagnostic) -> String {
    let text = d.span.map_or("<no span>", |s| {
        let range = s.range;
        &src[usize::from(range.start())..usize::from(range.end())]
    });
    format!("`{text}` {:?}", d.kind)
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
