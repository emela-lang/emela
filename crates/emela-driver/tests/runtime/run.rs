//! Plugs hand-written JS in place of the output stage and runs node through the `emela run`
//! path.

use std::path::{Path, PathBuf};

use emela_core::build::*;
use emela_core::{BinOp, Builtin, Expr, FnId, OpTy, TypeId};
use emela_driver::{
    Analysis, Backend, BuildOutput, Checked, CoreJs, DEFECT_EXIT_CODE, Diagnostic, FileId,
    Frontend, Input, LexOnly, Lower, MemoryFiles, Output, OutputFile, Parsed, RunOptions,
    RunOutput, SourceFile, build, run,
};
use emela_resolve::ModuleId;

mod effects;
mod stdlib;

/// An output stage returning fixed JS.
struct HandWritten {
    files: Vec<OutputFile>,
}

impl HandWritten {
    fn main(js: &str) -> Self {
        HandWritten {
            files: vec![
                OutputFile::new("main.mjs", js),
                OutputFile::new("emela_runtime.mjs", emela_codegen_js::RUNTIME),
                OutputFile::new(
                    "runtime/defect.mjs",
                    "export class Defect extends Error {\n  constructor(message) { super(message); this.name = \"Defect\"; }\n}\n",
                ),
            ],
        }
    }
}

impl Backend<()> for HandWritten {
    fn emit(&mut self, _: &(), analysis: &Analysis) -> Result<BuildOutput, Vec<Diagnostic>> {
        assert!(analysis.entry.is_some());
        Ok(BuildOutput {
            files: self.files.clone(),
            entry: "main.mjs".into(),
        })
    }
}

/// A per-test output directory. Leftovers from earlier runs are removed.
fn out_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("emela-driver-run-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn run_js(
    name: &str,
    sources: &[(&str, &str)],
    js: &str,
    args: &[&str],
) -> (Analysis, Option<RunOutput>) {
    run_with(name, sources, js, args, PathBuf::from("node"))
}

fn run_with(
    name: &str,
    sources: &[(&str, &str)],
    js: &str,
    args: &[&str],
    js_host: PathBuf,
) -> (Analysis, Option<RunOutput>) {
    let fs = MemoryFiles::new(sources.iter().copied());
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let options = RunOptions {
        out_dir: out_dir(name),
        js_host,
        args: args.iter().map(Into::into).collect(),
        output: Output::Capture,
    };
    let mut reported = None;
    let result = run(
        &fs,
        &input,
        &mut LexOnly,
        &mut HandWritten::main(js),
        &options,
        |analysis, _| reported = Some(analysis.diagnostics.len()),
    );
    // Called once before node. A diagnostic about failing to start node is added after it.
    assert!(reported.unwrap() <= result.0.diagnostics.len());
    let _ = std::fs::remove_dir_all(&options.out_dir);
    result
}

const MAIN: &[(&str, &str)] = &[("app/Pome.toml", ""), ("app/src/main.emel", "let x = 1\n")];

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap()
}

#[test]
fn prints_stdout() {
    let (analysis, output) = run_js(
        "stdout",
        MAIN,
        "import { Defect } from \"./runtime/defect.mjs\";\nconsole.log(\"こんにちは\");\nconsole.error(\"to stderr\");\n",
        &[],
    );
    // The programs may still write `use E` as an expression (W4401, deprecated).
    assert!(
        analysis
            .diagnostics
            .iter()
            .all(|d| matches!(d.code, Some(emela_driver::code::DEPRECATED_USE))),
        "{:?}",
        analysis.diagnostics
    );
    let output = output.unwrap();
    assert_eq!(text(&output.stdout), "こんにちは\n");
    assert_eq!(text(&output.stderr), "to stderr\n");
    assert_eq!(output.code, 0);
}

#[test]
fn passes_args_and_exit_code() {
    let (_, output) = run_js(
        "exit",
        MAIN,
        "console.log(process.argv.slice(2).join(\",\"));\nprocess.exitCode = 3;\n",
        &["a", "b c"],
    );
    let output = output.unwrap();
    assert_eq!(text(&output.stdout), "a,b c\n");
    assert_eq!(output.code, 3);
}

