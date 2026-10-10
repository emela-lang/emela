//! `emela test` の段（仕様 13章）．
//!
//! ソースから `@test` の関数を集める部分（[`WithTests`]）と，Core IR のテスト関数を JS にして
//! node で1つずつ呼ぶ部分（[`TestBackend`]，[`test`]）からなる．ソースからテスト関数の IR を作る
//! lowering はまだないので，その間は [`Frontend`] の `Program` を [`TestModule`] にして差し込む．

mod collect;
mod report;

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use emela_codegen_js::{
    Options, RUNTIME, RUNTIME_FILE, RuntimeMode, TEST_EVENT_PREFIX, TestEntry, emit_module,
    emit_test_main,
};
use emela_core::FnId;
use emela_resolve::{BuiltinModules, ModuleId};
use emela_syntax::Parse;
use serde::Deserialize;

pub use collect::{TEST_ANNOTATION, TestFn, collect_tests, is_test_fn};
pub use report::{TEST_FAILURE_EXIT_CODE, TestOutcome, TestReport, TestResult};

use crate::diagnostic::Diagnostic;
use crate::frontend::{ParseOnly, Resolve};
use crate::js::{CoreJs, JS_ENTRY_FILE};
use crate::output::{JsOutput, OutputFile, write_output};
use crate::pipeline::{Analysis, Checked, Frontend, Input, JsBackend, NoJsBackend, Parsed, check};
use crate::run::{Output, exit_code, spawn_error};
use crate::source::{FileId, FileSystem, SourceFile};

/// プロジェクト（単独ファイルならファイルのあるディレクトリ）からの，テストの JS の既定の出力先．
/// 通常のビルド（`target/emela/js`）とは分ける．テスト関数を export した別の出力になるため．
pub const JS_TEST_OUT_DIR: &str = "target/emela/test";

/// テストの起動用モジュールの出力ファイル名．node はこれを実行する．
pub const JS_TEST_ENTRY_FILE: &str = "emela_test.mjs";

/// `input` のテストの既定の出力先．
pub fn default_test_out_dir(input: &Input) -> PathBuf {
    input.base.join(JS_TEST_OUT_DIR)
}

/// モジュールごとの構文木を引けるフロントエンド．
pub trait ParseTrees {
    fn parse_of(&self, module: ModuleId) -> Option<&Parse>;
}

impl ParseTrees for ParseOnly {
    fn parse_of(&self, module: ModuleId) -> Option<&Parse> {
        ParseOnly::parse_of(self, module)
    }
}

impl<B: BuiltinModules> ParseTrees for Resolve<B> {
    fn parse_of(&self, module: ModuleId) -> Option<&Parse> {
        Resolve::parse_of(self, module)
    }
}

/// [`WithTests`] のプログラム．内側のフロントエンドのプログラムと，ソースの `@test` の関数．
#[derive(Debug)]
pub struct SourceTests<P> {
    pub program: P,
    pub tests: Vec<TestFn>,
}

/// 内側のフロントエンドの検査の後に `@test` の関数を集めるフロントエンド．
/// 誤った `@test`（E0222）は検査の診断の後に足す．
#[derive(Debug, Default)]
pub struct WithTests<F>(pub F);

impl<F: Frontend + ParseTrees> Frontend for WithTests<F> {
    type Program = SourceTests<F::Program>;

    fn parse(&mut self, module: ModuleId, file: FileId, source: &SourceFile) -> Parsed {
        self.0.parse(module, file, source)
    }

    fn check(&mut self, analysis: &Analysis, order: &[ModuleId]) -> Checked<Self::Program> {
        let checked = self.0.check(analysis, order);
        let (tests, test_diagnostics) = collect_tests(analysis, |m| self.0.parse_of(m));
        let mut diagnostics = checked.diagnostics;
        diagnostics.extend(test_diagnostics);
        Checked {
            program: checked
                .program
                .map(|program| SourceTests { program, tests }),
            diagnostics,
        }
    }
}

/// テスト関数を含む Core IR．lowering がテスト用に作る（通常のビルドではテスト関数を除く）．
#[derive(Debug, Clone, Default)]
pub struct TestModule {
    pub module: emela_core::Module,
    /// 宣言の順．
    pub tests: Vec<TestCase>,
}

/// テスト関数．`function` は引数のない関数でなければならない．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestCase {
    /// 結果の表示とフィルタに使う名前（[`TestFn::display_name`]）．
    pub name: String,
    pub function: FnId,
}

