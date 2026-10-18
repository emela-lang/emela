//! Plugs hand-written JS in place of the output stage and runs node through the `emela run`
//! path.

use std::path::{Path, PathBuf};

use emela_core::build::*;
use emela_core::{BinOp, Builtin, Expr, FnId, OpTy, Type, TypeId, WithBind};
use emela_driver::{
    Analysis, Backend, BuildOutput, Checked, CoreJs, DEFECT_EXIT_CODE, Diagnostic, FileId,
    Frontend, INTERRUPTED_EXIT_CODE, Input, LexOnly, Lower, MemoryFiles, Output, OutputFile,
    Parsed, RunOptions, RunOutput, SourceFile, build, run,
};
use emela_resolve::ModuleId;

/// An output stage returning fixed JS.
struct HandWritten {
    files: Vec<OutputFile>,
}

impl HandWritten {
    fn main(js: &str) -> Self {
        HandWritten {
            files: vec![
                OutputFile::new("main.mjs", js),
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
    node: PathBuf,
) -> (Analysis, Option<RunOutput>) {
    let fs = MemoryFiles::new(sources.iter().copied());
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let options = RunOptions {
        out_dir: out_dir(name),
        node,
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
        |analysis| reported = Some(analysis.diagnostics.len()),
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
    assert!(
        analysis.diagnostics.is_empty(),
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

#[test]
fn defect_has_its_own_exit_code() {
    let (_, output) = run_js(
        "defect",
        MAIN,
        "import { Defect } from \"./runtime/defect.mjs\";\nconsole.log(\"before\");\nthrow new Defect(\"整数を 0 で割った\");\n",
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
        node: PathBuf::from("node"),
        args: Vec::new(),
        output: Output::Capture,
    };
    let (analysis, output) = run(
        &fs,
        &input,
        &mut HandBuiltIr(module),
        &mut CoreJs,
        &options,
        |_| {},
    );
    let _ = std::fs::remove_dir_all(&options.out_dir);
    assert!(
        analysis.diagnostics.is_empty(),
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
fn run_source_in(name: &str, main: &str, frontend: &mut Lower, node: PathBuf) -> RunOutput {
    let sources = [("app/Pome.toml", ""), ("app/src/main.emel", main)];
    let fs = MemoryFiles::new(sources.iter().copied());
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let options = RunOptions {
        out_dir: out_dir(name),
        node,
        args: Vec::new(),
        output: Output::Capture,
    };
    let (analysis, output) = run(&fs, &input, frontend, &mut CoreJs, &options, |_| {});
    let _ = std::fs::remove_dir_all(&options.out_dir);
    assert!(
        analysis.diagnostics.is_empty(),
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
            "const A: Int = if True { A } else { 0 }\nfn main() { dbg(A) }\n",
        ),
        (
            "const-self-match",
            "const A: Int = True match {\n  True -> A\n  False -> 0\n}\nfn main() { dbg(A) }\n",
        ),
        (
            "const-self-indirect",
            "const A: Int = if True { value() } else { 0 }\nfn value() -> Int { A }\nfn main() { dbg(A) }\n",
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
    // Without main in the entry, run stops with E0230 and does not start node (spec 8.3). Starting
    // it would add a "node not found" diagnostic, so a missing node is passed to check that.
    let sources = [
        ("app/Pome.toml", ""),
        ("app/src/main.emel", "fn helper() { 1 }\n"),
    ];
    let fs = MemoryFiles::new(sources.iter().copied());
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let options = RunOptions {
        out_dir: out_dir("no-main"),
        node: PathBuf::from("/nonexistent/node"),
        args: Vec::new(),
        output: Output::Capture,
    };
    let (analysis, output) = run(
        &fs,
        &input,
        &mut Lower::default(),
        &mut CoreJs,
        &options,
        |_| {},
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
            Some("E0230"),
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
    assert!(
        analysis.diagnostics.is_empty(),
        "{:?}",
        analysis.diagnostics
    );
    assert!(entry.is_some_and(|entry| entry.exists()));
    let _ = std::fs::remove_dir_all(&options.out_dir);
}

/// A generic `pub` fn of the entry is exported to JS callers as its copy for `Any` (lowering
/// monomorphizes the JS output), under its own name, and works for any JS value.
#[test]
fn generic_pub_fns_are_exported_for_any_type() {
    let main = "pub fn apply[A, B](x: A, f: fn(A) -> B) -> B {\n  f(x)\n}\n\n\
                pub fn first_or[A](xs: List[A], d: A) -> A {\n  List.head(xs) |> Option.or(d)\n}\n\n\
                fn main() {\n  dbg(apply(1, fn(n) { n + 1 }))\n}\n";
    let sources = [("app/Pome.toml", ""), ("app/src/main.emel", main)];
    let fs = MemoryFiles::new(sources.iter().copied());
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let dir = out_dir("generic-exports");
    let (analysis, entry) = build(&fs, &input, &mut Lower::default(), &mut CoreJs, &dir);
    assert!(
        analysis.diagnostics.is_empty(),
        "{:?}",
        analysis.diagnostics
    );
    assert!(entry.is_some());
    let harness = "import { apply, first_or } from \"./main.mjs\";\n\
                   console.log(apply(\"a\", (s) => s + \"!\"), apply(2, (n) => n * 3));\n\
                   console.log(first_or(null, \"none\"), first_or({ h: 7, t: null }, 0));\n";
    std::fs::write(dir.join("harness.mjs"), harness).unwrap();
    let out = std::process::Command::new("node")
        .arg("harness.mjs")
        .current_dir(&dir)
        .output()
        .expect("cannot start node");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        (text(&out.stdout), text(&out.stderr)),
        ("a! 6\nnone 7\n", "")
    );
}

#[test]
fn source_runs_through_lowering() {
    let output = run_source("lower-dbg", "fn main() {\n  dbg(1 + 2)\n}\n");
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
    let output = run_source("lower-wrap", "fn main() {\n  dbg(2147483647 + 1)\n}\n");
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "", "-2147483648\n")
    );
}

#[test]
fn unhandled_error_exits_with_1() {
    // An error that reaches main prints `error: <the error shown>` and exits with 1 (spec 14.3),
    // distinct from 101 for a defect.
    let output = run_source(
        "lower-fail",
        "\
error NotFound(id: Int)
error Unauthorized

fn find(id: Int) -> Int fails NotFound | Unauthorized {
  if id < 0 { fail Unauthorized } else if id == 0 { fail NotFound(id:) } else { id }
}

fn main() {
  dbg(find(1))
  find(0)
}
",
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (1, "", "1\nerror: NotFound(id: 0)\n")
    );
    let output = run_source(
        "lower-fail-unit",
        "error Unauthorized\n\nfn main() {\n  fail Unauthorized\n}\n",
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (1, "", "error: Unauthorized\n")
    );
    let output = run_source("lower-defect", "fn main() {\n  panic(\"boom\")\n}\n");
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (DEFECT_EXIT_CODE, "", "defect: boom\n")
    );
}

#[test]
fn errors_are_values_and_escape_recovers() {
    // Error values are ordinary values, shown by interpolation and dbg (spec 7.1, 10.6). An error
    // caught by escape becomes a normal value, and an uncaught one propagates. Errors inside arms
    // propagate too (spec 7.3, 14.3).
    let output = run_source(
        "lower-escape",
        "\
error NotFound(id: Int)
error DbError(message: String)
error Unavailable(reason: String)

fn load(id: Int) -> String fails NotFound | DbError {
  if id < 0 { fail DbError(message: \"down\") } else if id == 0 { fail NotFound(id:) } else { \"user #{id}\" }
}

fn partial(id: Int) {
  load(id) escape { NotFound(_) -> \"guest\" }
}

fn convert(id: Int) -> String fails NotFound | Unavailable {
  load(id) escape { DbError(message:) -> fail Unavailable(reason: message) }
}

fn main() {
  e = NotFound(id: 7)
  dbg(e)
  dbg(\"got #{e} and #{[DbError(message: \"x\")]}\")
  dbg(e == NotFound(id: 7))
  dbg(load(1))
  dbg(load(0) escape { NotFound(id:) -> \"none #{id}\" })
  dbg(Some(load(0)) escape { NotFound(_) -> None })
  dbg(partial(0))
  dbg(partial(-1) escape { DbError(message:) -> \"partial: #{message}\" })
  dbg(convert(-1) escape { Unavailable(reason:) -> \"unavailable: #{reason}\" })
  dbg([1, 2] |> List.get(5) |> Option.or_fail(NotFound(id: 5)) escape { NotFound(id:) -> id * 10 })
  dbg(convert(0))
}
",
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (
            1,
            "",
            "\
NotFound(id: 7)
\"got NotFound(id: 7) and [DbError(message: \\\"x\\\")]\"
True
\"user 1\"
\"none 0\"
None
\"guest\"
\"partial: down\"
\"unavailable: down\"
50
error: NotFound(id: 0)
"
        )
    );
}

#[test]
fn partial_escape_arms_propagate_unmatched_errors() {
    // An error missing arms that catch only some values (field values, guards) propagates as an
    // error, not a defect (spec 7.3). Reaching main gives `error: ...` and exit code 1
    // (spec 14.3).
    let source = |last: &str| {
        format!(
            "\
error NotFound(id: Int)

fn find(id: Int) -> Int fails NotFound {{
  fail NotFound(id:)
}}

fn by_value(id: Int) {{
  find(id) escape {{ NotFound(id: 1) -> 10 }}
}}

fn by_guard(id: Int) {{
  find(id) escape {{ NotFound(id:) if id > 5 -> id * 10 }}
}}

fn main() {{
  dbg(by_value(1))
  dbg(by_guard(6))
  dbg(by_value(2) escape {{ NotFound(id:) -> id + 100 }})
  {last}
}}
"
        )
    };
    let output = run_source("lower-partial-escape-value", &source("by_value(2)"));
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (1, "", "10\n60\n102\nerror: NotFound(id: 2)\n")
    );
    let output = run_source("lower-partial-escape-guard", &source("by_guard(3)"));
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (1, "", "10\n60\n102\nerror: NotFound(id: 3)\n")
    );
}

#[test]
fn failing_tail_calls_do_not_grow_stack() {
    // Fallible self tail calls also become loops (spec 16.3). Calls inside an escape's target are
    // not in tail position.
    let output = run_source(
        "lower-fail-loop",
        "\
error Found(at: Int)

fn search(n: Int, target: Int) -> Int fails Found {
  if n == target { fail Found(at: n) } else if n == 0 { -1 } else { search(n - 1, target) }
}

fn main() {
  dbg(search(1000000, 3) escape { Found(at:) -> at })
  dbg(search(1000000, -5))
}
",
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "", "3\n-1\n")
    );
}

#[test]
fn console_runs_through_handlers() {
    // The bundled Console is provided by SystemConsole (spec 8.3, 15.2). An `Option.or_fail`
    // failure goes through as well.
    let output = run_source(
        "console",
        r#"import Std.Console.{Console, SystemConsole}

error NotFound(id: Int)

fn name(id: Int) -> String fails NotFound {
  List.get(["alice"], id) |> Option.or_fail(NotFound(id:))
}

fn main() {
  with SystemConsole {
    console = use Console
    console.print_line(name(0))
    console.print_line(name(1) escape { NotFound(id:) -> "no #{id}" })
    (use Console).print_error_line("done")
  }
}
"#,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "alice\nno 1\n", "done\n")
    );
}

/// Declarations for end-to-end checks of stateful handlers and layers (spec 8.4, 8.5). `Pool`
/// prints to `Console` on init and release. `Fragile` panics on release.
const STATEFUL: &str = r#"import Std.Console.{Console, SystemConsole}
import Std.Clock.{Clock, SystemClock}

error DbDown

effect Users {
  fn find(id: Int) -> String
}

effect Config {
  fn db_url() -> String
}

effect Wait {
  suspend fn forever() -> ()
}

handler EnvConfig implements Config {
  fn db_url() { "db://app" }
}

handler EmptyConfig implements Config {
  fn db_url() { "" }
}

handler Pool(url: String, opened: Int) implements Users {
  init {
    url = (use Config).db_url()
    if url == "" { fail DbDown }
    (use Console).print_line("open #{url}")
    Pool(url:, opened: 1)
  }

  release {
    (use Console).print_line("close #{self.url}")
  }

  fn find(id) { "user #{id} via #{self.url}" }
}

handler Spare implements Users {
  init {
    (use Console).print_line("open spare")
    Spare
  }

  release { (use Console).print_line("close spare") }

  fn find(id) { "spare #{id}" }
}

handler Fragile implements Wait {
  release { panic("close failed") }
  suspend fn forever(resume) { resume(()) }
}

handler Stuck implements Wait {
  suspend fn forever(resume) { () }
}

layer AppLive {
  Pool
  EnvConfig
}
"#;

fn run_stateful(name: &str, main: &str) -> RunOutput {
    run_source(name, &format!("{STATEFUL}\n{main}"))
}

#[test]
fn stateful_handlers_and_layers_run() {
    // The layer is expanded; init runs in dependency order (EnvConfig, Pool) and release in
    // reverse (spec 8.5, 14.2). A handler overridden by a later one (AppLive's Pool) is not
    // initialized (spec 8.3).
    let output = run_stateful(
        "stateful",
        r#"fn main() {
  with SystemConsole, AppLive {
    (use Console).print_line((use Users).find(1))
  }
  with SystemConsole, AppLive, Spare {
    (use Console).print_line((use Users).find(2))
  }
}
"#,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (
            0,
            "open db://app\nuser 1 via db://app\nclose db://app\n\
             open spare\nspare 2\nclose spare\n",
            ""
        )
    );
}

#[test]
fn failing_init_releases_what_was_initialized() {
    // If Pool's init fails, the body does not run and only the already initialized Spare is
    // released (spec 14.2). The `with` ends with that init error.
    let output = run_stateful(
        "stateful-init-fails",
        r#"handler SpareFirst implements Config {
  init {
    (use Console).print_line("open config")
    SpareFirst
  }
  release { (use Console).print_line("close config") }
  fn db_url() { "" }
}

fn main() {
  with SystemConsole, SpareFirst, Pool {
    (use Console).print_line("body")
  }
}
"#,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (1, "open config\nclose config\n", "error: DbDown\n")
    );
}

#[test]
fn release_defects_follow_the_body() {
    // If the body is a value, the release defect is the result. The remaining releases still run
    // (spec 14.4).
    let output = run_stateful(
        "stateful-release-defect",
        r#"fn main() {
  with SystemConsole, EnvConfig, Pool, Fragile {
    (use Wait).forever()
  }
}
"#,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (
            DEFECT_EXIT_CODE,
            "open db://app\nclose db://app\n",
            "defect: close failed\n"
        )
    );

    // If the body is a defect, the first defect is the result and the release defect becomes a
    // note.
    let output = run_stateful(
        "stateful-release-note",
        r#"fn main() {
  with SystemConsole, Fragile { panic("boom") }
}
"#,
    );
    assert_eq!(
        (output.code, text(&output.stderr)),
        (
            DEFECT_EXIT_CODE,
            "defect: boom\nnote: the release of handler Fragile also failed: close failed\n"
        )
    );
}

#[test]
fn released_handlers_are_defects_when_used() {
    // Calling a capability taken outside the `with` after its release is a defect (spec 8.6).
    let output = run_stateful(
        "stateful-used-after-release",
        r#"fn main() {
  with SystemConsole, EnvConfig {
    f = with Pool {
      users = use Users
      fn() { users.find(1) }
    }
    with Spare { (use Console).print_line(f()) }
  }
}
"#,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (
            DEFECT_EXIT_CODE,
            "open db://app\nclose db://app\nopen spare\nclose spare\n",
            "defect: handler Pool was used after release\n"
        )
    );
}

#[test]
fn interrupted_main_releases_first() {
    // A main unwound while suspended also runs the releases of open `with`s from the inside out,
    // then ends as `interrupted` (spec 14.5). The same holds for a non-suspending main ($sync) and
    // a main that could be resumed later.
    for (name, main) in [
        (
            "stateful-interrupted",
            "fn main() {\n  with SystemConsole, AppLive, Stuck { (use Wait).forever() }\n}\n",
        ),
        (
            "stateful-interrupted-async",
            "fn main() {\n  with SystemConsole, SystemClock, AppLive, Stuck {\n    (use Clock).sleep(1)\n    (use Wait).forever()\n  }\n}\n",
        ),
    ] {
        let output = run_stateful(name, main);
        assert_eq!(
            (output.code, text(&output.stdout), text(&output.stderr)),
            (
                INTERRUPTED_EXIT_CODE,
                "open db://app\nclose db://app\n",
                "interrupted\n"
            ),
            "{name}"
        );
    }
}

#[test]
fn time_values_run_through_core() {
    // An Instant can be made from milliseconds since the Unix epoch (spec 15.2).
    let output = run_source(
        "time",
        r#"import Std.Console.{Console, SystemConsole}
import Std.Instant.{Instant}
import Std.Duration.{Duration}

fn show(o: Ordering) -> String {
  o match {
    Less -> "Less"
    Equal -> "Equal"
    Greater -> "Greater"
  }
}

fn main() {
  with SystemConsole {
    c = use Console
    d = Duration.add(Duration.minutes(2), Duration.seconds(3))
    c.print_line(Duration.to_string(d))
    c.print_line(Int64.to_string(Duration.to_seconds(Duration.milliseconds(-1500))))
    c.print_line(Duration.to_string(Duration.subtract(Duration.hours(1), Duration.minutes(61))))
    c.print_line(show(Duration.compare(Duration.seconds(1), Duration.milliseconds(1000))))
    c.print_line(if Duration.seconds(1) == Duration.milliseconds(1000) { "eq" } else { "ne" })
    start = Instant.from_unix_milliseconds(1760238245678)
    later = start |> Instant.add(Duration.hours(25))
    c.print_line(Instant.to_string(start))
    c.print_line(Instant.to_string(later))
    c.print_line(Duration.to_string(later |> Instant.since(start)))
    c.print_line(show(Instant.compare(start, later)))
    c.print_line(Int64.to_string(Instant.to_unix_milliseconds(Instant.subtract(start, Duration.milliseconds(678)))))
    c.print_line(Instant.to_string(Instant.from_unix_milliseconds(0)))
    c.print_line(Instant.to_string(Instant.from_unix_milliseconds(-1)))
    c.print_line(Instant.to_string(Instant.from_unix_milliseconds(951782400000)))
    c.print_line(Instant.to_string(Instant.from_unix_milliseconds(-62198755200000)))
    c.print_line(Instant.to_string(Instant.from_unix_milliseconds(253402300800000)))
  }
}
"#,
    );
    assert_eq!(text(&output.stderr), "");
    assert_eq!(output.code, 0);
    insta::assert_snapshot!(text(&output.stdout));
}

mod suspend;

#[test]
fn suspended_main_waits_for_promises() {
    // A suspended main waits on the event loop, resumes and finishes.
    let mut m = emela_core::Module::new();
    let resolve = suspend::resolve(&mut m);
    suspend::main(
        &mut m,
        TypeId::INT,
        builtin_ty(
            Builtin::Dbg,
            vec![TypeId::INT],
            vec![bin(
                BinOp::Add,
                OpTy::Int,
                call_suspends(resolve, vec![int(41)]),
                int(1),
            )],
        ),
    );
    let output = run_ir("suspend-promise", m);
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "", "42\n")
    );
}

#[test]
fn dropped_resume_interrupts_main() {
    // A main that ends without a resume gives `interrupted` and exit code 130 (spec 14.5).
    let mut m = emela_core::Module::new();
    let (never, wait) = suspend::never(&mut m);
    let h = m.local("h", Type::Handler(wait));
    suspend::main(
        &mut m,
        TypeId::UNIT,
        Expr::With {
            binds: vec![WithBind::new(h, call(never, vec![]))],
            body: Box::new(suspend::forever(wait, h)),
        },
    );
    let output = run_ir("suspend-interrupted", m);
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (INTERRUPTED_EXIT_CODE, "", "interrupted\n")
    );
}

#[test]
fn dropped_main_releases_before_interrupted() {
    // A main unwound while suspended also runs the releases of open `with`s, then ends as
    // `interrupted` (spec 14.5).
    let mut m = emela_core::Module::new();
    let (never, wait) = suspend::never(&mut m);
    let log = suspend::log(&mut m);
    let (h, r) = (
        m.local("h", Type::Handler(wait)),
        m.local("r", Type::Handler(wait)),
    );
    let pool = suspend::released(&mut m, log, r, "Pool", "released");
    suspend::main(
        &mut m,
        TypeId::UNIT,
        Expr::With {
            binds: vec![pool, WithBind::new(h, call(never, vec![]))],
            body: Box::new(suspend::forever(wait, h)),
        },
    );
    let output = run_ir("release-interrupted", m);
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (INTERRUPTED_EXIT_CODE, "released\n", "interrupted\n")
    );
}

#[test]
fn defects_after_resuming_and_in_callbacks() {
    // A defect after resuming and a defect inside a host callback both end as defects
    // (spec 7.5).
    let mut m = emela_core::Module::new();
    let resolve = suspend::resolve(&mut m);
    let x = m.local("_", TypeId::INT);
    suspend::main(
        &mut m,
        TypeId::NEVER,
        let_(
            x,
            call_suspends(resolve, vec![int(1)]),
            builtin(Builtin::Panic, vec![string("after")]),
        ),
    );
    let output = run_ir("suspend-defect", m);
    assert_eq!(
        (output.code, text(&output.stderr)),
        (DEFECT_EXIT_CODE, "defect: after\n")
    );

    // main stays suspended while a setTimeout callback panics. This is not `interrupted`.
    let mut m = emela_core::Module::new();
    let set_timeout = suspend::set_timeout(&mut m);
    let (never, wait) = suspend::never(&mut m);
    let callback = m.fn_type(vec![], TypeId::UNIT);
    let (h, x) = (
        m.local("h", Type::Handler(wait)),
        m.local("_", TypeId::UNIT),
    );
    suspend::main(
        &mut m,
        TypeId::UNIT,
        Expr::With {
            binds: vec![WithBind::new(h, call(never, vec![]))],
            body: Box::new(let_(
                x,
                call(
                    set_timeout,
                    vec![
                        lambda(
                            vec![],
                            callback,
                            builtin(Builtin::Panic, vec![string("late")]),
                        ),
                        int(0),
                    ],
                ),
                suspend::forever(wait, h),
            )),
        },
    );
    let output = run_ir("suspend-callback-defect", m);
    assert_eq!(
        (output.code, text(&output.stderr)),
        (DEFECT_EXIT_CODE, "defect: late\n")
    );
}

#[test]
fn random_runs_through_handlers() {
    // The bundled Random is provided by its System handler (spec 15.2). Values differ each run, so
    // only the range is checked.
    let output = run_source(
        "random",
        r##"import Std.Console.{Console, SystemConsole}
import Std.Random.{Random, SystemRandom}

fn draw(n: Int) {
  if n > 0 {
    random = use Random
    console = use Console
    console.print_line("#{random.int()} #{random.float()}")
    draw(n - 1)
  }
}

fn main() {
  with SystemConsole, SystemRandom {
    draw(100)
  }
}
"##,
    );
    assert_eq!((output.code, text(&output.stderr)), (0, ""));
    let mut ints = Vec::new();
    for line in text(&output.stdout).lines() {
        let (int, float) = line.split_once(' ').unwrap();
        ints.push(int.parse::<i32>().unwrap());
        let float: f64 = float.parse().unwrap();
        assert!((0.0..1.0).contains(&float), "{float} is not in [0, 1)");
    }
    assert_eq!(ints.len(), 100);
    ints.sort();
    ints.dedup();
    assert!(ints.len() > 1, "all 100 values are the same");
}

/// Writes a script that sets environment variables and then starts node, returning its path.
/// Calling `std::env::set_var` inside a test would race with parallel tests, so the variables
/// are passed to node only.
#[cfg(unix)]
fn node_with_env(name: &str, exports: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let dir = out_dir(&format!("{name}-node"));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("node");
    std::fs::write(&path, format!("#!/bin/sh\n{exports}\nexec node \"$@\"\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn env_runs_through_handlers() {
    // The bundled Env is provided by its System handler (spec 15.2). An empty value is
    // distinguished from an unset one. The second half reads other values just by swapping the
    // handler (dependency injection, spec chapter 13).
    let node = node_with_env(
        "env",
        "export EMELA_TEST_SET=hello EMELA_TEST_EMPTY=\nunset EMELA_TEST_UNSET",
    );
    let output = run_source_in(
        "env",
        r##"import Std.Console.{Console, SystemConsole}
import Std.Env.{Env, SystemEnv}

fn show(name: String) -> String {
  (use Env).get(name) match {
    Some(value) -> "#{name}=#{value}"
    None -> "#{name} is unset"
  }
}

handler FixedEnv implements Env {
  fn get(name) { Some("fixed") }
}

fn main() {
  with SystemConsole, SystemEnv {
    console = use Console
    console.print_line(show("EMELA_TEST_SET"))
    console.print_line(show("EMELA_TEST_EMPTY"))
    console.print_line(show("EMELA_TEST_UNSET"))
  }
  with SystemConsole, FixedEnv {
    (use Console).print_line(show("EMELA_TEST_UNSET"))
  }
}
"##,
        &mut Lower::default(),
        node.clone(),
    );
    let _ = std::fs::remove_dir_all(node.parent().unwrap());
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (
            0,
            "EMELA_TEST_SET=hello\nEMELA_TEST_EMPTY=\nEMELA_TEST_UNSET is unset\nEMELA_TEST_UNSET=fixed\n",
            ""
        )
    );
}

#[test]
fn clock_runs_through_js_clock() {
    // The bundled Clock is provided by SystemClock (`Date.now()`, the bundled `.mjs`) (spec 8.1,
    // 15.2). The time differs each run, so only direction and range are checked.
    let output = run_source(
        "clock",
        r#"import Std.Console.{Console, SystemConsole}
import Std.Clock.{Clock, SystemClock}
import Std.Instant.{Instant}
import Std.Duration.{Duration}

fn elapsed(start: Instant) -> Duration use { Clock } {
  (use Clock).now() |> Instant.since(start)
}

fn main() {
  with SystemConsole, SystemClock {
    c = use Console
    start = (use Clock).now()
    d = elapsed(start)
    c.print_line(if Instant.to_unix_milliseconds(start) > 1700000000000 { "after 2023" } else { "before" })
    c.print_line(if Duration.to_milliseconds(d) >= 0 { "forward" } else { "backward" })
    c.print_line(if String.ends_with(Instant.to_string(start), "Z") { "iso" } else { "other" })
  }
}
"#,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "after 2023\nforward\niso\n", "")
    );
}

#[test]
fn clock_sleeps_through_js_clock() {
    // SystemClock's sleep resumes after setTimeout (spec 8.4, 8.7), and main waits to be resumed
    // before finishing. It also waits inside List.map given a suspending function (the suspending
    // copy).
    let output = run_source(
        "clock-sleep",
        r##"import Std.Console.{Console, SystemConsole}
import Std.Clock.{Clock, SystemClock}
import Std.Instant.{Instant}
import Std.Duration.{Duration}

fn nap(clock_name: String, ms: Int) -> String use { Clock } {
  clock = use Clock
  start = clock.now()
  clock.sleep(ms)
  waited = Duration.to_milliseconds(clock.now() |> Instant.since(start))
  if waited >= Int64.from_int(ms - 5) { "#{clock_name} waited" } else { "#{clock_name} did not wait" }
}

fn main() {
  with SystemConsole, SystemClock {
    c = use Console
    c.print_line(nap("js", 50))
    List.map([30, 10, 20], fn(ms) { nap("js #{ms}", ms) }) |> List.map(c.print_line)
    c.print_line("done")
  }
}
"##,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (
            0,
            "js waited\njs 30 waited\njs 10 waited\njs 20 waited\ndone\n",
            ""
        )
    );
}

#[test]
fn system_clock_sleeps_zero_for_a_negative_ms() {
    // A negative ms counts as 0 (spec 8.8). SystemClock clamps before passing it to the host, so
    // Node 24's TimeoutNegativeWarning does not appear on stderr.
    let output = run_source(
        "clock-negative-sleep",
        r##"import Std.Console.{Console, SystemConsole}
import Std.Clock.{Clock, SystemClock}

fn main() {
  with SystemConsole, SystemClock {
    (use Clock).sleep(-5)
    (use Console).print_line("woke")
  }
}
"##,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "woke\n", "")
    );
}

#[test]
fn state_run_returns_value_and_final_state() {
    // State.run (spec 8.8) returns the body's value paired with the final state. States with
    // different type arguments are different effects, and a generic fn (`modify[S]`) picks the
    // caller's State[Int] or State[String] (spec 8.1). If the body fails, the state is discarded
    // and only the error propagates. The function passed to update runs in one step.
    let output = run_source(
        "state-run",
        r##"import Std.Console.{Console, SystemConsole}
import Std.State.{State}

error Boom

fn count() -> Int use { State[Int] } {
  st = use State[Int]
  st.update(fn(n) { n + 1 })
  st.get()
}

fn modify[S](f: fn(S) -> S fails {} use {}) -> S use State[S] {
  (use State[S]).update(f)
  (use State[S]).get()
}

fn twice[T](f: fn(T) -> T fails {} use {}) -> T use State[T] {
  modify(f)
  modify(f)
}

fn both() -> String use { State[Int], State[String] } {
  n = modify(fn(n) { n + 1 })
  (use State[String]).set("n=#{n}")
  twice(fn(s) { String.append(s, "!") })
}

fn peek() { modify(fn(x) { x }) }

fn failing() -> Int fails Boom {
  (_, last) = State.run(1, fn() {
    (use State[Int]).set(5)
    fail Boom
  })
  last
}

fn main() {
  with SystemConsole {
    c = use Console
    (n, last) = State.run(0, fn() { count() + count() })
    c.print_line("#{n} #{last}")
    ((s, inner), outer) = State.run(10, fn() { State.run("", fn() { both() }) })
    c.print_line("#{s} #{inner} #{outer}")
    (v, same) = State.run(7, fn() { peek() })
    c.print_line("#{v} #{same}")
    f = modify
    (g, h) = State.run(3, fn() { f(fn(k) { k * 4 }) })
    c.print_line("#{g} #{h}")
    c.print_line("#{failing() escape { Boom -> -1 }}")
  }
}
"##,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "3 2\nn=11!! n=11!! 11\n7 7\n12 12\n-1\n", "")
    );
}

#[test]
fn test_handlers_record_output_and_move_the_clock() {
    // The bundled TestConsole collects output in State[List[String]] in write order, and TestClock
    // runs on virtual time in State[Instant] (spec 8.8, 15.2). With one fiber, the result is the
    // same as advancing time by each sleep and continuing.
    let output = run_source(
        "state-test-handlers",
        r##"import Std.Console.{Console, SystemConsole, TestConsole}
import Std.Clock.{Clock, TestClock}
import Std.State.{State}
import Std.Instant.{Instant}
import Std.Duration.{Duration}

fn greet(name: String) -> () use { Console } {
  (use Console).print_line("Hello, #{name}!")
  (use Console).print_error_line("dropped")
}

fn elapsed() -> Duration use { Clock } {
  clock = use Clock
  start = clock.now()
  clock.sleep(1500)
  clock.sleep(-3)
  clock.now() |> Instant.since(start)
}

fn main() {
  (_, lines) = State.run([], fn() {
    with TestConsole {
      greet("alice")
      greet("bob")
    }
  })
  start = Instant.from_unix_milliseconds(Int64.from_int(1000))
  (d, end) = State.run(start, fn() { with TestClock { elapsed() } })
  with SystemConsole {
    c = use Console
    c.print_line(String.join(lines, "|"))
    c.print_line("#{Duration.to_milliseconds(d)} #{Instant.to_unix_milliseconds(end)}")
  }
}
"##,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "Hello, alice!|Hello, bob!\n1500 2500\n", "")
    );
}

#[test]
fn state_run_keeps_the_state_across_suspension() {
    // Even if the body suspends on SystemClock (spec 8.7, 14.5), State.run's state carries over
    // after resuming.
    let output = run_source(
        "state-suspend",
        r##"import Std.Console.{Console, SystemConsole}
import Std.Clock.{Clock, SystemClock}
import Std.State.{State}

fn tick() -> () use { State[Int], Clock } {
  (use Clock).sleep(10)
  (use State[Int]).update(fn(n) { n + 1 })
}

fn main() {
  (_, n) = State.run(0, fn() {
    with SystemClock {
      tick()
      tick()
      List.map([1, 2, 3], fn(x) { tick() })
    }
  })
  with SystemConsole { (use Console).print_line("#{n}") }
}
"##,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "5\n", "")
    );
}

#[test]
fn platform_layer_provides_system_handlers() {
    // `Std.Platform.Platform` is a layer bundling the System handlers (spec 15.2). Some can be
    // replaced by Test handlers listed after it (spec 8.3). std modules are used through imports.
    let output = run_source(
        "platform",
        r##"import Std.Platform.{Platform}
import Std.Console.{Console, TestConsole}
import Std.Clock.{Clock, TestClock}
import Std.Instant
import Std.State.{State}

fn main() {
  with Platform {
    now = Instant.to_unix_milliseconds((use Clock).now())
    (use Console).print_line("now > 0: #{now > Int64.from_int(0)}")
  }
  start = Instant.from_unix_milliseconds(Int64.from_int(5))
  (_, end) = State.run(start, fn() { with Platform, TestClock { (use Clock).sleep(10) } })
  (_, lines) = State.run([], fn() { with Platform, TestConsole { (use Console).print_line("x") } })
  with Platform {
    (use Console).print_line("#{Instant.to_unix_milliseconds(end)} #{lines}")
  }
}
"##,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "now > 0: True\n15 [\"x\"]\n", "")
    );
}

#[test]
fn parse_and_conversions() {
    // The spec 15.3 API: parse fails with ParseError on malformed input; String.concat and
    // List.flatten flatten; String.append joins two; conversions live on the depending side
    // (Float.from_int, Int64.from_int).
    let output = run_source(
        "parse",
        r##"import Std.Console.{Console, SystemConsole}

fn int(s: String) -> String {
  Int.to_string(Int.parse(s)) escape { ParseError(input:) -> "bad #{input}" }
}

fn float(s: String) -> String {
  Float.to_string(Float.parse(s)) escape { ParseError(input:) -> "bad #{input}" }
}

fn main() {
  with SystemConsole {
    c = use Console
    c.print_line(String.join(List.map(["42", "-7", "+3", "2147483647", "-2147483648"], int), " "))
    c.print_line(String.join(List.map(["", " 1", "1.0", "2147483648", "0x10", "-"], int), "|"))
    c.print_line(String.join(List.map(["1.5", "-0.25", "3", "1e+21", "Infinity", "NaN"], float), " "))
    c.print_line(String.join(List.map(["", "1.", ".5", "1e", "inf"], float), "|"))
    c.print_line(String.concat(["a", "b", "c"]))
    c.print_line(String.append("ab", "cd"))
    c.print_line("#{List.flatten([[1], [], [2, 3]])}")
    c.print_line("#{Float.from_int(7)} #{Int64.from_int(-1)}")
    c.print_line("#{Option.or(None, 5)} #{List.fold([1, 2, 3], from: 10, f: fn(a, b) { a + b })}")
  }
}
"##,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (
            0,
            "42 -7 3 2147483647 -2147483648\n\
             bad |bad  1|bad 1.0|bad 2147483648|bad 0x10|bad -\n\
             1.5 -0.25 3.0 1e+21 Infinity NaN\n\
             bad |bad 1.|bad .5|bad 1e|bad inf\n\
             abc\nabcd\n[1, 2, 3]\n7.0 -1\n5 16\n",
            ""
        )
    );
}

