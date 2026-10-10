//! `emela test` の経路（仕様 13章）．手で組んだ Core IR のテスト関数を型検査の結果の代わりに差し込み，
//! codegen-js と node を通して，結果の表示と終了コードを確かめる．

use std::path::{Path, PathBuf};

use emela_core::build::*;
use emela_core::{Assert, BinOp, Builtin, Expr, Module, OpTy, Type};
use emela_driver::{
    Analysis, Checked, CoreJs, Diagnostic, FileId, Frontend, Input, JsOutput, MemoryFiles,
    NoJsBackend, Output, OutputFile, ParseOnly, Parsed, SourceFile, TestBackend, TestCase,
    TestModule, TestOptions, TestOutcome, TestReport, WithTests, check, test,
};
use emela_resolve::ModuleId;

/// 手で組んだテストの IR を型検査の結果の代わりに返すフロントエンド．
struct HandBuiltTests(TestModule);

impl Frontend for HandBuiltTests {
    type Program = TestModule;

    fn parse(&mut self, _: ModuleId, _: FileId, _: &SourceFile) -> Parsed {
        Parsed::default()
    }

    fn check(&mut self, _: &Analysis, _: &[ModuleId]) -> Checked<TestModule> {
        Checked {
            program: Some(self.0.clone()),
            diagnostics: Vec::new(),
        }
    }
}

const PROJECT: &[(&str, &str)] = &[("app/Pome.toml", ""), ("app/src/main.emel", "")];