/// テストの JS の出力の差し込み口．エラーが1つもないときだけ呼ぶ．
pub trait TestBackend<P> {
    /// `program` のテストの名前．宣言の順．
    fn test_names(&self, program: &P) -> Vec<String>;

    /// `selected`（[`TestBackend::test_names`] の添字，昇順）のテストを順に呼ぶ起動用モジュールと，
    /// それが読むファイルを出す．`entry` が起動用モジュールで，node はこれを直接実行する．
    fn emit_tests(
        &mut self,
        program: &P,
        selected: &[usize],
        analysis: &Analysis,
    ) -> Result<JsOutput, Vec<Diagnostic>>;
}

impl<P> TestBackend<P> for NoJsBackend {
    fn test_names(&self, _: &P) -> Vec<String> {
        Vec::new()
    }

    fn emit_tests(
        &mut self,
        program: &P,
        _: &[usize],
        analysis: &Analysis,
    ) -> Result<JsOutput, Vec<Diagnostic>> {
        JsBackend::emit(self, program, analysis)
    }
}

impl TestBackend<TestModule> for CoreJs {
    fn test_names(&self, program: &TestModule) -> Vec<String> {
        program.tests.iter().map(|t| t.name.clone()).collect()
    }

    fn emit_tests(
        &mut self,
        program: &TestModule,
        selected: &[usize],
        _: &Analysis,
    ) -> Result<JsOutput, Vec<Diagnostic>> {
        let mut module = program.module.clone();
        // 起動用モジュールから呼べるように，選んだテスト関数を Emela の名前で export する．
        for &i in selected {
            module.functions[program.tests[i].function].exported = true;
        }
        emela_core::tail::loopify(&mut module);
        let options = Options {
            runtime: RuntimeMode::Import(None),
        };
        let entries: Vec<TestEntry> = selected
            .iter()
            .map(|&i| {
                let test = &program.tests[i];
                TestEntry {
                    name: &test.name,
                    export: &module.functions[test.function].name,
                }
            })
            .collect();
        let main = emit_test_main(&format!("./{JS_ENTRY_FILE}"), &entries);
        Ok(JsOutput {
            files: vec![
                OutputFile::new(JS_ENTRY_FILE, emit_module(&module, &options)),
                OutputFile::new(RUNTIME_FILE, RUNTIME),
                OutputFile::new(JS_TEST_ENTRY_FILE, main),
            ],
            entry: JS_TEST_ENTRY_FILE.into(),
        })
    }
}

/// `emela test` の実行のしかた．
#[derive(Debug, Clone)]
pub struct TestOptions {
    pub out_dir: PathBuf,
    /// node の実行ファイル（[`crate::node_program`]）．
    pub node: PathBuf,
    /// テストの名前の部分文字列．`None` なら全部を実行する．
    pub filter: Option<String>,
    /// node の標準エラーの扱い．標準出力は結果を読むためにいつも集める．
    pub output: Output,
}

/// `emela test` の段．検査し，テストを JS にして書き出し，node で1つずつ実行する．
///
/// 結果は `out` に `cargo test` に近い形で，届いたものから順に書く．`before_node` は
/// 出力の後，node を起動する前に1度だけ呼ぶ（検査や出力が失敗しても呼ぶ）．CLI はここで診断を出す．
/// 診断でエラーになったか node を起動できなかったときは，結果が `None`．
/// node を起動できなかった診断は `before_node` の後に `Analysis` の末尾へ足す．
pub fn test<F: Frontend, B: TestBackend<F::Program>>(
    fs: &dyn FileSystem,
    input: &Input,
    frontend: &mut F,
    backend: &mut B,
    options: &TestOptions,
    before_node: impl FnOnce(&Analysis),
    out: &mut dyn Write,
) -> (Analysis, Option<TestReport>) {
    let (mut analysis, program) = check(fs, input, frontend);
    let prepared = prepare(&mut analysis, program, backend, options);
    before_node(&analysis);
    let Some((entry, selected, filtered_out)) = prepared else {
        return (analysis, None);
    };
    match run_tests(&options.node, &entry, &selected, options.output, out) {
        Ok((results, stderr)) => {
            let report = TestReport {
                results,
                filtered_out,
                stderr,
            };
            let _ = report::write_summary(out, &report);
            (analysis, Some(report))
        }
        Err(diagnostic) => {
            analysis.diagnostics.push(diagnostic);
            (analysis, None)
        }
    }
}

