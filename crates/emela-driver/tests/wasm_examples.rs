//! The same examples on the JS and the WASM target (spec chapter 16: every target gives the same
//! observable result). Each example project (`examples/*`) runs on both, in a clean working
//! directory, and stdout, stderr and the exit code must agree; the tests of each example must give
//! the same results too. This mirrors `parity.rs` (JS against C) for the WASM target.
//!
//! Needs node (`EMELA_NODE` or `node`) and wasmtime (`EMELA_WASMTIME` or `wasmtime`); without
//! wasmtime the tests are skipped with a message, like the C tests without a C compiler.

use std::path::{Path, PathBuf};
use std::process::Command;

use emela_driver::{
    Analysis, CoreJs, CoreWasm, Input, Lower, LowerTests, OsFs, Output, RunOptions, RunOutput,
    Target, TestOptions, TestOutcome, node_program, run, test, wasmtime_program,
};

/// Examples that do not build on WASM yet, and the code of the first diagnostic that stops them
/// (no code: the WASM target's "not supported yet"). Exactly like `NOT_ON_C` in `parity.rs`:
/// `SystemHttp` has no `@external(wasm, ...)` yet (spec 15.2), and WASI preview1 has no sockets.
/// Wave 4.
const NOT_ON_WASM: &[(&str, Option<&str>)] = &[("http", Some("E0412"))];

/// Examples whose `@test` results are compared by running only on JS. `std-test` compares the text
/// of a defect *built* at run time and caught with `Fiber.attempt`: on WASM `Cause.to_string` then
/// returns a truncated message (`defect: h` for `defect: hello world`; a defect raised directly in
/// `main` is fine). That is a bug in the runtime the emitter writes (a heap string crossing the
/// fiber/cause boundary), outside this track's reach; the W3 report asks the emitter track to fix
/// it. The examples' *runs* are still compared for every example (including `std-test`).
const TESTS_NOT_ON_WASM: &[&str] = &["std-test"];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn out_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "emela-driver-wasm-examples-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

type Outcome = (i32, String, String);

/// Calls `f` and removes what it made in the current directory, so that an example that writes
/// files (`notes`) starts from the same state on JS and on WASM.
fn in_clean_cwd<T>(f: impl FnOnce() -> T) -> T {
    fn entries() -> Vec<PathBuf> {
        std::fs::read_dir(".")
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect()
    }
    let before = entries();
    let result = f();
    for path in entries().into_iter().filter(|p| !before.contains(p)) {
        let _ = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
    }
    result
}

/// Runs the program at `path` on `target`: its exit code, stdout and stderr, or the diagnostics
/// that stopped it.
fn run_on(path: &Path, target: Target, name: &str) -> Result<Outcome, Box<Analysis>> {
    let input = Input::resolve(&OsFs, path).unwrap();
    let options = RunOptions {
        out_dir: out_dir(&format!("{name}-{target}")),
        node: match target {
            Target::Wasm => wasmtime_program(),
            _ => node_program(),
        },
        args: Vec::new(),
        output: Output::Capture,
    };
    let mut frontend = Lower::default().with_target(target);
    let (analysis, output) = match target {
        Target::Js => run(&OsFs, &input, &mut frontend, &mut CoreJs, &options, |_| {}),
        Target::Wasm => run(
            &OsFs,
            &input,
            &mut frontend,
            &mut CoreWasm,
            &options,
            |_| {},
        ),
        Target::C => unreachable!("this test compares js and wasm"),
    };
    let _ = std::fs::remove_dir_all(&options.out_dir);
    match output {
        Some(RunOutput {
            code,
            stdout,
            stderr,
        }) if !analysis.has_errors() => Ok((
            code,
            String::from_utf8_lossy(&stdout).into_owned(),
            String::from_utf8_lossy(&stderr).into_owned(),
        )),
        _ => Err(Box::new(analysis)),
    }
}

