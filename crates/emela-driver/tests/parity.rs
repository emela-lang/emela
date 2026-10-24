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
    // SystemHttp has no `@external(c, ...)` yet (spec 15.2).
    ("http", Some("E0412")),
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

/// Calls `f` and removes what it made in the current directory, so that an example that writes
/// files (`notes`) starts from the same state on JS and on C.
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
        Target::Wasm => unreachable!("parity compares js and c"),
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
        reporter: emela_driver::Reporter::Plain,
        color: false,
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
        Target::Wasm => unreachable!("parity compares js and c"),
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
        let on_c = in_clean_cwd(|| run_on(&dir, Target::C, &mut c, &name));
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
        let js = in_clean_cwd(|| run_on(&dir, Target::Js, &mut c, &name))
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
    let Some(c) = c_backend() else { return };
    let dir = root().join("crates/emela-codegen-c/tests/programs");
    let mut programs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "emel"))
        .collect();
    programs.sort();
    assert!(!programs.is_empty());
    // The programs do not touch the current directory, so they run in parallel: this test is the
    // slowest of the suite otherwise.
    let failures = in_parallel(&programs, &c, |path, c| {
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        // `program-` keeps the output directories apart from those of an example of the same
        // name (`errors`), which another test builds at the same time.
        let dir_name = format!("program-{name}");
        let js = run_on(path, Target::Js, c, &dir_name)
            .unwrap_or_else(|a| panic!("{name} does not run on JS: {:?}", messages(&a)));
        match run_on(path, Target::C, c, &dir_name) {
            Ok(on_c) if on_c == js => None,
            Ok(on_c) => Some(format!("{name}: JS gives {js:?}\nC gives {on_c:?}")),
            Err(analysis) => Some(format!(
                "{name} does not build on C: {:?}",
                messages(&analysis)
            )),
        }
    });
    report(&failures);
}

/// Calls `check` on every item from as many threads as there are CPUs, each with its own copy of
/// the C backend, and returns the failures in the order of `items`.
fn in_parallel<T: Sync>(
    items: &[T],
    c: &CoreC,
    check: impl Fn(&T, &mut CoreC) -> Option<String> + Sync,
) -> Vec<String> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut results: Vec<(usize, String)> = std::thread::scope(|s| {
        let workers: Vec<_> = (0..threads.min(items.len()))
            .map(|_| {
                s.spawn(|| {
                    let mut c = c.clone();
                    let mut failures = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(item) = items.get(i) else { break };
                        failures.extend(check(item, &mut c).map(|f| (i, f)));
                    }
                    failures
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
            .collect()
    });
    results.sort_by_key(|(i, _)| *i);
    results.into_iter().map(|(_, f)| f).collect()
}

/// Test functions with fibers and suspension (spec chapters 11, 13, 14.5): the outcomes, an
/// interrupted test included, are the same on both targets.
#[test]
fn suspending_tests_give_the_same_results_on_js_and_c() {
    let Some(mut c) = c_backend() else { return };
    let main = r##"import Std.Fiber.{Fiber, Scope}
import Std.Deferred.{Deferred}

error Boom

effect Tick {
  suspend fn tick() -> ()
}

handler Ticks: Tick {
  suspend fn tick(resume) { resume(()) }
}

effect Wait {
  suspend fn wait() -> Int
}

handler Ignores: Wait {
  suspend fn wait(resume) { () }
}

fn spin(n: Int) -> () use { Tick } {
  if n == 0 { () } else {
    (use Tick).tick()
    spin(n - 1)
  }
}

fn fetch(n: Int, ticks: Int) -> Int use { Tick } {
  spin(ticks * 100)
  n * 10
}

fn boom() -> Int fails Boom { fail Boom }

fn explode() -> Int { panic("child") }

@test
fn joins() {
  with Ticks {
    xs = Fiber.all([fn() { fetch(1, 20) }, fn() { fetch(2, 10) }])
    assert xs == [10, 20]
  }
}

@test
fn child_error() {
  Fiber.scope(fn() {
    h = Fiber.fork(fn() { boom() })
    Fiber.join(h)
  })
}

@test
fn child_defect() {
  Fiber.scope(fn() {
    h = Fiber.fork(fn() { explode() })
    Fiber.join(h)
  })
}

@test
fn waits_forever() {
  Fiber.scope(fn() {
    d = Fiber.deferred()
    Deferred.await(d) + 1
  })
}

@test
fn dropped_in_place() {
  with Ignores { assert (use Wait).wait() == 1 }
}

@test
fn after_the_others() {
  with Ticks { assert fetch(3, 1) == 30 }
}
"##;
    let fs = emela_driver::MemoryFiles::new([("app/Pome.toml", ""), ("app/src/main.emel", main)]);
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let mut outcomes = Vec::new();
    for target in [Target::Js, Target::C] {
        let options = TestOptions {
            out_dir: out_dir(&format!("suspending-tests-{target}")),
            node: emela_driver::node_program(),
            filter: None,
            output: Output::Capture,
            reporter: emela_driver::Reporter::Plain,
            color: false,
        };
        let mut frontend = LowerTests::default().with_target(target);
        let mut out = Vec::new();
        let (analysis, report) = match target {
            Target::Js => test(
                &fs,
                &input,
                &mut frontend,
                &mut CoreJs,
                &options,
                |_| {},
                &mut out,
            ),
            Target::C => test(
                &fs,
                &input,
                &mut frontend,
                &mut c,
                &options,
                |_| {},
                &mut out,
            ),
            Target::Wasm => unreachable!("parity compares js and c"),
        };
        let _ = std::fs::remove_dir_all(&options.out_dir);
        assert!(!analysis.has_errors(), "{:?}", messages(&analysis));
        let results: Vec<(String, TestOutcome)> = report
            .unwrap()
            .results
            .into_iter()
            .map(|r| (r.name, r.outcome))
            .collect();
        outcomes.push(results);
    }
    assert_eq!(outcomes[0], outcomes[1]);
    assert_eq!(
        outcomes[1]
            .iter()
            .map(|(_, o)| o.clone())
            .collect::<Vec<_>>(),
        [
            TestOutcome::Passed,
            TestOutcome::Error("Boom".into()),
            TestOutcome::Defect("child".into()),
            TestOutcome::Interrupted,
            TestOutcome::Interrupted,
            TestOutcome::Passed,
        ]
    );
}

/// Fails with every difference found (written to stderr too, in case the panic message is lost).
fn report(failures: &[String]) {
    if !failures.is_empty() {
        let text = failures.join("\n\n");
        eprintln!("{text}");
        panic!("\n{text}");
    }
}
