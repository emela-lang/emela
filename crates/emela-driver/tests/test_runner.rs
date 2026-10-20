//! The `emela test` path (spec chapter 13). Hand-built Core IR test functions stand in for the
//! type checking results, and results and exit codes are checked through codegen-js and node.

use std::path::{Path, PathBuf};

use emela_core::build::*;
use emela_core::{Assert, BinOp, Builtin, Expr, Module, OpTy, Type, TypeId, WithBind};
use emela_driver::{
    Analysis, BuildOutput, Checked, CoreJs, Diagnostic, FileId, Frontend, Input, LowerTests,
    MemoryFiles, Output, OutputFile, ParseOnly, Parsed, SourceFile, TestBackend, TestCase,
    TestModule, TestOptions, TestOutcome, TestReport, Typecheck, check, test,
};
use emela_resolve::ModuleId;

mod suspend;

/// A frontend that returns hand-built test IR in place of the type checking results.
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

/// A per-test output directory. Leftovers from earlier runs are removed.
fn out_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("emela-driver-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Runs the `emela test` stage. Returns (diagnostics, report, what was written to stdout).
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

/// `fn add(a, b) = a + b` and tests using it (success, a failing assert, panic, division by zero,
/// a non-comparison assert, and a function named after a JS reserved word).
fn sample() -> TestModule {
    let mut m = Module::new();
    let add = m.declare("add", false);
    let (a, b) = (m.local("a", TypeId::INT), m.local("b", TypeId::INT));
    m.define(
        add,
        vec![a, b],
        TypeId::INT,
        bin(BinOp::Add, OpTy::Int, var(a), var(b)),
    );
    let mut tests = Vec::new();
    let mut case = |m: &mut Module, fn_name: &str, name: &str, body: Expr| {
        // Test functions return `()` (spec 13.1).
        let f = m.declare(fn_name, false);
        m.define(f, vec![], TypeId::UNIT, body);
        tests.push(TestCase {
            name: name.to_owned(),
            function: f,
        });
    };
    let add_eq = |n: i32, expected: i32| {
        Assert::compare(
            BinOp::Eq,
            OpTy::Int,
            TypeId::INT,
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
    // A name clashing with a JS reserved word is emitted as `new$` and exported as `new`.
    case(
        &mut m,
        "new",
        "Util.new",
        Assert::compare(
            BinOp::Ne,
            OpTy::String,
            TypeId::STRING,
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
    let ints = m.list(TypeId::INT);
    m.define(
        f,
        vec![],
        ints,
        builtin_ty(
            Builtin::Dbg,
            vec![ints],
            vec![list(TypeId::INT, vec![int(1)])],
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

/// An output stage emitting hand-written JS tests.
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
    ) -> Result<BuildOutput, Vec<Diagnostic>> {
        let entries: Vec<_> = selected
            .iter()
            .map(|&i| emela_codegen_js::TestEntry {
                name: self.names[i],
                export: self.names[i],
            })
            .collect();
        Ok(BuildOutput {
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
    // Stdout lines that are not result lines pass through.
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
        "@test\nfn gets() {\n  assert True\n}\n\n@test\n@external(js, \"x\")\nfn foreign()\n",
    ),
];

#[test]
fn collects_test_functions_from_source() {
    let fs = MemoryFiles::new(SOURCE_TESTS.iter().copied());
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let (analysis, program) = check(&fs, &input, &mut Typecheck::default());
    let tests: Vec<_> = program
        .unwrap()
        .tests
        .iter()
        .map(|t| t.test.display_name.clone())
        .collect();
    // Just the function name in the entry module; other modules get the module name.
    assert_eq!(tests, ["Http.Client.gets", "adds"]);
    let rendered = emela_driver::render(&analysis.diagnostics, &analysis.sources, false);
    insta::assert_snapshot!(rendered);
}

#[test]
fn source_tests_run_through_lowering() {
    // `@test` functions in source are lowered (`LowerTests`) and run on node.
    let sources = &[
        ("app/Pome.toml", ""),
        (
            "app/src/main.emel",
            "\
fn add(a: Int, b: Int) -> Int { a + b }

@test
fn adds() {
  assert add(1, 2) == 3
}

@test
fn fails() {
  assert add(1, 2) == 4
}
",
        ),
        (
            "app/src/util.emel",
            "@test\nfn in_util() {\n  assert List.map([1], fn(x) { x + 1 }) == [2]\n}\n",
        ),
    ];
    let (analysis, report, out) = run_tests(
        "source",
        sources,
        &mut LowerTests::default(),
        &mut CoreJs,
        None,
    );
    assert_eq!(analysis.diagnostics, []);
    let report = report.unwrap();
    let results: Vec<_> = report
        .results
        .iter()
        .map(|r| (r.name.as_str(), matches!(r.outcome, TestOutcome::Passed)))
        .collect();
    assert_eq!(
        results,
        [("adds", true), ("fails", false), ("Util.in_util", true)]
    );
    insta::assert_snapshot!(out);
}

#[test]
fn test_functions_cannot_be_used_outside_tests() {
    let sources = &[
        ("app/Pome.toml", ""),
        (
            "app/src/main.emel",
            "fn main() {\n  adds()\n}\n\n@test\nfn adds() {\n  assert True\n}\n\n@test\nfn calls_adds() {\n  adds()\n}\n",
        ),
    ];
    let fs = MemoryFiles::new(sources.iter().copied());
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let (analysis, _) = check(&fs, &input, &mut Typecheck::default());
    // References between test functions (`calls_adds`) are allowed.
    let codes: Vec<_> = analysis.diagnostics.iter().map(|d| d.code).collect();
    assert_eq!(codes, [Some("E0227")]);
}

#[test]
fn unhandled_errors_fail_only_that_test() {
    // An unhandled error fails that test, and the other tests continue (spec 13.3). A test
    // function's E need not be empty.
    let sources = &[
        ("app/Pome.toml", ""),
        (
            "app/src/main.emel",
            "\
error NotFound(id: Int) derive Show

fn find(id: Int) -> Int fails NotFound {
  if id > 0 { id } else { fail NotFound(id:) }
}

@test
fn finds() {
  assert find(1) == 1
}

@test
fn not_found() {
  assert find(0) == 0
}

@test
fn escapes() {
  assert (find(0) escape { NotFound(id:) -> id - 1 }) == -1
}
",
        ),
    ];
    let (analysis, report, out) = run_tests(
        "unhandled",
        sources,
        &mut LowerTests::default(),
        &mut CoreJs,
        None,
    );
    assert_eq!(analysis.diagnostics, []);
    let report = report.unwrap();
    let results: Vec<_> = report
        .results
        .iter()
        .map(|r| (r.name.as_str(), r.outcome.clone()))
        .collect();
    assert_eq!(
        results,
        [
            ("finds", TestOutcome::Passed),
            ("not_found", TestOutcome::Error("NotFound(id: 0)".into())),
            ("escapes", TestOutcome::Passed),
        ]
    );
    insta::assert_snapshot!(out);
}

#[test]
fn tests_provide_effects_with_handlers() {
    // A test function's R is empty, so handlers are provided with `with` (spec 13.3). A failure in
    // the body of a swapped-in handler's operation propagates to the operation's caller.
    let sources = &[
        ("app/Pome.toml", ""),
        (
            "app/src/main.emel",
            "\
import Std.Console.{Console, SystemConsole}\n\nerror Down(reason: String)

effect Store {
  fn get(key: String) -> Int fails Down
}

handler FixedStore: Store {
  fn get(key) { String.length(key) }
}

handler DownStore: Store {
  fn get(key) { fail Down(reason: key) }
}

fn total(keys: List[String]) -> Int fails Down {
  store = use Store
  List.fold(keys, 0, fn(acc, k) { acc + store.get(k) })
}

@test
fn fixed() {
  with FixedStore { assert total([\"ab\", \"c\"]) == 3 }
}

@test
fn down() {
  r = with DownStore { total([\"x\"]) } escape { Down(reason:) -> String.length(reason) * 10 }
  assert r == 10
}

@test
fn printed() {
  with SystemConsole { (use Console).print_line(\"from test\") }
}
",
        ),
    ];
    let (analysis, report, _) = run_tests(
        "effects",
        sources,
        &mut LowerTests::default(),
        &mut CoreJs,
        None,
    );
    assert_eq!(analysis.diagnostics, []);
    let report = report.unwrap();
    let results: Vec<_> = report
        .results
        .iter()
        .map(|r| (r.name.as_str(), matches!(r.outcome, TestOutcome::Passed)))
        .collect();
    assert_eq!(
        results,
        [("fixed", true), ("down", true), ("printed", true)]
    );
}

#[test]
fn suspended_tests_are_awaited_or_interrupted() {
    // A suspending test function waits to be resumed. A test that ends without a resume, or that
    // hits a defect inside a host callback, is reported as failed and the rest continue
    // (spec 14.5).
    let mut m = Module::new();
    let resolve = suspend::resolve(&mut m);
    let set_timeout = suspend::set_timeout(&mut m);
    let (never, wait) = suspend::never(&mut m);
    let callback = m.fn_type(vec![], TypeId::UNIT);
    let mut tests = Vec::new();
    let mut case = |m: &mut Module, name: &str, suspends: bool, body: Expr| {
        let f = m.declare(name, false);
        m.functions[f].suspends = suspends;
        m.define(f, vec![], TypeId::UNIT, body);
        tests.push(TestCase {
            name: name.to_owned(),
            function: f,
        });
    };
    let awaited = Assert::compare(
        BinOp::Eq,
        OpTy::Int,
        TypeId::INT,
        call_suspends(resolve, vec![int(1)]),
        int(1),
        "resolve(1) == 1",
    );
    case(&mut m, "awaits", true, awaited);
    let h = m.local("h", Type::Handler(wait));
    let forever = Expr::With {
        binds: vec![WithBind::new(h, call(never, vec![]))],
        body: Box::new(suspend::forever(wait, h)),
    };
    case(&mut m, "waits_forever", true, forever);
    let (h, x) = (
        m.local("h", Type::Handler(wait)),
        m.local("_", TypeId::UNIT),
    );
    let late = Expr::With {
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
    };
    case(&mut m, "panics_later", true, late);
    case(&mut m, "plain", false, unit());
    let log = suspend::log(&mut m);
    let (h, r) = (
        m.local("h", Type::Handler(wait)),
        m.local("r", Type::Handler(wait)),
    );
    let pool = suspend::released(&mut m, log, r, "Pool", "released");
    let releasing = Expr::With {
        binds: vec![pool, WithBind::new(h, call(never, vec![]))],
        body: Box::new(suspend::forever(wait, h)),
    };
    case(&mut m, "releases_when_interrupted", true, releasing);

    let (report, out) = run_ir("suspend", TestModule { module: m, tests }, None);
    let outcomes: Vec<_> = report
        .results
        .iter()
        .map(|r| (r.name.as_str(), r.outcome.clone()))
        .collect();
    assert_eq!(
        outcomes,
        [
            ("awaits", TestOutcome::Passed),
            ("waits_forever", TestOutcome::Interrupted),
            ("panics_later", TestOutcome::Defect("late".into())),
            ("plain", TestOutcome::Passed),
            ("releases_when_interrupted", TestOutcome::Interrupted),
        ]
    );
    assert!(
        out.contains("released\ntest releases_when_interrupted ... FAILED\n"),
        "{out}"
    );
    assert!(
        out.contains("---- waits_forever ----\ninterrupted\n"),
        "{out}"
    );
}

#[test]
fn tests_can_use_js_clock() {
    // The bundled Clock's `.mjs` is written into the test output too (under `emela_core/`). A test
    // that sleeps waits to be resumed.
    let sources = &[
        ("app/Pome.toml", ""),
        (
            "app/src/main.emel",
            "import Std.Clock.{Clock, SystemClock}\nimport Std.Instant.{Instant}\nimport Std.Duration.{Duration}\n\nfn main() {}\n\n@test\nfn clock_moves_forward() {\n  with SystemClock {\n    start = (use Clock).now()\n    (use Clock).sleep(20)\n    d = (use Clock).now() |> Instant.since(start)\n    assert Duration.to_milliseconds(d) >= 15\n  }\n}\n",
        ),
    ];
    let (analysis, report, _) = run_tests(
        "js-clock",
        sources,
        &mut LowerTests::default(),
        &mut CoreJs,
        None,
    );
    assert_eq!(analysis.diagnostics, []);
    let report = report.unwrap();
    assert!(
        matches!(report.results[0].outcome, TestOutcome::Passed),
        "{report:?}"
    );
}

#[test]
fn tests_run_stateful_handlers() {
    // A stateful handler inside a test function also runs its release however it ends: value,
    // error (init failure), or interruption (spec 14.2, 14.5). Its output comes before the test's
    // result line.
    let sources = &[
        ("app/Pome.toml", ""),
        (
            "app/src/main.emel",
            r##"import Std.Console.{Console, SystemConsole}

error Down

effect Users {
  fn find(id: Int) -> String
}

effect Wait {
  suspend fn forever() -> ()
}

handler Pool(name: String): Users {
  init {
    (use Console).print_line("open")
    Pool(name: "pool")
  }
  release { (use Console).print_line("close #{self.name}") }
  fn find(id) { "#{self.name} #{id}" }
}

handler Broken: Wait {
  init { fail Down }
  suspend fn forever(resume) { resume(()) }
}

handler Stuck: Wait {
  suspend fn forever(resume) { () }
}

fn main() {}

@test
fn finds() {
  with SystemConsole, Pool { assert (use Users).find(1) == "pool 1" }
}

@test
fn init_fails() {
  with SystemConsole, Pool, Broken { (use Users).find(1) }
}

@test
fn stuck() {
  with SystemConsole, Pool, Stuck { (use Wait).forever() }
}
"##,
        ),
    ];
    let (analysis, report, out) = run_tests(
        "stateful",
        sources,
        &mut LowerTests::default(),
        &mut CoreJs,
        None,
    );
    assert_eq!(analysis.diagnostics, []);
    let outcomes: Vec<_> = report
        .unwrap()
        .results
        .iter()
        .map(|r| (r.name.clone(), r.outcome.clone()))
        .collect();
    assert_eq!(
        outcomes,
        [
            ("finds".to_owned(), TestOutcome::Passed),
            ("init_fails".to_owned(), TestOutcome::Error("Down".into())),
            ("stuck".to_owned(), TestOutcome::Interrupted),
        ]
    );
    assert!(
        out.contains("open\nclose pool\ntest finds ... ok\n"),
        "{out}"
    );
    assert!(
        out.contains("open\nclose pool\ntest init_fails ... FAILED\n"),
        "{out}"
    );
    assert!(
        out.contains("open\nclose pool\ntest stuck ... FAILED\n"),
        "{out}"
    );
}

#[test]
fn state_and_test_handlers_in_tests() {
    // Tests use State.run and the bundled TestConsole and TestClock (spec 8.8, chapter 13, 15.2).
    // TestClock runs on virtual time and depends on Async, so the test function may suspend.
    let sources = &[
        ("app/Pome.toml", ""),
        (
            "app/src/main.emel",
            r##"import Std.Console.{Console, TestConsole}
import Std.Clock.{Clock, TestClock}
import Std.State.{State}
import Std.Instant.{Instant}
import Std.Duration.{Duration}

error Boom

fn greet(id: Int) -> () use { Console } {
  (use Console).print_line("Hello, #{id}!")
}

fn wait_twice() -> Duration use { Clock } {
  clock = use Clock
  start = clock.now()
  clock.sleep(250)
  clock.sleep(750)
  clock.now() |> Instant.since(start)
}

@test
fn greets() {
  (_, lines) = State.run([], fn() { with TestConsole { greet(1) } })
  assert lines == ["Hello, 1!"]
}

@test
fn clock_moves() {
  start = Instant.from_unix_milliseconds(Int64.from_int(0))
  (d, end) = State.run(start, fn() { with TestClock { wait_twice() } })
  assert Duration.to_milliseconds(d) == Int64.from_int(1000)
  assert Instant.to_unix_milliseconds(end) == Int64.from_int(1000)
}

@test
fn two_states() {
  ((_, s), n) = State.run(1, fn() {
    State.run("a", fn() {
      (use State[Int]).update(fn(n) { n + 41 })
      (use State[String]).update(fn(s) { String.append(s, "b") })
    })
  })
  assert n == 42
  assert s == "ab"
}

@test
fn failure_drops_the_state() {
  State.run(0, fn() {
    (use State[Int]).set(1)
    fail Boom
  })
}
"##,
        ),
    ];
    let (analysis, report, _) = run_tests(
        "state",
        sources,
        &mut LowerTests::default(),
        &mut CoreJs,
        None,
    );
    assert_eq!(analysis.diagnostics, []);
    let outcomes: Vec<_> = report
        .unwrap()
        .results
        .iter()
        .map(|r| (r.name.clone(), r.outcome.clone()))
        .collect();
    assert_eq!(
        outcomes,
        [
            ("greets".to_owned(), TestOutcome::Passed),
            ("clock_moves".to_owned(), TestOutcome::Passed),
            ("two_states".to_owned(), TestOutcome::Passed),
            (
                "failure_drops_the_state".to_owned(),
                TestOutcome::Error("Boom".into())
            ),
        ]
    );
}
