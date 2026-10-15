//! The WASM target (spec chapter 16) through the `emela build` and `run` paths: the module is run
//! with `wasmtime` (`EMELA_WASMTIME` or `wasmtime`). Tests that need wasmtime are skipped, with a
//! message, when it cannot start.

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
    let (fs, input) = project(main);
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
        "error Nope(why: String)\n\
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
fn runs_the_test_protocol() {
    if !have_wasmtime() {
        return;
    }
    let (analysis, results) = test_wasm(
        "tests",
        "error Nope(why: String)\n\
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
fn unsupported_features_are_diagnostics() {
    let (analysis, output) = run_wasm("unsupported", "fn main() { dbg(1.5) }\n");
    assert!(analysis.has_errors());
    assert!(output.is_none());
    let messages: Vec<&str> = analysis
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect();
    assert!(messages.iter().any(|m| m.contains("Float")), "{messages:?}");
}