/// A `main.mjs` that launches its `main` through the runtime, as the emitted one does
/// (`[target.js.module]`).
fn launching(main_body: &str) -> String {
    format!(
        "import {{ $launch }} from \"./emela_runtime.mjs\";\nimport {{ Defect }} from \"./runtime/defect.mjs\";\nfunction main() {{\n{main_body}}}\n$launch(import.meta.main, import.meta.url, main);\n"
    )
}

#[test]
fn defect_has_its_own_exit_code() {
    let (_, output) = run_js(
        "defect",
        MAIN,
        &launching("console.log(\"before\");\nthrow new Defect(\"整数を 0 で割った\");\n"),
        &[],
    );
    let output = output.unwrap();
    assert_eq!(text(&output.stdout), "before\n");
    assert_eq!(text(&output.stderr), "defect: 整数を 0 で割った\n");
    assert_eq!(output.code, DEFECT_EXIT_CODE);
}

#[test]
fn other_exceptions_are_left_to_node() {
    let (_, output) = run_js("throw", MAIN, "throw new TypeError(\"boom\");\n", &[]);
    let output = output.unwrap();
    assert_eq!(output.code, 1);
    assert!(text(&output.stderr).contains("TypeError: boom"));
}

#[test]
fn errors_stop_before_node() {
    let (analysis, output) = run_js(
        "errors",
        &[
            ("app/Pome.toml", ""),
            ("app/src/main.emel", "let x = $\n"),
            ("app/src/b.emel", "\""),
        ],
        "console.log(\"never\");\n",
        &[],
    );
    assert!(output.is_none());
    assert_eq!(analysis.error_count(), 2);
}

#[test]
fn missing_entry() {
    let (analysis, output) = run_js(
        "no-entry",
        &[("app/Pome.toml", ""), ("app/src/util.emel", "")],
        "console.log(\"never\");\n",
        &[],
    );
    assert!(output.is_none());
    let messages: Vec<_> = analysis
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(messages, ["the entry `src/main.emel` does not exist"]);
}

#[test]
fn missing_node_is_a_diagnostic() {
    let (analysis, output) = run_with("no-node", MAIN, "", &[], PathBuf::from("/nonexistent/node"));
    assert!(output.is_none());
    let messages: Vec<_> = analysis
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(messages, ["`/nonexistent/node` not found"]);
}

/// A frontend that returns hand-built Core IR in place of the type checking results. Like
/// `emela_core::lower`, it hands the IR on loopified.
struct HandBuiltIr(emela_core::Module);

impl Frontend for HandBuiltIr {
    type Program = emela_core::Module;

    fn parse(&mut self, _: ModuleId, _: FileId, _: &SourceFile) -> Parsed {
        Parsed::default()
    }

    fn check(&mut self, _: &Analysis, _: &[ModuleId]) -> Checked<emela_core::Module> {
        let mut module = self.0.clone();
        emela_core::tail::loopify(&mut module);
        Checked {
            program: Some(module),
            diagnostics: Vec::new(),
        }
    }
}

/// `fn fact(n, acc) = if n == 0 then acc else fact(n - 1, acc * n)` and a `main` whose body is
/// `body`, of type `ret`.
fn fact_module(ret: TypeId, body: impl FnOnce(FnId) -> Expr) -> emela_core::Module {
    let mut m = emela_core::Module::new();
    let fact = m.declare("fact", false);
    let n = m.local("n", TypeId::INT);
    let acc = m.local("acc", TypeId::INT);
    m.define(
        fact,
        vec![n, acc],
        TypeId::INT,
        if_(
            bin(BinOp::Eq, OpTy::Int, var(n), int(0)),
            var(acc),
            call(
                fact,
                vec![
                    bin(BinOp::Sub, OpTy::Int, var(n), int(1)),
                    bin(BinOp::Mul, OpTy::Int, var(acc), var(n)),
                ],
            ),
        ),
    );
    let main = m.declare("main", true);
    m.define(main, vec![], ret, body(fact));
    m
}

fn run_ir(name: &str, module: emela_core::Module) -> RunOutput {
    let fs = MemoryFiles::new(MAIN.iter().copied());
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let options = RunOptions {
        out_dir: out_dir(name),
        js_host: emela_driver::js_host_program(),
        args: Vec::new(),
        output: Output::Capture,
    };
    let (analysis, output) = run(
        &fs,
        &input,
        &mut HandBuiltIr(module),
        &mut CoreJs,
        &options,
        |_, _| {},
    );
    let _ = std::fs::remove_dir_all(&options.out_dir);
    // The programs may still write `use E` as an expression (W4401, deprecated).
    assert!(
        analysis
            .diagnostics
            .iter()
            .all(|d| matches!(d.code, Some(emela_driver::code::DEPRECATED_USE))),
        "{:?}",
        analysis.diagnostics
    );
    output.unwrap()
}