/// テストごとの出力先．前の実行の残りは消す．
fn out_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("emela-driver-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// `emela test` の段を通す．（診断，結果，標準出力に書いた表示）を返す．
fn run_tests<P, F: Frontend<Program = P>, B: TestBackend<P>>(
    name: &str,
    sources: &[(&str, &str)],
    frontend: &mut F,
    backend: &mut B,
    filter: Option<&str>,
) -> (Analysis, Option<TestReport>, String) {
    let fs = MemoryFiles::new(sources.iter().copied());
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let options = TestOptions {
        out_dir: out_dir(name),
        node: PathBuf::from("node"),
        filter: filter.map(Into::into),
        output: Output::Capture,
    };
    let mut called = 0;
    let mut out = Vec::new();
    let (analysis, report) = test(
        &fs,
        &input,
        frontend,
        backend,
        &options,
        |_| called += 1,
        &mut out,
    );
    assert_eq!(called, 1);
    let _ = std::fs::remove_dir_all(&options.out_dir);
    (analysis, report, String::from_utf8(out).unwrap())
}

fn run_ir(name: &str, tests: TestModule, filter: Option<&str>) -> (TestReport, String) {
    let (analysis, report, out) = run_tests(
        name,
        PROJECT,
        &mut HandBuiltTests(tests),
        &mut CoreJs,
        filter,
    );
    assert!(
        analysis.diagnostics.is_empty(),
        "{:?}",
        analysis.diagnostics
    );
    (report.unwrap(), out)
}

/// `fn add(a, b) = a + b` と，それを使うテスト（成功，assert の失敗，panic，ゼロ除算，
/// 比較でない assert，JS の予約語の名前の関数）．
fn sample() -> TestModule {
    let mut m = Module::new();
    let add = m.declare("add", false);
    let (a, b) = (m.local("a"), m.local("b"));
    m.define(add, vec![a, b], bin(BinOp::Add, OpTy::Int, var(a), var(b)));
    let mut tests = Vec::new();
    let mut case = |m: &mut Module, fn_name: &str, name: &str, body: Expr| {
        let f = m.declare(fn_name, false);
        m.define(f, vec![], body);
        tests.push(TestCase {
            name: name.to_owned(),
            function: f,
        });
    };
    let add_eq = |n: i32, expected: i32| {
        Assert::compare(
            BinOp::Eq,
            OpTy::Int,
            Type::Int,
            call(add, vec![int(1), int(n)]),
            int(expected),
            &format!("add(1, {n}) == {expected}"),
        )
    };
    case(&mut m, "adds", "adds", add_eq(2, 3));
    case(&mut m, "adds_wrong", "adds_wrong", add_eq(2, 4));
    case(
        &mut m,
        "panics",
        "panics",
        builtin(Builtin::Panic, vec![string("boom")]),
    );
    case(
        &mut m,
        "divides_by_zero",
        "divides_by_zero",
        bin(BinOp::Div, OpTy::Int, int(1), int(0)),
    );
    case(
        &mut m,
        "contains",
        "Util.contains",
        Assert::bool(
            builtin(Builtin::StringContains, vec![string("abc"), string("b")]),
            "String.contains(\"abc\", \"b\")",
        ),
    );
    // JS の予約語と重なる名前は `new$` で出して `new` で export する．
    case(
        &mut m,
        "new",
        "Util.new",
        Assert::compare(
            BinOp::Ne,
            OpTy::String,
            Type::String,
            string("a"),
            string("a"),
            "",
        ),
    );
    TestModule { module: m, tests }
}

#[test]
fn reports_each_test_and_keeps_going_after_failures() {
    let (report, out) = run_ir("sample", sample(), None);
    insta::assert_snapshot!(out);
    assert_eq!((report.passed(), report.failed()), (2, 4));
    assert_eq!(report.exit_code(), 1);
    assert_eq!(
        report.results[1].outcome,
        TestOutcome::Defect("assertion failed: add(1, 2) == 4\n  left: 3\n right: 4".into())
    );
}

#[test]
fn filter_selects_by_substring() {
    let (report, out) = run_ir("filter", sample(), Some("Util."));
    assert_eq!(
        out,
        "running 2 tests\n\
         test Util.contains ... ok\n\
         test Util.new ... FAILED\n\
         \n\
         failures:\n\
         \n\
         ---- Util.new ----\n\
         defect: assertion failed: left != right\n  left: \"a\"\n right: \"a\"\n\
         \n\
         failures:\n    Util.new\n\
         \n\
         test result: FAILED. 1 passed; 1 failed; 4 filtered out\n"
    );
    assert_eq!(report.filtered_out, 4);

    let (report, out) = run_ir("filter-ok", sample(), Some("adds"));
    assert_eq!(report.results.len(), 2);
    assert!(out.ends_with("test result: FAILED. 1 passed; 1 failed; 4 filtered out\n"));

    let (report, out) = run_ir("filter-one", sample(), Some("contains"));
    assert_eq!(
        out,
        "running 1 test\ntest Util.contains ... ok\n\ntest result: ok. 1 passed; 0 failed; 5 filtered out\n"
    );
    assert_eq!(report.exit_code(), 0);
}

#[test]
fn passing_tests_exit_with_zero() {
    let mut tests = sample();
    tests
        .tests
        .retain(|t| t.name == "adds" || t.name == "Util.contains");
    let (report, out) = run_ir("ok", tests, None);
    assert_eq!(
        out,
        "running 2 tests\ntest adds ... ok\ntest Util.contains ... ok\n\ntest result: ok. 2 passed; 0 failed; 0 filtered out\n"
    );
    assert!(report.is_ok());
    assert_eq!(report.exit_code(), 0);
}

#[test]
fn no_tests() {
    let (report, out) = run_ir("none", TestModule::default(), None);
    assert_eq!(
        out,
        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 filtered out\n"
    );
    assert_eq!(report.exit_code(), 0);
}

#[test]
fn stderr_of_tests_is_captured() {
    let mut m = Module::new();
    let f = m.declare("shows", false);
    m.define(
        f,
        vec![],
        builtin_ty(
            Builtin::Dbg,
            vec![Type::list(Type::Int)],
            vec![list(vec![int(1)])],
        ),
    );
    let tests = TestModule {
        module: m,
        tests: vec![TestCase {
            name: "shows".into(),
            function: f,
        }],
    };
    let (report, _) = run_ir("stderr", tests, None);
    assert!(report.is_ok());
    assert_eq!(String::from_utf8(report.stderr).unwrap(), "[1]\n");
}

/// 手で書いた JS のテストを出す出力段．
struct HandWrittenTests {
    names: Vec<&'static str>,
    js: &'static str,
}

impl TestBackend<()> for HandWrittenTests {
    fn test_names(&self, _: &()) -> Vec<String> {
        self.names.iter().map(|n| n.to_string()).collect()
    }

    fn emit_tests(
        &mut self,
        _: &(),
        selected: &[usize],
        _: &Analysis,
    ) -> Result<JsOutput, Vec<Diagnostic>> {
        let entries: Vec<_> = selected
            .iter()
            .map(|&i| emela_codegen_js::TestEntry {
                name: self.names[i],
                export: self.names[i],
            })
            .collect();
        Ok(JsOutput {
            files: vec![
                OutputFile::new("main.mjs", self.js),
                OutputFile::new(
                    "test.mjs",
                    emela_codegen_js::emit_test_main("./main.mjs", &entries),
                ),
            ],
            entry: "test.mjs".into(),
        })
    }
}

#[test]
fn foreign_exceptions_are_defects() {
    let (_, report, out) = run_tests(
        "foreign",
        PROJECT,
        &mut ParseOnly::new(),
        &mut HandWrittenTests {
            names: vec!["throws", "prints"],
            js: "export function throws() { null.x; }\nexport function prints() { console.log(\"from the test\"); }\n",
        },
        None,
    );
    let report = report.unwrap();
    assert!(matches!(
        &report.results[0].outcome,
        TestOutcome::Defect(m) if m.starts_with("TypeError: ")
    ));
    assert!(report.results[1].outcome.is_passed());
    // 結果の行でない標準出力はそのまま流す．
    assert!(out.contains("from the test\ntest prints ... ok\n"), "{out}");
}

#[test]
fn runner_exiting_midway_fails_the_running_test() {
    let (_, report, out) = run_tests(
        "abort",
        PROJECT,
        &mut ParseOnly::new(),
        &mut HandWrittenTests {
            names: vec!["first", "exits", "third", "fourth"],
            js: "export function first() {}\nexport function exits() { process.exit(3); }\nexport function third() {}\nexport function fourth() {}\n",
        },
        None,
    );
    let report = report.unwrap();
    assert_eq!(
        report.results[1].outcome,
        TestOutcome::Aborted {
            code: 3,
            not_run: 2
        }
    );
    assert_eq!(report.exit_code(), 1);
    assert!(
        out.contains(
            "---- exits ----\nthe test runner exited with code 3 while running this test; 2 tests after it did not run\n"
        ),
        "{out}"
    );
}

#[test]
fn missing_node_is_a_diagnostic() {
    let fs = MemoryFiles::new(PROJECT.iter().copied());
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let options = TestOptions {
        out_dir: out_dir("no-node"),
        node: PathBuf::from("/nonexistent/node"),
        filter: None,
        output: Output::Capture,
    };
    let mut out = Vec::new();
    let (analysis, report) = test(
        &fs,
        &input,
        &mut HandBuiltTests(sample()),
        &mut CoreJs,
        &options,
        |_| {},
        &mut out,
    );
    let _ = std::fs::remove_dir_all(&options.out_dir);
    assert!(report.is_none());
    assert_eq!(analysis.diagnostics.len(), 1);
    assert_eq!(
        analysis.diagnostics[0].message,
        "`/nonexistent/node` not found"
    );
}

const SOURCE_TESTS: &[(&str, &str)] = &[
    ("app/Pome.toml", ""),
    (
        "app/src/main.emel",
        "fn add(a: Int, b: Int) -> Int { a + b }\n\n@test\nfn adds() {\n  assert add(1, 2) == 3\n}\n\n@test\nfn takes(x: Int) {\n  assert x == 1\n}\n\n@test\ntype Item(x: Int)\n\n@test()\nfn with_args() {}\n",
    ),
    (
        "app/src/http/client.emel",
        "@test\nfn gets() {\n  assert True\n}\n\n@test\n@external(\"x\")\nfn foreign()\n",
    ),
];

#[test]
fn collects_test_functions_from_source() {
    let fs = MemoryFiles::new(SOURCE_TESTS.iter().copied());
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let (analysis, program) = check(&fs, &input, &mut WithTests(ParseOnly::new()));
    let tests: Vec<_> = program
        .unwrap()
        .tests
        .iter()
        .map(|t| t.display_name.clone())
        .collect();
    // エントリのモジュールは関数名だけ，ほかはモジュール名を付ける．
    assert_eq!(tests, ["Http.Client.gets", "adds"]);
    let rendered = emela_driver::render(&analysis.diagnostics, &analysis.sources, false);
    insta::assert_snapshot!(rendered);
}

#[test]
fn source_tests_stop_before_lowering() {
    // lowering がまだないので，検査を通ったら JS の出力がないことの診断で止まる．
    let sources = &[
        ("app/Pome.toml", ""),
        (
            "app/src/main.emel",
            "@test\nfn adds() {\n  assert 1 + 2 == 3\n}\n",
        ),
    ];
    let (analysis, report, out) = run_tests(
        "source",
        sources,
        &mut WithTests(ParseOnly::new()),
        &mut NoJsBackend,
        None,
    );
    assert!(report.is_none());
    assert_eq!(out, "");
    let codes: Vec<_> = analysis.diagnostics.iter().map(|d| d.code).collect();
    assert_eq!(codes, [Some("E0905")]);
}
