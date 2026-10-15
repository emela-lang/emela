//! Collecting and rendering diagnostics. Uncolored output is snapshotted.

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

/// A stand-in parser that lexes and reads only `import A.B` lines, to check import checks
/// through the plug-in point.
struct ImportLines {
    checked: bool,
}

impl Frontend for ImportLines {
    type Program = Vec<ModuleId>;

    fn parse(&mut self, module: ModuleId, file: FileId, source: &SourceFile) -> Parsed {
        // Lexer diagnostics are the frontend's job.
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
            // Subdirectories and ancestors are not looked at.
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
    // With a cycle there is no dependency order, so the type checker is not called.
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
    // Only lexer diagnostics (E0101 to E0109).
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
            // Not imported from anywhere: never read, no diagnostics.
            ("tools/scratch.emel", "fn broken( {\nconst z = $\n"),
            ("tools/Bad.emel", ""),
            ("tools/http_2.emel", ""),
            ("tools/http2.emel", ""),
        ],
        "tools/main.emel",
        &mut frontend,
    );
    // The module table is always complete.
    let names: Vec<String> = analysis
        .modules
        .iter()
        .map(|(_, m)| m.name.to_string())
        .collect();
    assert_eq!(names, ["Http2", "Main", "Scratch", "Strings", "Util"]);
    // Only the three reached modules were read and parsed.
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
    // In a project, diagnostics are reported even for modules the entry does not import.
    let unused = analysis.sources.file_id("/app/src/unused.emel".as_ref());
    assert!(analysis.has_errors());
    assert!(analysis.diagnostics.iter().all(|d| {
        matches!(d.location, Some(Location::Span(span)) if Some(span.file) == unused)
    }));
}

/// A small core used in place of the bundled core sources.
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
            // A source module named like a core module is hidden and gets a reserved name error.
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
            "fn main() -> Int {\n  xs = List.map([1, 2], fn(x) { x + 1 })\n  n = Option.or(List.head(xs), 0)\n  String.length(\"#{n}\")\n}\n",
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
            "fn main() -> Int {\n  xs = List.map([1, 2], fn(x) { x + 1 })\n  n = Option.or(List.head(xs), 0)\n  String.length(\"#{n}\")\n}\n",
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
  s match {
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

/// The messages of effect diagnostics (E04xx). The driver also passes main and test functions to
/// inference to check that their R is empty (spec 8.3, 13.3).
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

/// The messages of suspend, Async, Immediate and declaration type argument set diagnostics
/// (E0221, E0322 to E0324, E0407, E0413 to E0416). Only Async may remain in main's R (spec 8.3).
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

handler SystemClock implements Clock {
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
  with SystemClock { (use Clock).sleep(1) }
}
"#,
        )],
        "home/main.emel",
        &mut frontend,
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// The messages of stateful handler and layer diagnostics (E0411, E0418 to E0422; spec 8.4, 8.5).
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

/// The messages of effect type parameter diagnostics (E0423, E0424, E0304, E0402; spec 8.1).
#[test]
fn effect_type_param_diagnostics() {
    let mut frontend = Typecheck::default();
    let analysis = check_files(
        &[(
            "home/main.emel",
            r#"effect Cell[S] {
  fn get() -> S
}

effect Raise[E] {
  fn guard(f: fn() -> () fails E) -> ()
}

handler Generic implements Cell {
  fn get() { 0 }
}

handler Zero implements Cell[Int] {
  fn get() { 0 }
}

fn omitted() { (use Cell).get() }

fn too_many() { (use Cell[Int, Int]).get() }

handler One implements Cell[Int] {
  fn get() { 1 }
}

handler Empty implements Cell[String] {
  fn get() { "" }
}

fn leftover() -> Int use {} {
  with Zero {
    (use Cell[String]).get()
    (use Cell[Int]).get()
  }
}

layer Both {
  Zero
  Empty
}

layer Twice {
  Zero
  One
}
"#,
        )],
        "home/main.emel",
        &mut frontend,
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

#[test]
fn bundled_console_checks_cleanly() {
    let main = "import Std.Console.{Console, SystemConsole}\n\nfn main() {\n  with SystemConsole {\n    (use Console).print_line(\"hi\")\n  }\n}\n";
    let files = [("home/main.emel", main)];
    for analysis in [
        check_files(&files, "home/main.emel", &mut Resolve::default()),
        check_files(&files, "home/main.emel", &mut Typecheck::default()),
        check_files(&files, "home/main.emel", &mut Lower::default()),
    ] {
        assert_eq!(render(&analysis.diagnostics, &analysis.sources, false), "");
    }
}

/// An external function without a JS `@external`, called from main through a handler.
/// `wasm_unused` is never used.
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
    // Lowering for build, run and test requires JS annotations on external functions reachable
    // from main (spec 12.1).
    let files = [("home/main.emel", WASM_ONLY)];
    let analysis = check_files(&files, "home/main.emel", &mut Lower::default());
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
    // check and the LSP (up to type checking) choose no target, so they do not check.
    let analysis = check_files(&files, "home/main.emel", &mut Typecheck::default());
    assert_eq!(render(&analysis.diagnostics, &analysis.sources, false), "");
}