#[test]
fn core_ir_runs_through_codegen_js() {
    // Run the tail recursion 100,000 times, then pass the result out through a panic message.
    let output = run_ir(
        "ir-panic",
        fact_module(TypeId::NEVER, |fact| {
            let big = call(fact, vec![int(100000), int(1)]);
            let small = call(fact, vec![int(5), int(1)]);
            builtin(
                Builtin::Panic,
                vec![Expr::Concat(vec![
                    lit_part("fact(5) = "),
                    value_part(small, TypeId::INT),
                    lit_part(", fact(100000) = "),
                    value_part(big, TypeId::INT),
                ])],
            )
        }),
    );
    assert_eq!(text(&output.stdout), "");
    assert_eq!(
        text(&output.stderr),
        "defect: fact(5) = 120, fact(100000) = 0\n"
    );
    assert_eq!(output.code, DEFECT_EXIT_CODE);

    // A runtime defect (division by zero) is handled the same way.
    let output = run_ir(
        "ir-div",
        fact_module(TypeId::INT, |_| bin(BinOp::Div, OpTy::Int, int(1), int(0))),
    );
    assert_eq!(text(&output.stderr), "defect: division by zero\n");
    assert_eq!(output.code, DEFECT_EXIT_CODE);

    // Without a defect, the exit code is 0.
    let output = run_ir(
        "ir-ok",
        fact_module(TypeId::INT, |fact| call(fact, vec![int(10), int(1)])),
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "", "")
    );
}

/// Bundled host files (`Module::bundled_files`) are written under `emela_core/` in the output
/// directory and imported relative to the entry output. Relative imports between bundled files
/// work too.
#[test]
fn bundled_files_are_written_next_to_the_output() {
    let mut m = emela_core::Module::new();
    let now = m.declare("HostTest.now", false);
    m.define_external(
        now,
        vec![],
        TypeId::INT,
        emela_core::External {
            bindings: vec![emela_core::ExternalBinding {
                target: "js".into(),
                host: emela_core::HostRef::Import {
                    module: emela_core::HostModule::Bundled("host/now.mjs".into()),
                    name: "now".into(),
                },
            }],
        },
    );
    let main = m.declare("main", true);
    m.define(
        main,
        vec![],
        TypeId::NEVER,
        builtin(
            Builtin::Panic,
            vec![Expr::Concat(vec![
                lit_part("now = "),
                value_part(call(now, vec![]), TypeId::INT),
            ])],
        ),
    );
    m.bundled_files = vec![
        emela_core::BundledFile {
            path: "host/now.mjs".into(),
            contents: "import { base } from \"../base.mjs\";\nexport const now = () => base + 1;\n"
                .into(),
        },
        emela_core::BundledFile {
            path: "base.mjs".into(),
            contents: "export const base = 41;\n".into(),
        },
    ];
    let output = run_ir("core-js-files", m);
    assert_eq!(text(&output.stderr), "defect: now = 42\n");
    assert_eq!(output.code, DEFECT_EXIT_CODE);
}

/// Runs source through `Lower` (name resolution, type checking, lowering) and `CoreJs` on the
/// `emela run` path.
fn run_source(name: &str, main: &str) -> RunOutput {
    run_source_with(name, main, &mut Lower::default())
}

fn run_source_with(name: &str, main: &str, frontend: &mut Lower) -> RunOutput {
    run_source_in(name, main, frontend, PathBuf::from("node"))
}

/// Like `run_source_with`, with a given node executable.
fn run_source_in(name: &str, main: &str, frontend: &mut Lower, js_host: PathBuf) -> RunOutput {
    let sources = [("app/Pome.toml", ""), ("app/src/main.emel", main)];
    let fs = MemoryFiles::new(sources.iter().copied());
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let options = RunOptions {
        out_dir: out_dir(name),
        js_host,
        args: Vec::new(),
        output: Output::Capture,
    };
    let (analysis, output) = run(&fs, &input, frontend, &mut CoreJs, &options, |_, _| {});
    let _ = std::fs::remove_dir_all(&options.out_dir);
    // The programs may still write `use E` as an expression (W4401, deprecated).
    assert!(
        analysis
            .diagnostics
            .iter()
            .all(|d| matches!(d.code, Some(emela_driver::code::DEPRECATED_USE))),
        "{:?}",
        analysis.diagnostics
    );
    output.unwrap()
}