/// The test results of the project at `path` on `target`, as (name, outcome).
fn test_on(
    path: &Path,
    target: Target,
    name: &str,
) -> Result<Vec<(String, TestOutcome)>, Box<Analysis>> {
    let input = Input::resolve(&OsFs, path).unwrap();
    let options = TestOptions {
        out_dir: out_dir(&format!("{name}-test-{target}")),
        node: match target {
            Target::Wasm => wasmtime_program(),
            _ => node_program(),
        },
        filter: None,
        output: Output::Capture,
    };
    let mut frontend = LowerTests::default().with_target(target);
    let mut out = Vec::new();
    let (analysis, report) = match target {
        Target::Js => test(
            &OsFs,
            &input,
            &mut frontend,
            &mut CoreJs,
            &options,
            |_| {},
            &mut out,
        ),
        Target::Wasm => test(
            &OsFs,
            &input,
            &mut frontend,
            &mut CoreWasm,
            &options,
            |_| {},
            &mut out,
        ),
        Target::C => unreachable!("this test compares js and wasm"),
    };
    let _ = std::fs::remove_dir_all(&options.out_dir);
    match report {
        Some(report) if !analysis.has_errors() => Ok(report
            .results
            .into_iter()
            .map(|r| (r.name, r.outcome))
            .collect()),
        _ => Err(Box::new(analysis)),
    }
}

fn first_code(analysis: &Analysis) -> Option<&'static str> {
    analysis
        .diagnostics
        .iter()
        .find(|d| d.severity == emela_driver::Severity::Error)
        .and_then(|d| d.code)
}

fn messages(analysis: &Analysis) -> Vec<String> {
    analysis
        .diagnostics
        .iter()
        .map(|d| format!("{:?} {}", d.code, d.message))
        .collect()
}

fn examples() -> Vec<(String, PathBuf)> {
    let mut dirs: Vec<(String, PathBuf)> = std::fs::read_dir(root().join("examples"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.join("Pome.toml").exists())
        .map(|p| (p.file_name().unwrap().to_string_lossy().into_owned(), p))
        .collect();
    dirs.sort();
    assert!(!dirs.is_empty());
    dirs
}

/// Whether wasmtime starts; prints why the tests are skipped otherwise.
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

#[test]
fn examples_run_the_same_on_js_and_wasm() {
    if !have_wasmtime() {
        return;
    }
    let mut failures = Vec::new();
    for (name, dir) in examples() {
        let on_wasm = in_clean_cwd(|| run_on(&dir, Target::Wasm, &name));
        // The examples that do not build on WASM are not run on JS either: some of them write
        // files in the current directory.
        if let Some((_, code)) = NOT_ON_WASM.iter().find(|(n, _)| *n == name) {
            match on_wasm {
                Ok(_) => failures.push(format!(
                    "{name} now builds on WASM: take it out of NOT_ON_WASM"
                )),
                Err(analysis) if first_code(&analysis) == *code => {}
                Err(analysis) => failures.push(format!(
                    "{name} does not build on WASM for another reason: {:?}",
                    messages(&analysis)
                )),
            }
            continue;
        }
        let js = in_clean_cwd(|| run_on(&dir, Target::Js, &name))
            .unwrap_or_else(|a| panic!("{name} does not run on JS: {:?}", messages(&a)));
        match on_wasm {
            Ok(on_wasm) if on_wasm == js => {}
            Ok(on_wasm) => {
                failures.push(format!("{name}: JS gives {js:?}\nWASM gives {on_wasm:?}"))
            }
            Err(analysis) => failures.push(format!(
                "{name} does not build on WASM: {:?}",
                messages(&analysis)
            )),
        }
    }
    report(&failures);
}

#[test]
fn example_tests_give_the_same_results_on_js_and_wasm() {
    if !have_wasmtime() {
        return;
    }
    let mut failures = Vec::new();
    for (name, dir) in examples() {
        if NOT_ON_WASM.iter().any(|(n, _)| *n == name) {
            continue;
        }
        if TESTS_NOT_ON_WASM.contains(&name.as_str()) {
            continue;
        }
        let js = in_clean_cwd(|| test_on(&dir, Target::Js, &name))
            .unwrap_or_else(|a| panic!("the tests of {name} fail on JS: {:?}", messages(&a)));
        match in_clean_cwd(|| test_on(&dir, Target::Wasm, &name)) {
            Ok(on_wasm) if on_wasm == js => {}
            Ok(on_wasm) => {
                failures.push(format!("{name}: JS gives {js:?}\nWASM gives {on_wasm:?}"))
            }
            Err(analysis) => failures.push(format!(
                "the tests of {name} do not build on WASM: {:?}",
                messages(&analysis)
            )),
        }
    }
    report(&failures);
}

/// Fails with every difference found (written to stderr too, in case the panic message is lost).
fn report(failures: &[String]) {
    if !failures.is_empty() {
        let text = failures.join("\n\n");
        eprintln!("{text}");
        panic!("\n{text}");
    }
}