/// 選んだテストを JS にして書き出す．起動用モジュールのパス，選んだテストの名前，外した数を返す．
fn prepare<P, B: TestBackend<P>>(
    analysis: &mut Analysis,
    program: Option<P>,
    backend: &mut B,
    options: &TestOptions,
) -> Option<(PathBuf, Vec<String>, usize)> {
    if analysis.has_errors() {
        return None;
    }
    let Some(program) = program else {
        analysis.diagnostics.push(Diagnostic::error(
            "internal error: the type checker returned no program",
        ));
        return None;
    };
    let names = backend.test_names(&program);
    let selected: Vec<usize> = names
        .iter()
        .enumerate()
        .filter(|(_, name)| {
            options
                .filter
                .as_deref()
                .is_none_or(|filter| name.contains(filter))
        })
        .map(|(i, _)| i)
        .collect();
    let output = match backend.emit_tests(&program, &selected, analysis) {
        Ok(output) => output,
        Err(diagnostics) => {
            analysis.diagnostics.extend(diagnostics);
            return None;
        }
    };
    let entry = match write_output(&options.out_dir, &output) {
        Ok(entry) => entry,
        Err(diagnostic) => {
            analysis.diagnostics.push(diagnostic);
            return None;
        }
    };
    let filtered_out = names.len() - selected.len();
    let selected = selected.into_iter().map(|i| names[i].clone()).collect();
    Some((entry, selected, filtered_out))
}

/// テストランナーの書く結果の行（`test_runner.mjs`）．
#[derive(Deserialize)]
#[serde(tag = "event", rename_all = "lowercase")]
enum Event {
    Result {
        name: String,
        outcome: Outcome,
        #[serde(default)]
        message: String,
    },
    Done,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Outcome {
    Ok,
    Defect,
    Error,
}

/// 起動用モジュールを node で実行し，結果を集めながら `out` に書く．
/// 結果の行でない標準出力はそのまま `out` に流す．
fn run_tests(
    node: &Path,
    entry: &Path,
    selected: &[String],
    output: Output,
    out: &mut dyn Write,
) -> Result<(Vec<TestResult>, Vec<u8>), Diagnostic> {
    let mut command = Command::new(node);
    command
        .arg(entry)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(match output {
            Output::Inherit => Stdio::inherit(),
            Output::Capture => Stdio::piped(),
        });
    let mut child = command.spawn().map_err(|err| spawn_error(node, &err))?;
    // 標準エラーは別のスレッドで読む（パイプが詰まって node が止まらないように）．
    let stderr = child.stderr.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            buf
        })
    });

    let _ = report::write_header(out, selected.len());
    let mut results = Vec::new();
    let mut finished = false;
    let mut stdout = BufReader::new(child.stdout.take().expect("標準出力はパイプにした"));
    let mut line = Vec::new();
    loop {
        line.clear();
        match stdout.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let event = line
            .strip_prefix(TEST_EVENT_PREFIX.as_bytes())
            .and_then(|json| serde_json::from_slice::<Event>(json).ok());
        match event {
            Some(Event::Result {
                name,
                outcome,
                message,
            }) => {
                let outcome = match outcome {
                    Outcome::Ok => TestOutcome::Passed,
                    Outcome::Defect => TestOutcome::Defect(message),
                    Outcome::Error => TestOutcome::Error(message),
                };
                let result = TestResult { name, outcome };
                let _ = report::write_result(out, &result);
                results.push(result);
            }
            Some(Event::Done) => finished = true,
            None => {
                let _ = out.write_all(&line);
            }
        }
    }
    let status = child.wait().map_err(|err| spawn_error(node, &err))?;
    let stderr = stderr
        .map(|thread| thread.join().unwrap_or_default())
        .unwrap_or_default();
    if !finished || results.len() < selected.len() {
        // 途中で node が終わった（外部関数の `process.exit` や，出力の誤り）．
        // 実行中だったテストを失敗にし，残りは実行しなかった数として添える．
        let code = exit_code(status);
        if let Some(name) = selected.get(results.len()) {
            let result = TestResult {
                name: name.clone(),
                outcome: TestOutcome::Aborted {
                    code,
                    not_run: selected.len() - results.len() - 1,
                },
            };
            let _ = report::write_result(out, &result);
            results.push(result);
        }
    }
    Ok((results, stderr))
}