#[test]
fn const_self_reference_in_if_match_and_through_function_is_a_defect() {
    let cases = [
        (
            "const-self-if",
            "const A: Int = if True { A } else { 0 }\nfn main() {\n  dbg(A)\n  ()\n}\n",
        ),
        (
            "const-self-match",
            "const A: Int = True match {\n  True -> A\n  False -> 0\n}\nfn main() {\n  dbg(A)\n  ()\n}\n",
        ),
        (
            "const-self-indirect",
            "const A: Int = if True { value() } else { 0 }\nfn value() -> Int { A }\nfn main() {\n  dbg(A)\n  ()\n}\n",
        ),
    ];
    for (name, source) in cases {
        let output = run_source(name, source);
        assert_eq!(
            text(&output.stderr),
            "defect: const A refers to itself while being evaluated\n"
        );
        assert_eq!(output.code, DEFECT_EXIT_CODE);
    }
}

#[test]
fn run_without_main_stops_before_node() {
    // Without main in the entry, run stops with E2009 and does not start node (spec 8.3). Starting
    // it would add a "node not found" diagnostic, so a missing node is passed to check that.
    let sources = [
        ("app/Pome.toml", ""),
        ("app/src/main.emel", "fn helper() { 1 }\n"),
    ];
    let fs = MemoryFiles::new(sources.iter().copied());
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let options = RunOptions {
        out_dir: out_dir("no-main"),
        js_host: PathBuf::from("/nonexistent/node"),
        args: Vec::new(),
        output: Output::Capture,
    };
    let (analysis, output) = run(
        &fs,
        &input,
        &mut Lower::default(),
        &mut CoreJs,
        &options,
        |_, _| {},
    );
    assert!(output.is_none());
    assert!(!options.out_dir.exists());
    let diagnostics: Vec<_> = analysis
        .diagnostics
        .iter()
        .map(|d| (d.code, d.message.as_str()))
        .collect();
    assert_eq!(
        diagnostics,
        [(
            Some("E2009"),
            "entry `src/main.emel` has no `main` function"
        )]
    );

    // build emits output even without main.
    let (analysis, entry) = build(
        &fs,
        &input,
        &mut Lower::default(),
        &mut CoreJs,
        &options.out_dir,
    );
    // The programs may still write `use E` as an expression (W4401, deprecated).
    assert!(
        analysis
            .diagnostics
            .iter()
            .all(|d| matches!(d.code, Some(emela_driver::code::DEPRECATED_USE))),
        "{:?}",
        analysis.diagnostics
    );
    assert!(entry.is_some_and(|entry| entry.exists()));
    let _ = std::fs::remove_dir_all(&options.out_dir);
}

#[test]
fn run_stops_at_syntax_error_like_check() {
    // A syntax error stops run before lowering, as it stops check: lowering assumes an error-free
    // program, so a recovered tree would add errors without a code (#682).
    let main = "fn main() {\n  d = 1 +\n    2\n  ()\n}\n";
    let sources = [("app/Pome.toml", ""), ("app/src/main.emel", main)];
    let fs = MemoryFiles::new(sources.iter().copied());
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let options = RunOptions {
        out_dir: out_dir("syntax-error"),
        js_host: PathBuf::from("/nonexistent/node"),
        args: Vec::new(),
        output: Output::Capture,
    };
    let (ran, output) = run(
        &fs,
        &input,
        &mut Lower::default(),
        &mut CoreJs,
        &options,
        |_, _| {},
    );
    assert!(output.is_none());
    assert!(!options.out_dir.exists());
    let (checked, _) = emela_driver::check(&fs, &input, &mut emela_driver::Typecheck::default());
    let codes = |analysis: &Analysis| -> Vec<_> {
        analysis
            .diagnostics
            .iter()
            .map(|d| (d.code, d.message.clone()))
            .collect()
    };
    assert_eq!(codes(&ran), codes(&checked));
    assert!(
        ran.diagnostics.iter().all(|d| d.code == Some("E1102")),
        "{:?}",
        ran.diagnostics
    );
}