#[test]
fn test_http_answers_from_the_test_server() {
    // The bundled TestHttp decides responses by passing the request and its attempt number (from
    // 0) to the respond of State[TestServer]; `None` becomes ConnectionFailed. Sent requests are
    // appended to sent in order (spec 15.2). Functions above send (get, post, header, headers,
    // require_ok, encode_query) are plain value functions callable without a handler.
    let output = run_source(
        "http-test",
        r##"import Std.Console.{Console, SystemConsole}
import Std.Http.{Http, Get, Post, Delete, Request, Response, TestServer, TestHttp, ConnectionFailed, UnexpectedStatus}
import Std.State.{State}

fn reply(req: Request, n: Int) -> Option[Response] {
  if n >= 2 {
    None
  } else {
    Some(Response(status: 200 + n, headers: [("X-A", "1"), ("x-a", "2")], body: "#{n}:#{req.body}"))
  }
}

fn method(req: Request) -> String {
  req.method match {
    Get -> "GET"
    Post -> "POST"
    _ -> "other"
  }
}

fn main() {
  server = TestServer(respond: reply, sent: [])
  (out, after) = State.run(server, fn() {
    with TestHttp {
      a = Http.get("https://example.com/a")
      b = Http.post("https://example.com/b", "body")
      c = "#{Http.get("https://example.com/c").status}" escape {
        ConnectionFailed(url:, message:) -> "failed #{url}"
      }
      "#{a.status} #{a.body} | #{b.status} #{b.body} | #{c} | #{Http.header(a, "x-A")} #{Http.headers(b, "X-a")} #{Http.header(a, "none")}"
    }
  })
  with SystemConsole {
    c = use Console
    c.print_line(out)
    c.print_line(String.join(List.map(after.sent, fn(req) { "#{method(req)} #{req.url} #{req.body}" }), ", "))
    c.print_line(Http.encode_query([("q", "東 京"), ("a&b", "c=d"), ("page", "2")]))
    c.print_line("#{Http.require_ok(Http.response(204, "")).status}")
    c.print_line("#{Http.require_ok(Http.response(404, "nf")).status}" escape {
      UnexpectedStatus(status:, body:) -> "unexpected #{status} #{body}"
    })
    r = Http.request(Delete, "u") |> Http.add_header("a", "1") |> Http.add_header("a", "2")
    c.print_line("#{r.headers} #{r.body == ""}")
  }
}
"##,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (
            0,
            "200 0: | 201 1:body | failed https://example.com/c | Some(\"1\") [\"1\", \"2\"] None\n\
             GET https://example.com/a , POST https://example.com/b body, GET https://example.com/c \n\
             q=%E6%9D%B1%20%E4%BA%AC&a%26b=c%3Dd&page=2\n\
             204\n\
             unexpected 404 nf\n\
             [(\"a\", \"1\"), (\"a\", \"2\")] True\n",
            ""
        )
    );
}

