//! 手で書いた JS を出力段の代わりに差し込み，`emela run` の経路で node を動かす．

use std::path::{Path, PathBuf};

use emela_core::build::*;
use emela_core::{BinOp, Builtin, Expr, FnId, OpTy, StrKind};
use emela_driver::{
    Analysis, Checked, CoreJs, DEFECT_EXIT_CODE, Diagnostic, FileId, Frontend, Input, JsBackend,
    JsOutput, LexOnly, MemoryFiles, Output, OutputFile, Parsed, RunOptions, RunOutput, SourceFile,
    run,
};
use emela_resolve::ModuleId;
use emela_syntax::Lexed;

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

    fn parse(&mut self, _: ModuleId, _: FileId, _: &SourceFile, _: &Lexed) -> Parsed {
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
                    value_part(small, StrKind::Int),
                    lit_part(", fact(100000) = "),
                    value_part(big, StrKind::Int),
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