/// `[target.js.module]`: `@export(js, "name")` is a named export of main.mjs and
/// `@export(js, "default", "name")` a property of the default export; `@entry` exports the fn
/// through the adapter (`[ext.entry]`). A pub fn without `@export` is not exported. A fn that does
/// not suspend returns its value and throws a defect; one that may suspend returns a Promise.
/// Arguments and return values cross as boundary values (`[target.js.value]`), callbacks too.
#[test]
fn js_exports_are_called_by_the_host() {
    let main = r##"import Std.Clock.{Clock, SystemClock}

@export(js, "add")
@export(js, "default", "add")
pub fn add(a: Int, b: Int) -> Int { a + b }

@export(js, "greet")
fn greet(name: String, suffix: fn(String) -> String fails {} uses {}) -> String {
  "hello, #{suffix(name)}"
}

@export(js, "half")
fn half(n: Int) -> Int { 10 / n }

@export(js, "late")
fn late(ms: Int) -> Int {
  with SystemClock { (use Clock).sleep(ms) }
  ms * 2
}

@export(js, "late_boom")
fn late_boom(n: Int) -> Int {
  with SystemClock { (use Clock).sleep(1) }
  10 / n
}

@export(js, "default", "run")
fn run(app: fn(Int) -> Int fails {} uses {}, x: Int) -> Int { app(x) + 1 }

@entry(run)
fn twice(x: Int) -> Int { x * 2 }

pub fn hidden() -> Int { 1 }

fn main() {
  dbg(twice(1))
  ()
}
"##;
    let sources = [("app/Pome.toml", ""), ("app/src/main.emel", main)];
    let fs = MemoryFiles::new(sources.iter().copied());
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let dir = out_dir("js-exports");
    let (analysis, entry) = build(&fs, &input, &mut Lower::default(), &mut CoreJs, &dir);
    // The programs may still write `use E` as an expression (W4401, deprecated).
    assert!(
        analysis
            .diagnostics
            .iter()
            .all(|d| matches!(d.code, Some(emela_driver::code::DEPRECATED_USE))),
        "{:?}",
        analysis.diagnostics
    );
    assert!(entry.is_some());
    let harness = r#"import host, * as m from "./main.mjs";
console.log(m.add(2, 3), host.add(4, 5), host.run(10), Object.keys(host).join(","));
console.log(m.greet("ann", (s) => s + "!"));
console.log(typeof m.hidden, typeof m.twice, typeof m.run);
try { m.half(0); } catch (e) { console.log(e.name, e.message); }
const p = m.late(5);
console.log(p instanceof Promise, await p);
await m.late_boom(0).catch((e) => console.log(e.name, e.message));
"#;
    std::fs::write(dir.join("harness.mjs"), harness).unwrap();
    let out = std::process::Command::new("node")
        .arg("harness.mjs")
        .current_dir(&dir)
        .output()
        .expect("cannot start node");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        (text(&out.stdout), text(&out.stderr)),
        (
            "5 9 21 add,run\nhello, ann!\nundefined undefined undefined\n\
             Defect division by zero\ntrue 10\nDefect division by zero\n",
            ""
        )
    );
}