/// A test server on 127.0.0.1 using node's `http`. Stopped on drop.
struct LocalServer {
    child: std::process::Child,
    dir: PathBuf,
    port: u16,
    /// A port opened and closed immediately, used as a target that refuses connections.
    closed_port: u16,
}

impl LocalServer {
    fn start(name: &str) -> Self {
        use std::io::BufRead;

        let dir = out_dir(&format!("{name}-server"));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("server.mjs");
        std::fs::write(&script, LOCAL_SERVER).unwrap();
        let mut child = std::process::Command::new("node")
            .arg(&script)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let (port, closed_port) = line.trim().split_once(' ').unwrap();
        LocalServer {
            child,
            dir,
            port: port.parse().unwrap(),
            closed_port: closed_port.parse().unwrap(),
        }
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A server returning fixed responses per path. `/echo` returns the request's method,
/// content-type, x-one and body, and sets several headers with the same name. On start it writes
/// the listening port and the closed port on one line.
const LOCAL_SERVER: &str = r#"import http from "node:http";
const closed = http.createServer();
closed.listen(0, "127.0.0.1", () => {
  const closedPort = closed.address().port;
  closed.close(() => {
    const server = http.createServer((req, res) => {
      let body = "";
      req.setEncoding("utf8");
      req.on("data", (c) => (body += c));
      req.on("end", () => {
        switch (req.url) {
          case "/echo":
            res.setHeader("Set-Cookie", ["a=1", "b=2"]);
            res.setHeader("X-Multi", ["p", "q"]);
            res.setHeader("Content-Type", "text/plain; charset=utf-8");
            res.end(`${req.method} ${req.headers["content-type"] ?? "-"} ${req.headers["x-one"] ?? "-"} ${body}`);
            break;
          case "/binary":
            res.end(Buffer.from([0xff, 0xfe, 0x00]));
            break;
          case "/redirect":
            res.writeHead(302, { Location: "/echo" });
            res.end();
            break;
          case "/loop":
            res.writeHead(302, { Location: "/loop" });
            res.end();
            break;
          default:
            res.writeHead(404);
            res.end("not found 東");
        }
      });
    });
    server.listen(0, "127.0.0.1", () => console.log(`${server.address().port} ${closedPort}`));
  });
});
"#;

#[test]
fn system_http_talks_to_a_local_server() {
    // Points the bundled SystemHttp (fetch, spec 15.2) at the local server; nothing leaves the
    // machine. Response headers are lowercased and sorted by name, same-named headers are joined
    // with `, `, and set-cookie stays separate. date and others change every time, so only chosen
    // names are checked. 4xx is not a failure. Redirects are followed. Connection failure messages
    // vary across Node versions, so only the cause code on a refused connection is checked.
    let server = LocalServer::start("http-system");
    let base = format!("http://127.0.0.1:{}", server.port);
    let closed = format!("http://127.0.0.1:{}/", server.closed_port);
    let main = r##"import Std.Console.{Console}
import Std.Platform.{Platform}
import Std.Http.{Http, Get, Head, Post, Request, InvalidUrl, ConnectionFailed, ResponseNotText}

fn picked(headers: List[(String, String)]) -> List[(String, String)] {
  List.filter(headers, fn(pair) {
    (name, _) = pair
    name == "content-type" || name == "set-cookie" || name == "x-multi"
  })
}

fn show(req: Request) -> String fails InvalidUrl | ConnectionFailed | ResponseNotText use { Http } {
  res = (use Http).send(req)
  "#{res.status} #{picked(res.headers)} #{res.body}"
}

fn attempt(req: Request) -> String use { Http } {
  show(req) escape {
    InvalidUrl(url:) -> "invalid #{url}"
    ConnectionFailed(url:, message:) -> if String.contains(message, "ECONNREFUSED") { "refused" } else { "failed" }
    ResponseNotText(url:) -> "not text"
  }
}

fn main() {
  base = "BASE"
  with Platform {
    c = use Console
    c.print_line(attempt(Http.request(Get, "#{base}/echo")))
    c.print_line(attempt(Request(body: "東京", ..Http.request(Post, "#{base}/echo")) |> Http.add_header("X-One", "1")))
    c.print_line(attempt(Http.request(Head, "#{base}/echo")))
    c.print_line(attempt(Http.request(Get, "#{base}/redirect")))
    c.print_line(attempt(Http.request(Get, "#{base}/missing")))
    c.print_line(attempt(Http.request(Get, "#{base}/binary")))
    c.print_line(attempt(Http.request(Get, "#{base}/loop")))
    c.print_line(attempt(Http.request(Get, "not a url")))
    c.print_line(attempt(Http.request(Get, "ftp://127.0.0.1/")))
    c.print_line(attempt(Http.request(Get, "CLOSED")))
  }
}
"##
    .replace("BASE", &base)
    .replace("CLOSED", &closed);
    let output = run_source("http-system", &main);
    drop(server);
    let echo = "[(\"content-type\", \"text/plain; charset=utf-8\"), (\"set-cookie\", \"a=1\"), (\"set-cookie\", \"b=2\"), (\"x-multi\", \"p, q\")]";
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (
            0,
            format!(
                "200 {echo} GET - - \n\
                 200 {echo} POST text/plain;charset=UTF-8 1 東京\n\
                 200 {echo} \n\
                 200 {echo} GET - - \n\
                 404 [] not found 東\n\
                 not text\n\
                 failed\n\
                 invalid not a url\n\
                 invalid ftp://127.0.0.1/\n\
                 refused\n"
            )
            .as_str(),
            ""
        )
    );
}
