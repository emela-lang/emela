//! The WASM target (spec chapter 16) through the `emela build` and `run` paths: the component is
//! run with `wasmtime` (`EMELA_WASMTIME` or `wasmtime`). Tests that need wasmtime are skipped,
//! with a message, when it cannot start.

use std::path::{Path, PathBuf};
use std::process::Command;

use emela_driver::{
    Analysis, CoreWasm, Input, Lower, LowerTests, MemoryFiles, Output, RunOptions, RunOutput,
    Target, TestOptions, TestOutcome, WASM_ENTRY_FILE, build, run, test, wasmtime_program,
};

/// Whether wasmtime starts; prints why a test is skipped otherwise.
fn have_wasmtime() -> bool {
    let wasmtime = wasmtime_program();
    let ok = Command::new(&wasmtime)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!("skipping: cannot start `{}`", wasmtime.display());
    }
    ok
}

fn out_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("emela-driver-wasm-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn project(main: &str) -> (MemoryFiles, Input) {
    let fs = MemoryFiles::new([("app/Pome.toml", ""), ("app/src/main.emel", main)]);
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    (fs, input)
}

/// Builds and runs `main` on the WASM target.
fn run_wasm(name: &str, main: &str) -> (Analysis, Option<RunOutput>) {
    run_wasm_files(name, &[("main.emel", main)])
}

/// Builds and runs a project of `src` files (`main.emel` first) on the WASM target.
fn run_wasm_files(name: &str, files: &[(&str, &str)]) -> (Analysis, Option<RunOutput>) {
    let mut sources: Vec<(String, String)> = vec![("app/Pome.toml".to_owned(), String::new())];
    sources.extend(
        files
            .iter()
            .map(|(path, text)| (format!("app/src/{path}"), (*text).to_owned())),
    );
    let fs = MemoryFiles::new(sources);
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let options = RunOptions {
        out_dir: out_dir(name),
        node: wasmtime_program(),
        args: Vec::new(),
        output: Output::Capture,
    };
    let result = run(
        &fs,
        &input,
        &mut Lower::default().with_target(Target::Wasm),
        &mut CoreWasm,
        &options,
        |_| {},
    );
    let _ = std::fs::remove_dir_all(&options.out_dir);
    result
}

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap()
}

#[test]
fn builds_the_module() {
    let (fs, input) = project("fn main() { dbg(1) }\n");
    let (analysis, entry) = build(
        &fs,
        &input,
        &mut Lower::default().with_target(Target::Wasm),
        &mut CoreWasm,
        &out_dir("build"),
    );
    assert!(!analysis.has_errors(), "{:?}", analysis.diagnostics);
    let entry = entry.unwrap();
    assert!(entry.ends_with(WASM_ENTRY_FILE));
    assert!(std::fs::metadata(&entry).unwrap().len() > 8);
    // The WASM magic.
    let bytes = std::fs::read(&entry).unwrap();
    assert_eq!(&bytes[..4], b"\0asm");
    let _ = std::fs::remove_dir_all(entry.parent().unwrap());
}

#[test]
fn runs_under_wasmtime() {
    if !have_wasmtime() {
        return;
    }
    let (analysis, output) = run_wasm("run", "fn main() {\n  dbg(2 + 3)\n  dbg(\"hi\")\n}\n");
    assert!(!analysis.has_errors(), "{:?}", analysis.diagnostics);
    let output = output.unwrap();
    assert_eq!(output.code, 0);
    assert_eq!(text(&output.stdout), "");
    assert_eq!(text(&output.stderr), "5\n\"hi\"\n");
}