/// `[target.js.value]`, `[ext.boundary]`: a Bytes crosses as a `Uint8Array`. One from JS (a return
/// value, a value a suspend fn resolves to, an argument of a callback or of an exported fn) is
/// copied, only the bytes its view shows; a subclass (`Buffer`) is accepted, and any other value,
/// including a `Uint8Array` of another realm, is a defect.
#[test]
fn js_bytes_cross_as_uint8_array() {
    let main = r##"import Std.Bytes.{Bytes}

@external(js, "globalThis.hostBytes")
fn host_bytes() -> Bytes uses {}

@external(js, "globalThis.laterBytes")
suspend fn later_bytes() -> Bytes

@external(js, "globalThis.mutate")
fn mutate() -> () uses {}

@external(js, "globalThis.apply")
fn apply(f: fn(Bytes) -> Int) -> Int uses {}

effect Source {
  suspend fn next() -> Bytes
}

handler HostSource: Source {
  suspend fn next(resume) { resume(later_bytes()) }
}

@export(js, "size")
fn size(b: Bytes) -> Int { Bytes.size(b) }

@export(js, "echo")
fn echo(b: Bytes) -> Bytes { Bytes.append(b, Bytes.hex("ff")) }

@export(js, "fromHost")
fn from_host() -> String { Bytes.to_hex(host_bytes()) }

@export(js, "copied")
fn copied() -> String {
  b = host_bytes()
  mutate()
  Bytes.to_hex(b)
}

@export(js, "later")
fn later() -> String { with HostSource { Bytes.to_hex((use Source).next()) } }

@export(js, "applied")
fn applied() -> Int { apply(Bytes.size) }

fn main() {
  dbg(size(Bytes.empty()))
  ()
}
"##;
    let sources = [("app/Pome.toml", ""), ("app/src/main.emel", main)];
    let fs = MemoryFiles::new(sources.iter().copied());
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let dir = out_dir("js-bytes");
    let (analysis, entry) = build(&fs, &input, &mut Lower::default(), &mut CoreJs, &dir);
    // The programs may still write `use E` as an expression (W4401, deprecated).
    assert!(
        analysis
            .diagnostics
            .iter()
            .all(|d| matches!(d.code, Some(emela_driver::code::DEPRECATED_USE))),
        "{:?}",
        analysis.diagnostics
    );
    assert!(entry.is_some());
    let harness = r#"import vm from "node:vm";
import * as m from "./main.mjs";
const show = (f) => { try { console.log(f()); } catch (e) { console.log(e.name, e.message); } };
const buf = new Uint8Array([0, 1, 2, 3, 4, 5]);
globalThis.hostBytes = () => buf.subarray(2, 4);
globalThis.mutate = () => { buf[2] = 99; };
show(() => m.fromHost());
show(() => m.copied());
globalThis.hostBytes = () => Buffer.from([0xab]);
show(() => m.fromHost());
const out = m.echo(new Uint8Array([1, 2]));
console.log(out instanceof Uint8Array, Array.from(out).join(","));
show(() => m.size(Buffer.from("abc")));
show(() => m.size(new ArrayBuffer(2)));
show(() => m.size([1, 2]));
show(() => m.size(new Int8Array(2)));
show(() => m.size(vm.runInNewContext("new Uint8Array(2)")));
globalThis.hostBytes = () => "0203";
show(() => m.fromHost());
globalThis.apply = (f) => f(new Uint8Array(3));
show(() => m.applied());
globalThis.apply = (f) => f(new DataView(new ArrayBuffer(3)));
show(() => m.applied());
globalThis.laterBytes = async () => new Uint8Array([7]);
console.log(await m.later());
globalThis.laterBytes = async () => 7;
await m.later().catch((e) => console.log(e.name, e.message));
"#;
    std::fs::write(dir.join("harness.mjs"), harness).unwrap();
    let out = std::process::Command::new("node")
        .arg("harness.mjs")
        .current_dir(&dir)
        .output()
        .expect("cannot start node");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        (text(&out.stdout), text(&out.stderr)),
        (
            "0203\n0203\nab\ntrue 1,2,255\n3\n\
             Defect argument `b` of `size` is not a `Uint8Array`\n\
             Defect argument `b` of `size` is not a `Uint8Array`\n\
             Defect argument `b` of `size` is not a `Uint8Array`\n\
             Defect argument `b` of `size` is not a `Uint8Array`\n\
             Defect external function `host_bytes` returned a value that is not a `Uint8Array`\n\
             3\n\
             Defect argument `1` of `apply` is not a `Uint8Array`\n\
             07\n\
             Defect external function `later_bytes` returned a value that is not a `Uint8Array`\n",
            ""
        )
    );
}

/// The entry's `main` is exported for the launcher even when it also has a host export, so
/// `emela run` still calls it.
#[test]
fn main_with_a_host_export_runs() {
    let output = run_source(
        "main-host-export",
        "@export(js, \"run\")\nfn main() -> () {\n  dbg(41)\n  ()\n}\n",
    );
    assert_eq!(text(&output.stdout), "");
    assert_eq!(text(&output.stderr), "41\n");
    assert_eq!(output.code, 0);
}

