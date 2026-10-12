//! 手で書いた JS を出力段の代わりに差し込み，`emela run` の経路で node を動かす．

use std::path::{Path, PathBuf};

use emela_core::build::*;
use emela_core::{BinOp, Builtin, Expr, FnId, OpTy, Type};
use emela_driver::{
    Analysis, Checked, CoreJs, DEFECT_EXIT_CODE, Diagnostic, FileId, Frontend,
    INTERRUPTED_EXIT_CODE, Input, JsBackend, JsOutput, LexOnly, Lower, MemoryFiles, Output,
    OutputFile, Parsed, RunOptions, RunOutput, SourceFile, build, run,
};
use emela_resolve::ModuleId;

/// 決まった JS を返す出力段．
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

impl JsBackend<()> for HandWritten {
    fn emit(&mut self, _: &(), analysis: &Analysis) -> Result<JsOutput, Vec<Diagnostic>> {
        assert!(analysis.entry.is_some());
        Ok(JsOutput {
            files: self.files.clone(),
            entry: "main.mjs".into(),
        })
    }
}

/// テストごとの出力先．前の実行の残りは消す．
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
    // node の前に1度だけ呼ばれる．node を起動できなかった診断はその後に足される．
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

/// 手で組んだ Core IR を型検査の結果の代わりに返すフロントエンド．
struct HandBuiltIr(emela_core::Module);

impl Frontend for HandBuiltIr {
    type Program = emela_core::Module;

    fn parse(&mut self, _: ModuleId, _: FileId, _: &SourceFile) -> Parsed {
        Parsed::default()
    }

    fn check(&mut self, _: &Analysis, _: &[ModuleId]) -> Checked<emela_core::Module> {
        Checked {
            program: Some(self.0.clone()),
            diagnostics: Vec::new(),
        }
    }
}

