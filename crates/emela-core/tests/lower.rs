//! lowering（`emela_core::lower`）のテスト．

use std::path::Path;

use emela_core::{
    EnumDef, LowerError, LowerInput, Module, Roots, Type, VariantFields, dump, lower,
};
use emela_resolve::hir::Program;
use emela_resolve::{
    MemoryFs, ModuleId, ModuleMap, ModuleName, NoBuiltins, build_import_graph, collect_modules,
    imports, resolve,
};
use emela_types::check_program;
use la_arena::ArenaMap;

/// `files`（`src` からのパスとソース）を，同梱の Prelude と core のソースと一緒に名前解決する．
/// 先頭のファイルをエントリにする．構文と名前解決の誤りは許さない．
fn resolve_files<'s>(
    files: &[(&str, &'s str)],
) -> (Program, ModuleMap, ModuleId, ArenaMap<ModuleId, &'s str>) {
    let fs = MemoryFs::new(files.iter().map(|(path, _)| format!("src/{path}")));
    let (mut modules, diagnostics) = collect_modules(&fs, Path::new("src"));
    assert_eq!(diagnostics, []);
    let mut texts: ArenaMap<ModuleId, &str> = ArenaMap::default();
    for (path, src) in files {
        let (id, _) = modules
            .iter()
            .find(|(_, m)| m.file == Path::new("src").join(path))
            .unwrap();
        texts.insert(id, *src);
    }
    let entry = modules
        .iter()
        .find(|(_, m)| m.file == Path::new("src").join(files[0].0))
        .unwrap()
        .0;
    for (name, text) in emela_core::core_sources() {
        let (id, shadowed) =
            modules.add_core(ModuleName::parse(name), Path::new("<core>").join(name));
        assert_eq!(shadowed, None);
        texts.insert(id, text);
    }

    let mut trees = ArenaMap::default();
    let mut import_lists = ArenaMap::default();
    for (id, text) in texts.iter() {
        let parse = emela_syntax::parse(text);
        assert!(parse.diagnostics().is_empty(), "{:?}", parse.diagnostics());
        import_lists.insert(id, imports(&parse.tree()));
        trees.insert(id, parse.tree());
    }
    let (_, graph_diagnostics) = build_import_graph(&modules, &import_lists);
    assert_eq!(graph_diagnostics, []);
    let (program, diagnostics) = resolve(&modules, &trees, &NoBuiltins);
    // 同梱のソースの `@intrinsic` には E0130 が出る（emela-core の NOTES）ので，それだけ許す．
    let diagnostics: Vec<_> = diagnostics
        .iter()
        .filter(|d| !d.to_string().contains("E0130"))
        .collect();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    (program, modules, entry, texts)
}

fn lower_files(files: &[(&str, &str)]) -> Module {
    try_lower(files, Roots::Exported).unwrap_or_else(|e| panic!("{e:?}"))
}

/// 型検査まで通してから lowering する．誤りは `` `ソースの字面` what `` の形にする．
fn try_lower(files: &[(&str, &str)], roots: Roots) -> Result<Module, Vec<String>> {
    let (program, modules, entry, texts) = resolve_files(files);
    let types = check_program(&program);
    assert!(types.diags.is_empty(), "{:?}", types.diags);
    let input = LowerInput {
        hir: &program,
        types: &types,
        modules: &modules,
        sources: texts.clone(),
        entry,
        tests: Vec::new(),
    };
    let lowered = lower(&input, roots).map(|l| l.module);
    lowered.map_err(|errors| {
        errors
            .iter()
            .map(|LowerError::Unsupported { what, span }| {
                let Some(span) = span else {
                    return format!("<no span> {what}");
                };
                let range = usize::from(span.range.start())..usize::from(span.range.end());
                let text = &texts[span.module][range];
                if span.module == entry {
                    format!("`{text}` {what}")
                } else {
                    // 他のモジュールの誤りはモジュール名と，式の1行目だけを出す．
                    let first = text.lines().next().unwrap_or_default();
                    format!("{}: `{first}` {what}", modules[span.module].name)
                }
            })
            .collect()
    })
}

/// enum の定義を `Name[2] { Ctor(first: A, second: B), Unit }` の形にする．型引数は `A`，`B`，… と出す．
fn render_enum(m: &Module, def: &EnumDef) -> String {
    let variants: Vec<String> = def
        .variants
        .iter()
        .map(|v| {
            let tys = v.tys.iter().map(|t| render_ty(m, t));
            match &v.fields {
                VariantFields::Unit => v.name.clone(),
                VariantFields::Positional(_) => {
                    format!("{}({})", v.name, tys.collect::<Vec<_>>().join(", "))
                }
                VariantFields::Named(names) => {
                    let fields: Vec<String> = names
                        .iter()
                        .zip(tys)
                        .map(|(n, t)| format!("{n}: {t}"))
                        .collect();
                    format!("{}({})", v.name, fields.join(", "))
                }
            }
        })
        .collect();
    format!("{}[{}] {{ {} }}", def.name, def.params, variants.join(", "))
}

fn render_ty(m: &Module, ty: &Type) -> String {
    let list = |ts: &[Type]| {
        ts.iter()
            .map(|t| render_ty(m, t))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match ty {
        Type::Param(i) => char::from(b'A' + *i as u8).to_string(),
        Type::Tuple(ts) => format!("({})", list(ts)),
        Type::List(t) => format!("List[{}]", render_ty(m, t)),
        Type::Enum(id, ts) if ts.is_empty() => m.enums[*id].name.clone(),
        Type::Enum(id, ts) => format!("{}[{}]", m.enums[*id].name, list(ts)),
        other => format!("{other:?}"),
    }
}

fn enum_named<'m>(m: &'m Module, name: &str) -> &'m EnumDef {
    m.enums
        .iter()
        .map(|(_, e)| e)
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("enum `{name}` がない"))
}