/// `[target.js.value]`, `[ext.boundary]`: an Array crosses as a JS `Array` whose elements are
/// represented by the same table. One from JS (a return value, a value a suspend fn resolves to, an
/// argument of a callback or of an exported fn) must satisfy `Array.isArray`; it is copied, each
/// element converted as a single value would be (a String normalized, a nested Array and a Bytes
/// checked and copied), and a hole read as `undefined`. An Array passed to JS is a JS `Array`.
#[test]
fn js_arrays_cross_as_js_arrays() {
    let main = r##"import Std.Array.{Array}
import Std.Bytes.{Bytes}

@external(js, "globalThis.hostRows")
fn host_rows() -> Array[String] uses {}

@external(js, "globalThis.hostGrid")
fn host_grid() -> Array[Array[Int]] uses {}

@external(js, "globalThis.hostChunks")
fn host_chunks() -> Array[Bytes] uses {}

@external(js, "globalThis.hostUnits")
fn host_units() -> Array[()] uses {}

@external(js, "globalThis.laterRows")
suspend fn later_rows() -> Array[String]

@external(js, "globalThis.mutate")
fn mutate() -> () uses {}

@external(js, "globalThis.sum")
fn sum(xs: Array[Int]) -> Int uses {}

@external(js, "globalThis.apply")
fn apply(f: fn(Array[Int]) -> Int) -> Int uses {}

effect Source {
  suspend fn next() -> Array[String]
}

handler HostSource: Source {
  suspend fn next(resume) { resume(later_rows()) }
}

@export(js, "size")
fn size(a: Array[Int]) -> Int { Array.size(a) }

@export(js, "echo")
fn echo(a: Array[Array[String]]) -> Array[Array[String]] {
  Array.append(a, Array.from_list([Array.from_list(["end"])]))
}

@export(js, "rows")
fn rows() -> String { "#{host_rows()}" }

@export(js, "copied")
fn copied() -> String {
  a = host_rows()
  mutate()
  "#{a}"
}

@export(js, "grid")
fn grid() -> String { "#{host_grid()}" }

@export(js, "chunks")
fn chunks() -> Int { Array.size(host_chunks()) }

@export(js, "units")
fn units() -> Int { Array.size(host_units()) }

@export(js, "summed")
fn summed() -> Int { sum(Array.from_list([1, 2, 3])) }

@export(js, "later")
fn later() -> String { with HostSource { "#{(use Source).next()}" } }

@export(js, "applied")
fn applied() -> Int { apply(Array.size) }

fn main() {
  dbg(size(Array.empty()))
  ()
}
"##;
    let sources = [("app/Pome.toml", ""), ("app/src/main.emel", main)];
    let fs = MemoryFiles::new(sources.iter().copied());
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let dir = out_dir("js-array");
    let (analysis, entry) = build(&fs, &input, &mut Lower::default(), &mut CoreJs, &dir);
    // The programs may still write `use E` as an expression (W4401, deprecated).
    assert!(
        analysis
            .diagnostics
            .iter()
            .all(|d| matches!(d.code, Some(emela_driver::code::DEPRECATED_USE))),
        "{:?}",
        analysis.diagnostics
    );
    assert!(entry.is_some());
    let harness = r#"import * as m from "./main.mjs";
const show = (f) => { try { console.log(f()); } catch (e) { console.log(e.name, e.message); } };
const rows = ["a", "b\uD800"];
globalThis.hostRows = () => rows;
globalThis.mutate = () => { rows[0] = "changed"; rows.push("more"); };
show(() => m.rows());
show(() => m.copied());
globalThis.hostRows = () => new Int32Array(2);
show(() => m.rows());
globalThis.hostRows = () => ({ length: 1, 0: "a" });
show(() => m.rows());
globalThis.hostRows = () => "ab";
show(() => m.rows());
globalThis.hostGrid = () => [[1, 2], [], [3]];
show(() => m.grid());
globalThis.hostGrid = () => [[1], 2];
show(() => m.grid());
globalThis.hostGrid = () => [[1], , [2]];
show(() => m.grid());
globalThis.hostChunks = () => [new Uint8Array(1), Buffer.from("ab")];
show(() => m.chunks());
globalThis.hostChunks = () => [new Uint8Array(1), [1]];
show(() => m.chunks());
globalThis.hostUnits = () => new Array(3);
show(() => m.units());
show(() => m.size([1, 2, 3]));
show(() => m.size([]));
show(() => m.size(new Int32Array(2)));
show(() => m.size("abc"));
const out = m.echo([["x"], ["y", "z"]]);
console.log(Array.isArray(out), Array.isArray(out[0]), JSON.stringify(out));
show(() => m.echo([["x"], "y"]));
globalThis.sum = (xs) => { console.log(Array.isArray(xs)); return xs.reduce((a, b) => a + b, 0); };
show(() => m.summed());
globalThis.apply = (f) => f([4, 5]);
show(() => m.applied());
globalThis.apply = (f) => f(new Set([1]));
show(() => m.applied());
globalThis.laterRows = async () => ["later"];
console.log(await m.later());
globalThis.laterRows = async () => "later";
await m.later().catch((e) => console.log(e.name, e.message));
"#;
    std::fs::write(dir.join("harness.mjs"), harness).unwrap();
    let out = std::process::Command::new("node")
        .arg("harness.mjs")
        .current_dir(&dir)
        .output()
        .expect("cannot start node");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        (text(&out.stdout), text(&out.stderr)),
        (
            "Array.from_list([\"a\", \"b\u{FFFD}\"])\n\
             Array.from_list([\"a\", \"b\u{FFFD}\"])\n\
             Defect external function `host_rows` returned a value that is not an array\n\
             Defect external function `host_rows` returned a value that is not an array\n\
             Defect external function `host_rows` returned a value that is not an array\n\
             Array.from_list([Array.from_list([1, 2]), Array.from_list([]), Array.from_list([3])])\n\
             Defect external function `host_grid` returned a value that is not an array\n\
             Defect external function `host_grid` returned a value that is not an array\n\
             2\n\
             Defect external function `host_chunks` returned a value that is not a `Uint8Array`\n\
             3\n\
             3\n\
             0\n\
             Defect argument `a` of `size` is not an array\n\
             Defect argument `a` of `size` is not an array\n\
             true true [[\"x\"],[\"y\",\"z\"],[\"end\"]]\n\
             Defect argument `a` of `echo` is not an array\n\
             true\n\
             6\n\
             2\n\
             Defect argument `1` of `apply` is not an array\n\
             Array.from_list([\"later\"])\n\
             Defect external function `later_rows` returned a value that is not an array\n",
            ""
        )
    );
}