/// `fn fact(n, acc) = if n == 0 then acc else fact(n - 1, acc * n)` と，
/// `body` を本体にする `main`．
fn fact_module(body: impl FnOnce(FnId) -> Expr) -> emela_core::Module {
    let mut m = emela_core::Module::new();
    let fact = m.declare("fact", false);
    let n = m.local("n");
    let acc = m.local("acc");
    m.define(
        fact,
        vec![n, acc],
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
    m.define(main, vec![], body(fact));
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
    // 末尾再帰を 10 万回回してから，結果を panic のメッセージで外に出す（IR にはまだ入出力がない）．
    let output = run_ir(
        "ir-panic",
        fact_module(|fact| {
            let big = call(fact, vec![int(100000), int(1)]);
            let small = call(fact, vec![int(5), int(1)]);
            builtin(
                Builtin::Panic,
                vec![Expr::Concat(vec![
                    lit_part("fact(5) = "),
                    value_part(small, Type::Int),
                    lit_part(", fact(100000) = "),
                    value_part(big, Type::Int),
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

    // ランタイムの defect（ゼロ除算）も同じ扱い．
    let output = run_ir(
        "ir-div",
        fact_module(|_| bin(BinOp::Div, OpTy::Int, int(1), int(0))),
    );
    assert_eq!(text(&output.stderr), "defect: division by zero\n");
    assert_eq!(output.code, DEFECT_EXIT_CODE);

    // defect がなければ終了コード 0．
    let output = run_ir(
        "ir-ok",
        fact_module(|fact| call(fact, vec![int(10), int(1)])),
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "", "")
    );
}

/// 同梱の JS のファイル（`Module::js_files`）は出力先の `emela_core/` に並べて書き出し，
/// エントリの出力から相対で import する．同梱のファイルどうしの相対の import も動く．
#[test]
fn core_js_files_are_written_next_to_the_output() {
    let mut m = emela_core::Module::new();
    let now = m.declare("HostTest.now", false);
    m.define_external(
        now,
        vec![],
        emela_core::External {
            bindings: vec![emela_core::ExternalBinding {
                target: "js".into(),
                args: vec!["./emela_core/host/now.mjs".into(), "now".into()],
            }],
            params: vec![],
            ret: emela_core::Boundary::Plain,
        },
    );
    let main = m.declare("main", true);
    m.define(
        main,
        vec![],
        builtin(
            Builtin::Panic,
            vec![Expr::Concat(vec![
                lit_part("now = "),
                value_part(call(now, vec![]), Type::Int),
            ])],
        ),
    );
    m.js_files = vec![
        emela_core::JsFile {
            path: "host/now.mjs".into(),
            contents: "import { base } from \"../base.mjs\";\nexport const now = () => base + 1;\n"
                .into(),
        },
        emela_core::JsFile {
            path: "base.mjs".into(),
            contents: "export const base = 41;\n".into(),
        },
    ];
    let output = run_ir("core-js-files", m);
    assert_eq!(text(&output.stderr), "defect: now = 42\n");
    assert_eq!(output.code, DEFECT_EXIT_CODE);
}

/// ソースを `Lower`（名前解決，型検査，lowering）と `CoreJs` で `emela run` の経路に通す．
fn run_source(name: &str, main: &str) -> RunOutput {
    run_source_with(name, main, &mut Lower::default())
}

fn run_source_with(name: &str, main: &str, frontend: &mut Lower) -> RunOutput {
    run_source_in(name, main, frontend, PathBuf::from("node"))
}

/// `run_source_with` と同じで，node の実行ファイルを指定する．
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
fn run_without_main_stops_before_node() {
    // エントリに main がなければ run は E0230 で止まり，node を起動しない（8.3）．
    // 起動すれば「node が見つからない」の診断が足されるので，ない node を渡して確かめる．
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

    // build は main がなくても出力する．
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

#[test]
fn source_runs_through_lowering() {
    let output = run_source("lower-dbg", "fn main() {\n  dbg(1 + 2)\n}\n");
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "", "3\n")
    );
}

#[test]
fn lowered_calls_reorder_named_args_and_reach_core() {
    // 名前付き引数は位置の順に並べ替え，core の `List.range` はたどって写す．
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
    // Int のあふれは 32bit で折り返す（16.1）．
    let output = run_source("lower-wrap", "fn main() {\n  dbg(2147483647 + 1)\n}\n");
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "", "-2147483648\n")
    );
}

#[test]
fn unhandled_error_exits_with_1() {
    // main まで達したエラーは `error: <エラーの値の表示>` を出して終了コード 1（14.3）．
    // defect の 101 とは分ける．
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
    // エラーの値は普通の値で，補間と dbg で表示できる（7.1，10.6）．escape で受けたエラーは通常の値に
    // 戻り，受けないエラーは伝わる．腕の中のエラーも伝わる（7.3，14.3）．
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
    // 一部の値しか受けない腕（フィールドの値，ガード）に合わなかったエラーは，defect でなく
    // エラーのまま伝わる（7.3）．main まで達すれば `error: ...` で終了コード 1（14.3）．
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
    // 失敗しうる自己末尾呼び出しもループになる（16.3）．escape の対象の中の呼び出しは末尾位置でない．
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
    // 同梱の Console を JsConsole で供給する（8.3，15.2）．`Option.or_fail` の失敗も一緒に通す．
    let output = run_source(
        "console",
        r#"error NotFound(id: Int)

fn name(id: Int) -> String fails NotFound {
  List.get(["alice"], id) |> Option.or_fail(NotFound(id:))
}

fn main() {
  with JsConsole {
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

#[test]
fn time_values_run_through_core() {
    // Instant は Unix エポックからのミリ秒で作れる（15.2）．
    let output = run_source(
        "time",
        r#"fn show(o: Ordering) -> String {
  match o {
    Less -> "Less"
    Equal -> "Equal"
    Greater -> "Greater"
  }
}

fn main() {
  with JsConsole {
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
    // 中断した main はイベントループで待ち，再開して終わる．
    let mut m = emela_core::Module::new();
    let resolve = suspend::resolve(&mut m);
    suspend::main(
        &mut m,
        builtin_ty(
            Builtin::Dbg,
            vec![Type::Int],
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
    // resume されないまま終わった main は `interrupted` と終了コード 130（14.5）．
    let mut m = emela_core::Module::new();
    let (never, wait) = suspend::never(&mut m);
    let h = m.local("h");
    suspend::main(
        &mut m,
        Expr::With {
            binds: vec![(h, call(never, vec![]))],
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
fn defects_after_resuming_and_in_callbacks() {
    // 再開した後の defect も，ホストのコールバックの中の defect も，defect として終わる（7.5）．
    let mut m = emela_core::Module::new();
    let resolve = suspend::resolve(&mut m);
    let x = m.local("_");
    suspend::main(
        &mut m,
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

    // main は中断したまま，setTimeout のコールバックが panic する．interrupted にはならない．
    let mut m = emela_core::Module::new();
    let set_timeout = suspend::set_timeout(&mut m);
    let (never, wait) = suspend::never(&mut m);
    let (h, x) = (m.local("h"), m.local("_"));
    suspend::main(
        &mut m,
        Expr::With {
            binds: vec![(h, call(never, vec![]))],
            body: Box::new(let_(
                x,
                call(
                    set_timeout,
                    vec![
                        lambda(vec![], builtin(Builtin::Panic, vec![string("late")])),
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
    // 同梱の Random を JsRandom で供給する（15.2）．値は毎回変わるので範囲だけを見る．
    let output = run_source(
        "random",
        r##"fn draw(n: Int) {
  if n > 0 {
    random = use Random
    console = use Console
    console.print_line("#{random.int()} #{random.float()}")
    draw(n - 1)
  }
}

fn main() {
  with JsConsole, JsRandom {
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
        assert!(
            (0.0..1.0).contains(&float),
            "{float} が 0 以上 1 未満でない"
        );
    }
    assert_eq!(ints.len(), 100);
    ints.sort();
    ints.dedup();
    assert!(ints.len() > 1, "100 回とも同じ値");
}

/// 環境変数を設定してから node を起動するスクリプトを書き，そのパスを返す．
/// テストの中で `std::env::set_var` すると並列のテストと競合するので，node の側にだけ渡す．
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
    // 同梱の Env を JsEnv で供給する（15.2）．空の値と未設定を区別する．
    // 後半はハンドラを差し替えるだけで別の値を読ませる（13章の DI）．
    let node = node_with_env(
        "env",
        "export EMELA_TEST_SET=hello EMELA_TEST_EMPTY=\nunset EMELA_TEST_UNSET",
    );
    let output = run_source_in(
        "env",
        r##"fn show(name: String) -> String {
  match (use Env).get(name) {
    Some(value) -> "#{name}=#{value}"
    None -> "#{name} is unset"
  }
}

handler FixedEnv implements Env {
  fn get(name) { Some("fixed") }
}

fn main() {
  with JsConsole, JsEnv {
    console = use Console
    console.print_line(show("EMELA_TEST_SET"))
    console.print_line(show("EMELA_TEST_EMPTY"))
    console.print_line(show("EMELA_TEST_UNSET"))
  }
  with JsConsole, FixedEnv {
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