fn fn_names(m: &Module) -> Vec<&str> {
    m.functions.iter().map(|(_, f)| f.name.as_str()).collect()
}

fn enum_names(m: &Module) -> Vec<&str> {
    m.enums.iter().map(|(_, e)| e.name.as_str()).collect()
}

#[test]
fn names_are_qualified_except_in_the_entry() {
    let m = lower_files(&[
        (
            "main.emel",
            "import Shapes\n\nfn main() { helper() }\n\nfn helper() -> List[Int] { Shapes.count() }\n\nfn unused() -> Int { 2 }\n",
        ),
        (
            "shapes.emel",
            "pub enum Shape { Circle(Float), Square(Float) }\n\npub fn count() -> List[Int] { List.range(0, 2) }\n",
        ),
    ]);
    // `main` からたどれる fn だけを写す．使われない `unused` と，他の core の関数は写さない．
    // 非公開の `List.range_onto` にもモジュール名が付く．
    let mut fns = fn_names(&m);
    fns.sort_unstable();
    assert_eq!(
        fns,
        [
            "List.range",
            "List.range_onto",
            "Shapes.count",
            "helper",
            "main"
        ]
    );

    let enums = enum_names(&m);
    assert!(enums.contains(&"Shapes.Shape"), "{enums:?}");
    // Option は1つだけ，Bool は IR の enum にしない．
    assert_eq!(
        enums.iter().filter(|&&e| e == "Option").count(),
        1,
        "{enums:?}"
    );
    assert!(!enums.iter().any(|e| e.ends_with("Bool")), "{enums:?}");
}

#[test]
fn entry_exports_pub_fns_and_main() {
    let m = lower_files(&[(
        "main.emel",
        "fn main() { 0 }\n\npub fn api() -> Int { 1 }\n\nfn private() -> Int { 2 }\n",
    )]);
    let exported: Vec<&str> = m
        .functions
        .iter()
        .filter(|(_, f)| f.exported)
        .map(|(_, f)| f.name.as_str())
        .collect();
    assert_eq!(exported, ["main", "api"]);
}

#[test]
fn ctors_carry_field_names_and_types() {
    let m = lower_files(&[(
        "main.emel",
        "\
type Pair[A, B](first: A, second: B)

enum Shape {
  Circle(Float),
  Rect(width: Float, height: Float),
  Empty,
}

type Wrapper(items: List[Option[Shape]], pair: Pair[Int, (Bool, String)])

error NotFound(id: Int)

fn main() { 0 }
",
    )]);
    let rendered: Vec<String> = ["Pair", "Shape", "Wrapper", "NotFound", "Option"]
        .iter()
        .map(|name| render_enum(&m, enum_named(&m, name)))
        .collect();
    assert_eq!(
        rendered,
        [
            "Pair[2] { Pair(first: A, second: B) }",
            "Shape[0] { Circle(Float), Rect(width: Float, height: Float), Empty }",
            "Wrapper[0] { Wrapper(items: List[Option[Shape]], pair: Pair[Int, (Bool, String)]) }",
            "NotFound[0] { NotFound(id: Int) }",
            "Option[1] { Some(A), None }",
        ]
    );
}

#[test]
fn dbg_of_arithmetic() {
    let m = lower_files(&[(
        "main.emel",
        "\
fn main() {
  x = 1 + 2 * 3
  big: Int64 = 0x7FFF_FFFF_FF
  _ = dbg(x)
  dbg(-big)
  greet(name: \"emela\", times: 2)
}

fn greet(times: Int, name: String) -> Bool {
  if times > 0 && String.length(name) != 0 { True } else { False }
}
",
    )]);
    insta::assert_snapshot!(dump(&m));
}