#[test]
fn examples_run_on_wasm() {
    if !have_wasmtime() {
        return;
    }
    // The examples that use only the supported hosts (hello, errors and resources use
    // SystemConsole; tour is pure apart from `dbg`).
    let (analysis, output) = run_wasm_files(
        "example-hello",
        &[(
            "main.emel",
            include_str!("../../../examples/hello/src/main.emel"),
        )],
    );
    assert!(!analysis.has_errors(), "{:?}", analysis.diagnostics);
    let output = output.unwrap();
    assert_eq!(output.code, 0);
    assert_eq!(text(&output.stdout), "hi\nHello, Emela!\nHello, world!\n");
    assert_eq!(text(&output.stderr), "done\n");

    let (analysis, output) = run_wasm_files(
        "example-errors",
        &[(
            "main.emel",
            include_str!("../../../examples/errors/src/main.emel"),
        )],
    );
    assert!(!analysis.has_errors(), "{:?}", analysis.diagnostics);
    let output = output.unwrap();
    assert_eq!(output.code, 1);
    assert_eq!(text(&output.stdout), "Hello, alice!\nHello, guest!\n");
    assert_eq!(
        text(&output.stderr),
        "error: Invalid(reason: \"negative id\")\n"
    );

    let (analysis, output) = run_wasm_files(
        "example-resources",
        &[(
            "main.emel",
            include_str!("../../../examples/resources/src/main.emel"),
        )],
    );
    assert!(!analysis.has_errors(), "{:?}", analysis.diagnostics);
    let output = output.unwrap();
    assert_eq!(output.code, 0);
    assert_eq!(
        text(&output.stdout),
        "open store v1\nv1/alice\nv1/bob\nclose store v1\n"
    );
    assert_eq!(text(&output.stderr), "");

    let (analysis, output) = run_wasm_files(
        "example-tour",
        &[
            (
                "main.emel",
                include_str!("../../../examples/tour/src/main.emel"),
            ),
            (
                "email.emel",
                include_str!("../../../examples/tour/src/email.emel"),
            ),
        ],
    );
    assert!(!analysis.has_errors(), "{:?}", analysis.diagnostics);
    let output = output.unwrap();
    assert_eq!(output.code, 0);
    assert_eq!(text(&output.stdout), "");
    assert_eq!(
        text(&output.stderr),
        "Todo(id: 1, title: \"Write docs\", done: True)\n6.0\n50005000\n\
         \"1 2 Fizz 4 Buzz Fizz 7 8 Fizz Buzz 11 Fizz 13 14 FizzBuzz\"\n\
         Some(\"alice@example.com\")\nNone\n"
    );
}

/// Runs `main`'s tests on the WASM target and returns the outcomes, as (name, outcome).
fn test_wasm(name: &str, main: &str) -> (Analysis, Vec<(String, TestOutcome)>) {
    let (fs, input) = project(main);
    let options = TestOptions {
        out_dir: out_dir(name),
        node: wasmtime_program(),
        filter: None,
        output: Output::Capture,
    };
    let mut out = Vec::new();
    let (analysis, report) = test(
        &fs,
        &input,
        &mut LowerTests::default().with_target(Target::Wasm),
        &mut CoreWasm,
        &options,
        |_| {},
        &mut out,
    );
    let _ = std::fs::remove_dir_all(&options.out_dir);
    let results = report
        .map(|r| r.results.into_iter().map(|r| (r.name, r.outcome)).collect())
        .unwrap_or_default();
    (analysis, results)
}

#[test]
fn errors_and_defects_reach_stderr() {
    if !have_wasmtime() {
        return;
    }
    // An error that reaches main (spec 14.3).
    let (analysis, output) = run_wasm(
        "main-error",
        "error Nope(why: String) derive Show\n\
         fn half(n: Int) -> Int fails Nope {\n\
           if n % 2 != 0 { fail Nope(why: \"odd\") }\n\
           n / 2\n\
         }\n\
         fn main() { dbg(half(3)) }\n",
    );
    assert!(!analysis.has_errors(), "{:?}", analysis.diagnostics);
    let output = output.unwrap();
    assert_eq!(output.code, 1);
    assert_eq!(text(&output.stderr), "error: Nope(why: \"odd\")\n");
    // A defect (spec 14.4).
    let (analysis, output) = run_wasm("main-defect", "fn main() { assert 1 + 1 == 3 }\n");
    assert!(!analysis.has_errors(), "{:?}", analysis.diagnostics);
    let output = output.unwrap();
    assert_eq!(output.code, 101);
    assert_eq!(
        text(&output.stderr),
        "defect: assertion failed: 1 + 1 == 3\n  left: 2\n right: 3\n"
    );
}

