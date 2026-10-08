//! 手で書いた JS を出力段の代わりに差し込み，`emela run` の経路で node を動かす．

use std::path::{Path, PathBuf};

use emela_driver::{
    Analysis, DEFECT_EXIT_CODE, Diagnostic, Input, JsBackend, JsOutput, LexOnly, MemoryFiles,
    Output, OutputFile, RunOptions, RunOutput, run,
};

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
                    "export class EmelaDefect extends Error {\n  constructor(message) { super(message); this.name = \"EmelaDefect\"; }\n}\n",
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
    let result = run(
        &fs,
        &input,
        &mut LexOnly,
        &mut HandWritten::main(js),
        &options,
    );
    let _ = std::fs::remove_dir_all(&options.out_dir);
    result
}

const MAIN: &[(&str, &str)] = &[("app/src/main.emel", "let x = 1\n")];

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap()
}

#[test]
fn prints_stdout() {
    let (analysis, output) = run_js(
        "stdout",
        MAIN,
        "import { EmelaDefect } from \"./runtime/defect.mjs\";\nconsole.log(\"こんにちは\");\nconsole.error(\"to stderr\");\n",
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
        "import { EmelaDefect } from \"./runtime/defect.mjs\";\nconsole.log(\"before\");\nthrow new EmelaDefect(\"整数を 0 で割った\");\n",
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
        &[("app/src/util.emel", "")],
        "console.log(\"never\");\n",
        &[],
    );
    assert!(output.is_none());
    let messages: Vec<_> = analysis
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(messages, ["エントリ `app/src/main.emel` がない"]);
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
    assert_eq!(messages, ["`/nonexistent/node` が見つからない"]);
}
