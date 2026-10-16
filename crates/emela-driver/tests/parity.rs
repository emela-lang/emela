//! The same programs on the JS and the C target (spec chapter 16: every target gives the same
//! observable result). Each example project (`examples/*`) and each program of emela-codegen-c's
//! tests (`crates/emela-codegen-c/tests/programs/*.emel`) runs on both, and stdout, stderr and the
//! exit code must agree; the tests of each example must give the same results too. The C runtime
//! is compiled with its leak check (`-DEMELA_LEAK_CHECK`), so a leaked allocation shows up as a
//! difference (exit code 99). Examples the C target cannot build yet are listed in
//! [`NOT_ON_C`] with the diagnostic that stops them.
//!
//! Needs node and a C compiler (`EMELA_CC` or `cc`); without cc the tests are skipped with a
//! message.

use std::path::{Path, PathBuf};
use std::process::Command;

use emela_driver::{
    Analysis, CoreC, CoreJs, Input, Lower, LowerTests, OsFs, Output, RunOptions, RunOutput, Target,
    TestOptions, TestOutcome, cc_command, run, test,
};

/// Examples that do not build on C yet, and the code of the first diagnostic that stops them (no
/// code: the C target's "not supported yet").
const NOT_ON_C: &[(&str, Option<&str>)] = &[
    // `with SystemClock` reaches the `sleep` external function, which has no `@external(c, ...)`
    // until the C target can suspend (spec 18.1 #37).
    ("clock", Some("E0412")),
    ("fibers", Some("E0412")),
    // `with Platform` reaches every System handler, including SystemClock and SystemHttp.
    ("http", Some("E0412")),
    ("notes", Some("E0412")),
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The C backend with the runtime's leak check compiled in.
fn c_backend() -> Option<CoreC> {
    let (cc, mut flags) = cc_command();
    let ok = Command::new(&cc)
        .args(&flags)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!(
            "skipping: cannot start the C compiler `{}` (set EMELA_CC)",
            cc.display()
        );
        return None;
    }
    flags.push("-DEMELA_LEAK_CHECK".to_owned());
    let flags: Vec<&str> = flags.iter().map(String::as_str).collect();
    Some(CoreC::with_cc(cc, &flags))
}

fn out_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("emela-driver-parity-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

type Outcome = (i32, String, String);

/// Runs the program at `path` on `target`: its exit code, stdout and stderr, or the diagnostics
/// that stopped it.
fn run_on(
    path: &Path,
    target: Target,
    c: &mut CoreC,
    name: &str,
) -> Result<Outcome, Box<Analysis>> {
    let input = Input::resolve(&OsFs, path).unwrap();
    let options = RunOptions {
        out_dir: out_dir(&format!("{name}-{target}")),
        node: emela_driver::node_program(),
        args: Vec::new(),
        output: Output::Capture,
    };
    let mut frontend = Lower::default().with_target(target);
    let (analysis, output) = match target {
        Target::Js => run(&OsFs, &input, &mut frontend, &mut CoreJs, &options, |_| {}),
        Target::C => run(&OsFs, &input, &mut frontend, c, &options, |_| {}),
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
    c: &mut CoreC,
    name: &str,
) -> Result<Vec<(String, TestOutcome)>, Box<Analysis>> {
    let input = Input::resolve(&OsFs, path).unwrap();
    let options = TestOptions {
        out_dir: out_dir(&format!("{name}-test-{target}")),
        node: emela_driver::node_program(),
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
        Target::C => test(&OsFs, &input, &mut frontend, c, &options, |_| {}, &mut out),
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

#[test]
fn examples_run_the_same_on_js_and_c() {
    let Some(mut c) = c_backend() else { return };
    let mut failures = Vec::new();
    for (name, dir) in examples() {
        let on_c = run_on(&dir, Target::C, &mut c, &name);
        // The examples that do not build on C are not run on JS either: some of them write files
        // in the current directory.
        if let Some((_, code)) = NOT_ON_C.iter().find(|(n, _)| *n == name) {
            match on_c {
                Ok(_) => failures.push(format!("{name} now builds on C: take it out of NOT_ON_C")),
                Err(analysis) if first_code(&analysis) == *code => {}
                Err(analysis) => failures.push(format!(
                    "{name} does not build on C for another reason: {:?}",
                    messages(&analysis)
                )),
            }
            continue;
        }
        let js = run_on(&dir, Target::Js, &mut c, &name)
            .unwrap_or_else(|a| panic!("{name} does not run on JS: {:?}", messages(&a)));
        match on_c {
            Ok(on_c) if on_c == js => {}
            Ok(on_c) => failures.push(format!("{name}: JS gives {js:?}\nC gives {on_c:?}")),
            Err(analysis) => failures.push(format!(
                "{name} does not build on C: {:?}",
                messages(&analysis)
            )),
        }
    }
    report(&failures);
}

#[test]
fn example_tests_give_the_same_results_on_js_and_c() {
    let Some(mut c) = c_backend() else { return };
    let mut failures = Vec::new();
    for (name, dir) in examples() {
        if NOT_ON_C.iter().any(|(n, _)| *n == name) {
            continue;
        }
        let js = test_on(&dir, Target::Js, &mut c, &name)
            .unwrap_or_else(|a| panic!("the tests of {name} fail on JS: {:?}", messages(&a)));
        match test_on(&dir, Target::C, &mut c, &name) {
            Ok(on_c) if on_c == js => {}
            Ok(on_c) => failures.push(format!("{name}: JS gives {js:?}\nC gives {on_c:?}")),
            Err(analysis) => failures.push(format!(
                "the tests of {name} do not build on C: {:?}",
                messages(&analysis)
            )),
        }
    }
    report(&failures);
}

#[test]
fn codegen_programs_run_the_same_on_js_and_c() {
    let Some(mut c) = c_backend() else { return };
    let dir = root().join("crates/emela-codegen-c/tests/programs");
    let mut programs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "emel"))
        .collect();
    programs.sort();
    assert!(!programs.is_empty());
    let mut failures = Vec::new();
    for path in programs {
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let js = run_on(&path, Target::Js, &mut c, &name)
            .unwrap_or_else(|a| panic!("{name} does not run on JS: {:?}", messages(&a)));
        match run_on(&path, Target::C, &mut c, &name) {
            Ok(on_c) if on_c == js => {}
            Ok(on_c) => failures.push(format!("{name}: JS gives {js:?}\nC gives {on_c:?}")),
            Err(analysis) => failures.push(format!(
                "{name} does not build on C: {:?}",
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