#[test]
fn source_runs_through_lowering() {
    let output = run_source("lower-dbg", "fn main() {\n  dbg(1 + 2)\n  ()\n}\n");
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "", "3\n")
    );
}

#[test]
fn postfix_match_runs_after_pipes_and_before_escape() {
    // Spec 6.5: `x match { ... }` binds weaker than `|>` and chains left to right with escape.
    let output = run_source(
        "postfix-match",
        r#"error Boom(n: Int)

fn load(n: Int) -> Option[Int] fails Boom {
  if n < 0 { fail Boom(n:) } else { Some(n) }
}

fn describe(n: Int) -> String {
  load(n) match {
    Some(0) -> "zero"
    Some(_) -> "some"
    None -> "none"
  } escape {
    Boom(_) -> "boom"
  }
}

fn main() {
  first = [3, 1, 2] |> List.reverse |> List.head match {
    Some(x) -> x
    None -> 0
  }
  dbg(first)
  dbg(describe(0))
  dbg(describe(-1))
  dbg(Some(Some(5)) match {
    Some(inner) -> inner match {
      Some(x) -> x * 2
      None -> 0
    }
    None -> -1
  })
  ()
}
"#,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "", "2\n\"zero\"\n\"boom\"\n10\n")
    );
}

#[test]
fn lowered_calls_reorder_named_args_and_reach_core() {
    // Named arguments are reordered by position, and core's `List.range` is followed and lowered.
    let output = run_source(
        "lower-calls",
        "\
fn main() {
  dbg(List.range(0, 3))
  dbg(scale(by: 3, n: String.length(\"emela\")))
  ()
}

fn scale(n: Int, by: Int) -> Int {
  n * by
}
",
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "", "[0, 1, 2]\n15\n")
    );
}

#[test]
fn lowered_int_arithmetic_wraps() {
    // Int overflow wraps at 32 bits (spec 16.1).
    let output = run_source(
        "lower-wrap",
        "fn main() {\n  dbg(2147483647 + 1)\n  ()\n}\n",
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "", "-2147483648\n")
    );
}