/// A handler whose `sleep` has no JS `@external`. Only operations called somewhere reachable are
/// checked (spec 12.1), so using only `now` builds, and calling `sleep` reports `wasm_sleep`.
const PARTIAL_HANDLER: &str = r#"effect Clock {
  fn now() -> Int
  fn sleep(ms: Int) -> ()
}

@external(js, "Date.now")
fn js_now() -> Int

@external(wasm, "env", "sleep")
fn wasm_sleep(ms: Int) -> ()

handler HostClock implements Clock {
  fn now() { js_now() }
  fn sleep(ms) { wasm_sleep(ms) }
}

fn main() {
  with HostClock { dbg(CALL) }
}
"#;

#[test]
fn only_called_operations_are_checked_for_the_target() {
    let render_main = |call: &str| {
        let main = PARTIAL_HANDLER.replace("CALL", call);
        let files = [("home/main.emel", main.as_str())];
        let analysis = check_files(&files, "home/main.emel", &mut Lower::default());
        render(&analysis.diagnostics, &analysis.sources, false)
    };
    assert_eq!(render_main("(use Clock).now()"), "");
    let called = render_main("(use Clock).sleep(1)");
    assert!(
        called.contains("external function `wasm_sleep` has no `@external` for target `js`"),
        "{called}"
    );
    // An operation used as a value counts as used.
    let as_value = render_main("List.map([1], (use Clock).sleep)");
    assert!(as_value.contains("`wasm_sleep`"), "{as_value}");
}

/// Handlers of one effect with different type arguments (spec 8.1). The check runs on the
/// monomorphized IR, where `Store[Int]` and `Store[String]` are different effects, so calling
/// `put` of `Store[String]` follows only `StringStore`'s implementation.
const STORES: &str = r#"effect Store[T] {
  fn put(x: T) -> ()
}

@external(js, "console.log")
fn js_log(s: String) -> ()

@external(wasm, "env", "log_int")
fn wasm_log_int(n: Int) -> ()

handler IntStore implements Store[Int] {
  fn put(x) { wasm_log_int(x) }
}

handler StringStore implements Store[String] {
  fn put(x) { js_log(x) }
}

fn main() {
  with IntStore, StringStore { CALL }
}
"#;

#[test]
fn operations_are_checked_per_effect_instance() {
    let render_main = |call: &str| {
        let main = STORES.replace("CALL", call);
        let files = [("home/main.emel", main.as_str())];
        let analysis = check_files(&files, "home/main.emel", &mut Lower::default());
        render(&analysis.diagnostics, &analysis.sources, false)
    };
    assert_eq!(render_main("(use Store[String]).put(\"a\")"), "");
    let int = render_main("(use Store[Int]).put(1)");
    assert!(
        int.contains("external function `wasm_log_int` has no `@external` for target `js`"),
        "{int}"
    );
}

#[test]
fn internal_fns_are_core_only() {
    // `@internal` fns can be called from core modules but not from source modules (E0228), and
    // `@internal` cannot be written in source modules (E0229). The bundled core has no `@internal`
    // fn, so a test core module is added.
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
fn std_and_core_module_files_are_reserved() {
    // Files named like core modules, and `std.emel` and `std/`, cannot sit directly under the
    // source root (spec 4.1, E0203). Std modules live under `Std`, so `instant.emel` is allowed
    // (spec 15.2).
    let analysis = check_files(
        &[
            ("app/Pome.toml", ""),
            (
                "app/src/main.emel",
                "import Instant\n\nfn main() { Instant.f() }\n",
            ),
            ("app/src/instant.emel", "pub fn f() -> Int { 1 }\n"),
            ("app/src/list.emel", "pub fn f() -> Int { 1 }\n"),
            ("app/src/std.emel", "pub fn f() -> Int { 1 }\n"),
            ("app/src/std/console.emel", "pub fn f() -> Int { 1 }\n"),
        ],
        "app",
        &mut Typecheck::default(),
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}