#[test]
fn unsupported_expressions_are_all_reported() {
    let errors = try_lower(
        &[(
            "main.emel",
            "\
const LIMIT: Int = 3

fn main() {
  assert LIMIT == 3
  show_any(1)
}

fn show_any[A](x: A) -> String {
  \"x = #{x}\"
}
",
        )],
        Roots::Exported,
    )
    .unwrap_err();
    assert_eq!(errors, ["`x` showing a value of a type parameter"]);
}

/// 全部の fn を根にして，core の関数が全部写せることを確かめる．たどれない fn は通常の
/// ビルドでは写さないので，使われない core の関数の誤りはここでしか見つからない．
#[test]
fn every_core_function_lowers() {
    let m = try_lower(&[("main.emel", "fn main() { 0 }\n")], Roots::All)
        .unwrap_or_else(|errors| panic!("{}", errors.join("\n")));
    assert!(fn_names(&m).contains(&"List.map"));
}

#[test]
fn match_and_binding_patterns() {
    let m = lower_files(&[(
        "main.emel",
        "\
enum Shape {
  Circle(Float)
  Rect(width: Float, height: Float)
  Empty
}

type Pair[A, B](first: A, second: B)

fn main() {
  dbg(area(Rect(height: 2.0, width: 3.0)))
  dbg(describe([1, 2, 3]))
  (a, b) = (1, \"x\")
  Pair(second:, ..) = Pair(first: a, second: b)
  dbg(second)
}

fn area(s: Shape) -> Float {
  match s {
    Circle(r) -> r * r * 3.0
    Rect(width: w, ..) if w > 10.0 -> 0.0
    Rect(width: w, height: h) -> w * h
    Empty -> 0.0
  }
}

fn describe(xs: List[Int]) -> String {
  match xs {
    [] -> \"empty\"
    [1, ..] -> \"one first\"
    [_, ..rest] -> if rest == [] { \"single\" } else { \"many\" }
  }
}
",
    )]);
    insta::assert_snapshot!(dump(&m));
}

#[test]
fn lambdas_and_wrapped_functions() {
    let m = lower_files(&[(
        "main.emel",
        "\
fn main() {
  k = 10
  add_k = fn(x) { x + k }
  dbg(add_k(1))
  dbg(List.map([1, 2], Some))
  dbg(List.map([\"a\", \"bc\"], String.length))
  dbg(apply(f: add_k, x: 2))
}

fn apply(x: Int, f: fn(Int) -> Int) -> Int {
  f(x)
}
",
    )]);
    let out = dump(&m);
    // core の関数は長いので，エントリのモジュールの関数だけを見る．
    let mine: Vec<&str> = out
        .split("\n\n")
        .filter(|f| f.contains("fn main(") || f.contains("fn apply("))
        .collect();
    insta::assert_snapshot!(mine.join("\n\n"));
}

#[test]
fn fields_updates_and_interpolation() {
    let m = lower_files(&[(
        "main.emel",
        "\
type User(id: Int, name: String, tags: List[String])

type Pair[A, B](first: A, second: B)

fn main() {
  u = User(id: 1, name: \"a\", tags: [])
  renamed = User(tags: [\"x\"], name: String.concat(u.name, \"!\"), ..u)
  p = Pair(first: 1, second: True)
  q = Pair(first: \"s\", ..p)
  \"#{renamed.name} has #{renamed.tags} and #{q}, id #{u.id}\"
}
",
    )]);
    let out = dump(&m);
    let main = out.split("\n\n").find(|f| f.contains("fn main(")).unwrap();
    insta::assert_snapshot!(main);
}

#[test]
fn asserts_and_consts() {
    let m = lower_files(&[(
        "main.emel",
        "\
const LIMIT: Int = 3
const ORIGIN: (Int, Int) = (0, 0)
const EMPTY = []

fn main() {
  assert LIMIT == 3
  assert classify(2) != \"big\"
  assert ORIGIN == (0, 0)
  assert List.length(EMPTY) == 0
  assert same(1, 1)
  assert is_small(LIMIT)
}

fn classify(n: Int) -> String {
  match n {
    LIMIT -> \"limit\"
    0 -> \"zero\"
    m if m > 100 -> \"big\"
    _ -> \"other\"
  }
}

fn same[A](a: A, b: A) -> Bool {
  assert a == b
  True
}

fn is_small(n: Int) -> Bool {
  n < LIMIT + 1
}
",
    )]);
    let out = dump(&m);
    let mine: Vec<&str> = out
        .split("\n\n")
        .filter(|f| {
            ["main(", "classify(", "same(", "LIMIT(", "ORIGIN(", "EMPTY("]
                .iter()
                .any(|name| f.contains(&format!("fn {name}")))
        })
        .collect();
    insta::assert_snapshot!(mine.join("\n\n"));
}