#[test]
fn a_release_defect_keeps_the_first_defect() {
    if !have_wasmtime() {
        return;
    }
    // The body defects and the `release` fails too: the first defect stays the result, and the
    // release's defect becomes a note (spec 14.4).
    let (analysis, output) = run_wasm(
        "release-defect",
        "effect Pool {\n  fn get() -> Int\n}\n\
         handler Outer(n: Int): Pool {\n\
           init { Outer(n: 0) }\n\
           release { panic(\"outer release failed\") }\n\
           fn get() { self.n }\n\
         }\n\
         fn work() -> Int {\n\
           zero = (use Pool).get()\n\
           1 / zero\n\
         }\n\
         fn main() {\n\
           with Outer { dbg(work()) }\n\
         }\n",
    );
    assert!(!analysis.has_errors(), "{:?}", analysis.diagnostics);
    let output = output.unwrap();
    assert_eq!(output.code, 101);
    assert_eq!(
        text(&output.stderr),
        "defect: division by zero\n\
         note: the release of handler Outer also failed: outer release failed\n"
    );
    // A `release` that fails with no defect in the body is the result (spec 14.4).
    let (analysis, output) = run_wasm(
        "release-only-defect",
        "effect Pool {\n  fn get() -> Int\n}\n\
         handler Outer(n: Int): Pool {\n\
           init { Outer(n: 0) }\n\
           release { panic(\"outer release failed\") }\n\
           fn get() { self.n }\n\
         }\n\
         fn main() {\n\
           with Outer { dbg((use Pool).get()) }\n\
         }\n",
    );
    assert!(!analysis.has_errors(), "{:?}", analysis.diagnostics);
    let output = output.unwrap();
    assert_eq!(output.code, 101);
    assert!(
        text(&output.stderr).ends_with("defect: outer release failed\n"),
        "{}",
        text(&output.stderr)
    );
}

#[test]
fn runs_the_test_protocol() {
    if !have_wasmtime() {
        return;
    }
    let (analysis, results) = test_wasm(
        "tests",
        "error Nope(why: String) derive Show\n\
         fn half(n: Int) -> Int fails Nope {\n\
           if n % 2 != 0 { fail Nope(why: \"odd\") }\n\
           n / 2\n\
         }\n\
         @test\n\
         fn passes() { assert half(4) == 2 }\n\
         @test\n\
         fn fails_with_an_error() { assert half(3) == 1 }\n\
         @test\n\
         fn raises_a_defect() { assert 1 + 1 == 3 }\n",
    );
    assert!(!analysis.has_errors(), "{:?}", analysis.diagnostics);
    assert_eq!(
        results,
        [
            ("passes".to_owned(), TestOutcome::Passed),
            (
                "fails_with_an_error".to_owned(),
                TestOutcome::Error("Nope(why: \"odd\")".to_owned())
            ),
            (
                "raises_a_defect".to_owned(),
                TestOutcome::Defect(
                    "assertion failed: 1 + 1 == 3\n  left: 2\n right: 3".to_owned()
                )
            ),
        ]
    );
}

#[test]
fn the_test_protocol_escapes_messages() {
    if !have_wasmtime() {
        return;
    }
    // The names, the errors and the defects cross the runner boundary as JSON, so `"`, `\`,
    // control characters and non-ASCII must survive the round trip (spec 16.4).
    let (analysis, results) = test_wasm(
        "escaping",
        "error Nope(why: String) derive Show\n\
         fn boom() -> () fails Nope { fail Nope(why: \"a\\\"b\\\\c\\nd\u{20AC}\") }\n\
         @test\n\
         fn fails_with_a_quoted_message() { boom() }\n\
         @test\n\
         fn also_passes() { assert 1 + 1 == 2 }\n",
    );
    assert!(!analysis.has_errors(), "{:?}", analysis.diagnostics);
    assert_eq!(
        results,
        [
            (
                "fails_with_a_quoted_message".to_owned(),
                TestOutcome::Error("Nope(why: \"a\\\"b\\\\c\\nd\u{20AC}\")".to_owned())
            ),
            ("also_passes".to_owned(), TestOutcome::Passed),
        ]
    );
}

#[test]
fn unsupported_features_are_diagnostics() {
    let (analysis, output) = run_wasm(
        "unsupported",
        "import Std.Fiber.{Fiber}\nfn main() -> () use { Async } {\n  Fiber.scope(fn() { () })\n}\n",
    );
    assert!(analysis.has_errors());
    assert!(output.is_none());
    let messages: Vec<&str> = analysis
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect();
    assert!(
        messages
            .iter()
            .any(|m| m.contains("fibers are not supported on target `wasm`")),
        "{messages:?}"
    );
}
