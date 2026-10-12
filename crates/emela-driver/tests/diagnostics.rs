//! 診断の収集と表示．色なしの出力をスナップショットで見る．

use std::path::Path;

use emela_driver::{
    Analysis, Checked, CoreSource, Diagnostic, FileId, Frontend, Input, LexOnly, Location, Lower,
    MemoryFiles, ParseOnly, Parsed, Resolve, SourceDb, SourceFile, Span, Typecheck, check, render,
};
use emela_resolve::{Import, ModuleId, ModuleName};
use emela_types::{InferCtx, Prim, Ty, TyCons};
use line_index::{TextRange, TextSize};

fn check_files(files: &[(&str, &str)], path: &str, frontend: &mut impl Frontend) -> Analysis {
    let fs = MemoryFiles::new(files.iter().copied());
    let input = Input::resolve(&fs, Path::new(path)).unwrap();
    check(&fs, &input, frontend).0
}

#[test]
fn collects_every_broken_file() {
    let analysis = check_files(
        &[
            ("app/Pome.toml", ""),
            ("app/src/main.emel", "let x = \"abc\nlet y = $\n"),
            ("app/src/http/client.emel", "let n = 3px\n"),
            ("app/src/http/Server.emel", "let ok = 1\n"),
            ("app/src/My-Lib/util.emel", "let ok = 1\n"),
            ("app/src/json.emel", "let fine = 1\n"),
        ],
        "app",
        &mut LexOnly,
    );
    assert_eq!(analysis.error_count(), 5);
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

#[test]
fn clean_project_has_no_output() {
    let analysis = check_files(
        &[("app/Pome.toml", ""), ("app/src/main.emel", "let x = 1\n")],
        "app",
        &mut LexOnly,
    );
    assert!(analysis.diagnostics.is_empty());
    assert_eq!(render(&analysis.diagnostics, &analysis.sources, false), "");
    assert!(analysis.entry.is_some());
}

#[test]
fn invalid_entry_name() {
    let analysis = check_files(
        &[("app/Pome.toml", ""), ("app/src/Main.emel", "")],
        "app/src/Main.emel",
        &mut LexOnly,
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// 字句解析をして，`import A.B` の行だけを読む仮のパーサ．差し込み口を通して import の検査を確かめる．
struct ImportLines {
    checked: bool,
}

impl Frontend for ImportLines {
    type Program = Vec<ModuleId>;

    fn parse(&mut self, module: ModuleId, file: FileId, source: &SourceFile) -> Parsed {
        // 字句の診断はフロントエンドが出す．
        let mut parsed = LexOnly.parse(module, file, source);
        let mut offset = 0;
        for line in source.text().split_inclusive('\n') {
            if let Some(path) = line.trim_end().strip_prefix("import ") {
                parsed.imports.push(Import {
                    path: ModuleName::parse(path),
                    range: TextRange::at(
                        TextSize::new(offset),
                        TextSize::new(line.trim_end().len() as u32),
                    ),
                });
            }
            offset += line.len() as u32;
        }
        parsed
    }

    fn check(&mut self, _: &Analysis, order: &[ModuleId]) -> Checked<Vec<ModuleId>> {
        self.checked = true;
        Checked {
            program: Some(order.to_vec()),
            diagnostics: Vec::new(),
        }
    }
}

#[test]
fn single_file_sees_only_its_directory() {
    let mut frontend = ImportLines { checked: false };
    let analysis = check_files(
        &[
            (
                "home/src/tools/main.emel",
                "import Util\nimport Deep.Thing\n",
            ),
            ("home/src/tools/util.emel", "let x = $\n"),
            ("home/src/tools/Bad.emel", ""),
            // サブディレクトリと祖先は見ない．
            ("home/src/tools/deep/thing.emel", "let y = $\n"),
            ("home/src/other.emel", "let z = $\n"),
        ],
        "home/src/tools/main.emel",
        &mut frontend,
    );
    let names: Vec<String> = analysis
        .modules
        .iter()
        .map(|(_, m)| m.name.to_string())
        .collect();
    assert_eq!(names, ["Main", "Util"]);
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

#[test]
fn import_graph_through_frontend() {
    let mut frontend = ImportLines { checked: false };
    let analysis = check_files(
        &[
            ("app/Pome.toml", ""),
            ("app/src/main.emel", "import Http.Client\nimport Missing\n"),
            ("app/src/http/client.emel", "import Json\n"),
            ("app/src/json.emel", "import Http.Client\n"),
        ],
        "app",
        &mut frontend,
    );
    // 循環があると依存順がないので，型検査は呼ばない．
    assert!(!frontend.checked);
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

#[test]
fn check_runs_in_dependency_order() {
    let mut frontend = ImportLines { checked: false };
    let fs = MemoryFiles::new([
        ("Pome.toml", ""),
        ("src/main.emel", "import Json\n"),
        ("src/json.emel", ""),
    ]);
    let input = Input::resolve(&fs, Path::new(".")).unwrap();
    let (analysis, program) = check(&fs, &input, &mut frontend);
    assert!(analysis.diagnostics.is_empty());
    let names: Vec<String> = program
        .unwrap()
        .iter()
        .map(|&id| analysis.module(id).name.to_string())
        .collect();
    assert_eq!(names, ["Json", "Main"]);
}

#[test]
fn type_errors() {
    let mut sources = SourceDb::new();
    let file = sources.add(
        "src/main.emel",
        "let a: Int = \"s\"\nlet b = [1] == [\"s\"]\nlet f = fn(x) { x(x) }\n",
    );
    let span = |start: u32, end: u32| Span::new(file, TextRange::new(start.into(), end.into()));
    let cons = TyCons::new();
    let mut ctx = InferCtx::new();
    let int = Ty::Prim(Prim::Int);
    let string = Ty::Prim(Prim::String);

    let top = ctx.unify(&int, &string).unwrap_err();
    let nested = ctx
        .unify(&Ty::list(int.clone()), &Ty::list(string.clone()))
        .unwrap_err();
    let var = ctx.new_var();
    let infinite = ctx
        .unify(&var, &Ty::func([var.clone()], int.clone()))
        .unwrap_err();

    let diagnostics = vec![
        Diagnostic::from_type_error(&top, &cons, span(13, 16)),
        Diagnostic::from_type_error(&nested, &cons, span(25, 36)),
        Diagnostic::from_type_error(&infinite, &cons, span(56, 60)),
    ];
    insta::assert_snapshot!(render(&diagnostics, &sources, false));
}

#[test]
fn labels_warnings_and_locations() {
    let mut sources = SourceDb::new();
    let file = sources.add("src/main.emel", "let x = 1\nlet x = 2\n");
    let diagnostics = vec![
        Diagnostic::warning("`x` shadows an earlier binding")
            .with_span(Span::new(file, TextRange::new(14.into(), 15.into())))
            .with_label(
                Span::new(file, TextRange::new(4.into(), 5.into())),
                "the earlier `x` is here",
            )
            .with_note("rename it, or use the earlier `x`"),
        Diagnostic::error("a problem with the whole file").at(Location::File(file)),
        Diagnostic::error("cannot read directory").at(Location::Path("src/secret".into())),
        Diagnostic::error("`node` not found").with_note("install Node.js"),
    ];
    insta::assert_snapshot!(render(&diagnostics, &sources, false));
}

#[test]
fn color_output_has_escapes() {
    let mut sources = SourceDb::new();
    let file = sources.add("a.emel", "$");
    let diagnostics = [Diagnostic::error("認識できない文字")
        .with_span(Span::new(file, TextRange::new(0.into(), 1.into())))];
    assert!(render(&diagnostics, &sources, true).contains('\x1b'));
    assert!(!render(&diagnostics, &sources, false).contains('\x1b'));
}

#[test]
fn syntax_errors_in_every_file() {
    let analysis = check_files(
        &[
            ("app/Pome.toml", ""),
            (
                "app/src/main.emel",
                "import Json\nfn main() {\n  let x = (1,)\n}\n",
            ),
            ("app/src/json.emel", "pub fn parse(s: String -> Int { 1 }\n"),
            ("app/src/http/client.emel", "enum Method {}\nlet n = 3px\n"),
        ],
        "app",
        &mut ParseOnly::new(),
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

#[test]
fn lexical_errors_are_reported_once() {
    let files = [
        ("app/Pome.toml", ""),
        (
            "app/src/main.emel",
            "fn main() {\n  \"abc\n}\nconst y = $\n",
        ),
    ];
    // 字句の診断（E0101〜E0109）だけを取り出す．
    let codes = |analysis: &Analysis| -> Vec<&'static str> {
        analysis
            .diagnostics
            .iter()
            .filter_map(|d| d.code)
            .filter(|c| *c < "E0110")
            .collect()
    };
    let parsed = check_files(&files, "app", &mut ParseOnly::new());
    assert_eq!(codes(&parsed), ["E0101", "E0105"]);
    let lexed = check_files(&files, "app", &mut LexOnly);
    assert_eq!(codes(&lexed), ["E0101", "E0105"]);
}

#[test]
fn real_imports_are_checked() {
    let analysis = check_files(
        &[
            ("app/Pome.toml", ""),
            (
                "app/src/main.emel",
                "import Http.Client.{get, Response}\nimport Missing.Thing\n",
            ),
            ("app/src/http/client.emel", "## 送る\nimport Json\n"),
            ("app/src/json.emel", "import Http.Client.{get}\n"),
            ("app/src/selfish.emel", "import Selfish\n"),
        ],
        "app",
        &mut ParseOnly::new(),
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

#[test]
fn broken_import_is_not_resolved() {
    let analysis = check_files(
        &[
            ("app/Pome.toml", ""),
            (
                "app/src/main.emel",
                "import http\nimport Nope.\nimport Json\n",
            ),
            ("app/src/json.emel", ""),
        ],
        "app",
        &mut ParseOnly::new(),
    );
    let codes: Vec<_> = analysis.diagnostics.iter().filter_map(|d| d.code).collect();
    assert_eq!(codes, ["E0110", "E0122"]);
}

#[test]
fn single_file_reads_only_what_the_entry_imports() {
    let mut frontend = ParseOnly::new();
    let analysis = check_files(
        &[
            ("tools/main.emel", "import Util\nfn main() { 1 }\n"),
            ("tools/util.emel", "import Strings\nfn f( { 1 }\n"),
            ("tools/strings.emel", "const s = \"x\n"),
            // どこからも import されないもの．読まず，診断も出さない．
            ("tools/scratch.emel", "fn broken( {\nconst z = $\n"),
            ("tools/Bad.emel", ""),
            ("tools/http_2.emel", ""),
            ("tools/http2.emel", ""),
        ],
        "tools/main.emel",
        &mut frontend,
    );
    // 対応表は全部作る．
    let names: Vec<String> = analysis
        .modules
        .iter()
        .map(|(_, m)| m.name.to_string())
        .collect();
    assert_eq!(names, ["Http2", "Main", "Scratch", "Strings", "Util"]);
    // 読んで構文解析したのはたどれた3つだけ．
    let read: Vec<String> = analysis
        .modules
        .iter()
        .filter(|(id, _)| frontend.parse_of(*id).is_some())
        .map(|(_, m)| m.name.to_string())
        .collect();
    assert_eq!(read, ["Main", "Strings", "Util"]);
    assert_eq!(analysis.files.iter().count(), 3);
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

#[test]
fn single_file_reports_duplicates_it_reaches() {
    let analysis = check_files(
        &[
            ("tools/main.emel", "import Http2\n"),
            ("tools/http_2.emel", ""),
            ("tools/http2.emel", ""),
        ],
        "tools/main.emel",
        &mut ParseOnly::new(),
    );
    let codes: Vec<_> = analysis.diagnostics.iter().filter_map(|d| d.code).collect();
    assert_eq!(codes, ["E0204"]);
}

#[test]
fn project_reads_everything() {
    let mut frontend = ParseOnly::new();
    let analysis = check_files(
        &[
            ("app/Pome.toml", ""),
            ("app/src/main.emel", ""),
            ("app/src/unused.emel", "fn f( {\n"),
        ],
        "app/src/main.emel",
        &mut frontend,
    );
    assert_eq!(analysis.files.iter().count(), 2);
    // エントリから import されていなくても，プロジェクトなら診断が出る．
    let unused = analysis.sources.file_id("/app/src/unused.emel".as_ref());
    assert!(analysis.has_errors());
    assert!(analysis.diagnostics.iter().all(|d| {
        matches!(d.location, Some(Location::Span(span)) if Some(span.file) == unused)
    }));
}

/// 同梱の core のソースの代わりに使う小さな core．
fn test_core() -> Vec<CoreSource> {
    vec![
        CoreSource {
            module: "String".into(),
            text: "@intrinsic\npub fn length(s: String) -> Int\n".into(),
        },
        CoreSource {
            module: "Prelude".into(),
            text: "pub fn identity[A](x: A) -> A { x }\n".into(),
        },
    ]
}

#[test]
fn core_sources_resolve_as_modules() {
    let mut frontend = Resolve::default().with_core(test_core());
    let analysis = check_files(
        &[
            ("app/Pome.toml", ""),
            (
                "app/src/main.emel",
                "fn main() -> Int {\n  identity(String.length(\"abc\"))\n}\n\n@intrinsic\nfn mine() -> Int\n",
            ),
            // core と同じ名前のソースのモジュールは隠れ，予約された名前になる．
            (
                "app/src/string.emel",
                "pub fn length(s: String) -> Int { 0 }\n",
            ),
        ],
        "app",
        &mut frontend,
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

#[test]
fn single_file_reads_core_sources() {
    let mut frontend = Resolve::default().with_core(test_core());
    let analysis = check_files(
        &[(
            "home/main.emel",
            "fn main() -> Int { String.length(\"a\") }\n",
        )],
        "home/main.emel",
        &mut frontend,
    );
    assert_eq!(render(&analysis.diagnostics, &analysis.sources, false), "");
    let names: Vec<String> = analysis
        .files
        .iter()
        .map(|(id, _)| analysis.module(id).name.to_string())
        .collect();
    assert!(names.contains(&"String".to_owned()), "{names:?}");
}

#[test]
fn bundled_core_resolves_cleanly() {
    let mut frontend = Resolve::default();
    let analysis = check_files(
        &[(
            "home/main.emel",
            "fn main() -> Int {\n  xs = List.map([1, 2], fn(x) { x + 1 })\n  n = Option.unwrap_or(List.head(xs), 0)\n  String.length(\"#{n}\")\n}\n",
        )],
        "home/main.emel",
        &mut frontend,
    );
    assert_eq!(render(&analysis.diagnostics, &analysis.sources, false), "");
}

#[test]
fn bundled_core_typechecks_cleanly() {
    let mut frontend = Typecheck::default();
    let analysis = check_files(
        &[(
            "home/main.emel",
            "fn main() -> Int {\n  xs = List.map([1, 2], fn(x) { x + 1 })\n  n = Option.unwrap_or(List.head(xs), 0)\n  String.length(\"#{n}\")\n}\n",
        )],
        "home/main.emel",
        &mut frontend,
    );
    assert_eq!(render(&analysis.diagnostics, &analysis.sources, false), "");
}

#[test]
fn type_checking_reports_in_source_order() {
    let mut frontend = Typecheck::default();
    let analysis = check_files(
        &[(
            "home/main.emel",
            r#"type User(id: Int, name: String)

enum Shape {
  Circle(radius: Float)
  Rect(Float, Float)
  Empty
}

fn connect(host: String, port: Int) -> Bool { todo() }

fn area(s: Shape) -> Float {
  match s {
    Circle(radius:) -> radius
    Rect(w, h) -> w * h
    _ -> 0.0
    Empty -> 1.0
  }
}

fn main() {
  u = User(id: 1, name: "a")
  a: Int = u.name
  b = connect("x", prot: 1)
  c: Int = 3000000000
  Some(x) = Some(1)
  User(name:) = u
  d = u.nmae
}
"#,
        )],
        "home/main.emel",
        &mut frontend,
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// エフェクトの診断（E04xx）の文面．main とテスト関数の R が空でないことも，driver が
/// main とテスト関数を型推論に渡して確かめる（8.3，13.3）．
#[test]
fn effect_diagnostics() {
    let mut frontend = Typecheck::default();
    let analysis = check_files(
        &[(
            "home/main.emel",
            r#"effect Logger {
  fn info(msg: String) -> ()
  fn warn(msg: String) -> ()
}

effect Clock {
  fn now() -> Int
}

handler Half implements Logger {
  fn info(msg) { () }
  fn info(msg) { () }
}

handler Stamped implements Logger {
  fn info(a, b) { (use Clock).now()
  () }
  fn warn(msg) { () }
}

handler Ticking implements Clock {
  fn now() { (use Logger).info("tick")
  0 }
}

@external(js, "console.log")
fn console_log(text: String) -> ()

fn only(f: fn() -> () use {}) -> () use {} {
  (use Logger).infoo("x")
  f()
}

fn misuse() {
  log = use Logger
  only(fn() { log.info("x") })
  console_log("x")
  xs = [log]
  Some(log)
  with Stamped, Ticking { () }
  log
}

fn main() {
  (use Clock).now()
}

@test
fn logs() {
  (use Logger).info("t")
}
"#,
        )],
        "home/main.emel",
        &mut frontend,
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// suspend，Async，Immediate，宣言の型引数の集合の診断（E0221，E0322〜E0324，E0407，E0413〜E0416）の文面．main の R には Async だけが
/// 残ってよい（8.3）．
#[test]
fn suspend_diagnostics() {
    let mut frontend = Typecheck::default();
    let analysis = check_files(
        &[(
            "home/main.emel",
            r#"effect Clock {
  suspend fn sleep(ms: Int) -> ()
  fn now() -> Int
}

@external(js, "setTimeout")
fn set_timeout(ms: Int, f: fn() -> ()) -> ()

@external(js, "./ffi.mjs", "fetchText")
suspend fn fetch_text(url: String) -> String

handler JsClock implements Clock {
  suspend fn sleep(ms, resume) {
    set_timeout(ms, fn() { resume(()) })
  }
  suspend fn now(resume) { resume(0) }
}

handler Runtime implements Async {
}

effect Http {
  fn get(url: String) -> String
}

handler JsHttp implements Http {
  fn get(url) { fetch_text(url) }
}

effect Ticks {
  fn tick() -> Int
}

fn stamp() -> Int use { Clock } { (use Clock).now() }

handler Stamped implements Ticks {
  fn tick() { stamp() + (use Clock).now() }
}

fn update[A, R: Immediate](x: A, f: fn(A) -> A use R) -> A use R { f(x) }

fn not_effect[A: Immediate](x: A) -> A { x }

type Handler[E, R](run: fn() -> () fails E use R)

fn handlers(a: Handler[Int, Clock], b: Handler[Clock, Clock], c: List[Clock], d: List[{}]) { () }

fn misuse(g: fn(Int) -> Int) {
  use Async
  fetch_text("x")
  update(1, fn(n) {
    (use Clock).sleep(n)
    n
  })
  update(1, g)
}

fn main() {
  with JsClock { (use Clock).sleep(1) }
}
"#,
        )],
        "home/main.emel",
        &mut frontend,
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// 状態のあるハンドラと layer の診断（E0411，E0418〜E0422）の文面（8.4，8.5）．
#[test]
fn stateful_handler_diagnostics() {
    let mut frontend = Typecheck::default();
    let analysis = check_files(
        &[(
            "home/main.emel",
            r#"error Boom

effect Logger {
  fn info(msg: String) -> ()
}

effect Ping {
  fn ping() -> ()
}

effect Pong {
  fn pong() -> ()
}

fn risky() -> () fails Boom { fail Boom }

handler Counter(n: Int) implements Logger {
  init { Counter(n: 0) }
  release { risky() }
  fn info(msg) { () }
}

handler NoInit(n: Int) implements Logger {
  fn info(msg) { () }
}

handler Quiet implements Logger {
  fn info(msg) { () }
}

handler HPing implements Ping {
  fn ping() { (use Pong).pong() }
}

handler HPong implements Pong {
  fn pong() { (use Ping).ping() }
}

layer Loud {
  Counter
  Quiet
}

layer Twice { Quiet, Quiet }

layer LoopA { LoopB }

layer LoopB { LoopA }

layer PingPong {
  HPing
  HPong
}

fn make() { Counter(n: 1) }
"#,
        )],
        "home/main.emel",
        &mut frontend,
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

#[test]
fn bundled_console_checks_cleanly() {
    // 同梱の Console と JsConsole は import なしで使える（15.2）．
    let main = "fn main() {\n  with JsConsole {\n    (use Console).print_line(\"hi\")\n  }\n}\n";
    let files = [("home/main.emel", main)];
    for analysis in [
        check_files(&files, "home/main.emel", &mut Resolve::default()),
        check_files(&files, "home/main.emel", &mut Typecheck::default()),
        check_files(&files, "home/main.emel", &mut Lower::default()),
    ] {
        assert_eq!(render(&analysis.diagnostics, &analysis.sources, false), "");
    }
}

/// JS 用の `@external` のない外部関数を，main からハンドラ越しに呼ぶもの．`wasm_unused` は使わない．
const WASM_ONLY: &str = r#"effect Clock {
  fn now() -> Int
}

@external(wasm, "env", "now")
fn wasm_now() -> Int

@external(wasm, "env", "unused")
fn wasm_unused() -> Int

handler WasmClock implements Clock {
  fn now() { wasm_now() }
}

handler UnusedClock implements Clock {
  fn now() { wasm_unused() }
}

fn main() {
  with WasmClock { dbg((use Clock).now()) }
}
"#;

#[test]
fn reachable_external_without_target() {
    // build，run，test の lowering は，main からたどれる外部関数に JS の注釈を求める（12.1）．
    let files = [("home/main.emel", WASM_ONLY)];
    let analysis = check_files(&files, "home/main.emel", &mut Lower::default());
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
    // check と LSP（型検査まで）はターゲットを決めないので検査しない．
    let analysis = check_files(&files, "home/main.emel", &mut Typecheck::default());
    assert_eq!(render(&analysis.diagnostics, &analysis.sources, false), "");
}

#[test]
fn internal_fns_are_core_only() {
    // `@internal` の fn は core のモジュールからは呼べ，ソースのモジュールからは呼べない（E0228）．
    // `@internal` はソースのモジュールには書けない（E0229）．今の同梱の core には `@internal` の
    // fn がないので，テスト用の core のモジュールを足す．
    let mut core = CoreSource::bundled();
    core.push(CoreSource {
        module: "Secret".into(),
        text: "@internal\npub fn key() -> Int { 42 }\n\npub fn hint() -> Int { key() - 1 }\n"
            .into(),
    });
    core.push(CoreSource {
        module: "Friend".into(),
        text: "pub fn peek() -> Int { Secret.key() }\n".into(),
    });
    let mut frontend = Typecheck::new(Resolve::default().with_core(core));
    let analysis = check_files(
        &[(
            "home/main.emel",
            "\
fn main() {
  k = Secret.key()
  f = Secret.key
  dbg(Secret.hint() + Friend.peek())
}

@internal
fn mine() -> Int { 1 }
",
        )],
        "home/main.emel",
        &mut frontend,
    );
    let codes: Vec<_> = analysis.diagnostics.iter().map(|d| d.code).collect();
    assert_eq!(codes, [Some("E0228"), Some("E0228"), Some("E0229")]);
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

#[test]
fn time_module_files_are_reserved() {
    // 同梱の Instant と Duration と同じ名前のファイルはソースのルート直下に置けない（4.1，E0203）．
    let analysis = check_files(
        &[
            ("app/Pome.toml", ""),
            ("app/src/main.emel", "fn main() {}\n"),
            ("app/src/instant.emel", "pub fn f() -> Int { 1 }\n"),
            ("app/src/duration.emel", "pub fn f() -> Int { 1 }\n"),
        ],
        "app",
        &mut Typecheck::default(),
    );
    let codes: Vec<_> = analysis.diagnostics.iter().map(|d| d.code).collect();
    assert_eq!(codes, [Some("E0203"), Some("E0203")]);
}
