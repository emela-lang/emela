//! The C target (spec chapter 16) through the `emela build`, `run` and `test` paths: the output
//! is compiled with cc (`EMELA_CC` or `cc`) and run. Tests that need cc are skipped, with a
//! message, when it cannot start.

use std::path::{Path, PathBuf};
use std::process::Command;

use emela_driver::{
    Analysis, C_EXECUTABLE, CoreC, Input, Lower, LowerTests, MemoryFiles, Output, RunOptions,
    RunOutput, Target, TestOptions, TestOutcome, build, cc_command, project_target, run, test,
};

/// Whether the C compiler starts; prints why a test is skipped otherwise.
fn have_cc() -> bool {
    let (cc, flags) = cc_command();
    let ok = Command::new(&cc)
        .args(&flags)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!("skipping: cannot start the C compiler `{}`", cc.display());
    }
    ok
}

/// A per-test output directory. Leftovers from earlier runs are removed.
fn out_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("emela-driver-c-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn project(main: &str) -> (MemoryFiles, Input) {
    let fs = MemoryFiles::new([("app/Pome.toml", ""), ("app/src/main.emel", main)]);
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    (fs, input)
}

/// Builds and runs `main` on the C target.
fn run_c(name: &str, main: &str, backend: &mut CoreC) -> (Analysis, Option<RunOutput>) {
    let (fs, input) = project(main);
    let options = RunOptions {
        out_dir: out_dir(name),
        node: PathBuf::from("node"),
        args: Vec::new(),
        output: Output::Capture,
    };
    let result = run(
        &fs,
        &input,
        &mut Lower::default().with_target(Target::C),
        backend,
        &options,
        |_| {},
    );
    let _ = std::fs::remove_dir_all(&options.out_dir);
    result
}

fn codes(analysis: &Analysis) -> Vec<(Option<&str>, &str)> {
    analysis
        .diagnostics
        .iter()
        .map(|d| (d.code, d.message.as_str()))
        .collect()
}

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap()
}

#[test]
fn platform_handlers_run_natively() {
    if !have_cc() {
        return;
    }
    let main = r##"import Std.Console.{Console, SystemConsole}
import Std.Env.{Env, SystemEnv}
import Std.Random.{Random, SystemRandom}

error Bad(n: Int)

fn check(n: Int) -> Int fails Bad {
  if n > 1 { fail Bad(n:) }
  n
}

fn main() {
  with SystemConsole, SystemEnv, SystemRandom {
    console = use Console
    console.print_line("hi from c")
    x = (use Random).float()
    console.print_line("#{x >= 0.0 && x < 1.0}")
    console.print_line("#{(use Env).get("EMELA_SURELY_UNSET_VARIABLE")}")
    console.print_error_line("to stderr")
    console.print_line("#{check(1)}")
    console.print_line("#{check(2)}")
  }
}
"##;
    let (analysis, output) = run_c("platform", main, &mut CoreC::default());
    assert_eq!(codes(&analysis), []);
    let output = output.unwrap();
    assert_eq!(text(&output.stdout), "hi from c\nTrue\nNone\n1\n");
    assert_eq!(text(&output.stderr), "to stderr\nerror: Bad(n: 2)\n");
    assert_eq!(output.code, 1);
}

#[test]
fn build_writes_the_executable() {
    if !have_cc() {
        return;
    }
    let (fs, input) = project("fn main() { dbg(1 + 1) }\n");
    let dir = out_dir("build");
    let (analysis, entry) = build(
        &fs,
        &input,
        &mut Lower::default().with_target(Target::C),
        &mut CoreC::default(),
        &dir,
    );
    assert_eq!(codes(&analysis), []);
    let entry = entry.unwrap();
    assert_eq!(entry, dir.join(C_EXECUTABLE));
    for file in ["main.c", "emela_runtime.c", "emela_runtime.h"] {
        assert!(dir.join(file).exists(), "{file}");
    }
    let out = Command::new(&entry).output().unwrap();
    assert_eq!(text(&out.stderr), "2\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_c_external_is_e0412() {
    let main = r##"@external(js, "Date.now")
fn now() -> Float

handler Clock implements Tick {
  fn tick() { now() }
}

effect Tick {
  fn tick() -> Float
}

fn main() {
  with Clock { (use Tick).tick() }
}
"##;
    let (analysis, output) = run_c("missing", main, &mut CoreC::default());
    assert!(output.is_none());
    assert_eq!(
        codes(&analysis),
        [(
            Some("E0412"),
            "external function `now` has no `@external` for target `c`"
        )]
    );
}

#[test]
fn callbacks_and_suspend_externals_are_rejected() {
    let main = r##"@external(c, "take")
fn take(f: fn(Int) -> Int) -> () use {}

fn main() { take(fn(x) { x }) }
"##;
    let (analysis, _) = run_c("callback", main, &mut CoreC::default());
    assert_eq!(
        codes(&analysis),
        [(
            Some("E0427"),
            "external function `take` takes a callback, which target `c` does not support yet"
        )]
    );
}

#[test]
fn cc_problems_are_e0907_and_e0908() {
    let main = "fn main() { dbg(1) }\n";
    let (analysis, output) = run_c(
        "no-cc",
        main,
        &mut CoreC::with_cc("/nonexistent/emela-test-cc", &[]),
    );
    assert!(output.is_none());
    let found = codes(&analysis);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, Some("E0907"));
    assert!(
        found[0]
            .1
            .starts_with("cannot start `/nonexistent/emela-test-cc`")
    );

    // A compiler that fails.
    let (analysis, output) = run_c("bad-cc", main, &mut CoreC::with_cc("false", &[]));
    assert!(output.is_none());
    let found = codes(&analysis);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, Some("E0908"));
    assert!(found[0].1.starts_with("`false` failed to compile `"));
}

#[test]
fn tests_run_natively() {
    if !have_cc() {
        return;
    }
    let main = r##"error Nope(why: String)

fn half(n: Int) -> Int fails Nope {
  if n % 2 != 0 { fail Nope(why: "odd") }
  n / 2
}

@test
fn passes() {
  assert half(4) == 2
}

@test
fn fails_with_an_error() {
  assert half(3) == 1
}

@test
fn raises_a_defect() {
  assert 1 + 1 == 3
}
"##;
    let (fs, input) = project(main);
    let options = TestOptions {
        out_dir: out_dir("tests"),
        node: PathBuf::from("node"),
        filter: None,
        output: Output::Capture,
    };
    let mut out = Vec::new();
    let (analysis, report) = test(
        &fs,
        &input,
        &mut LowerTests::default().with_target(Target::C),
        &mut CoreC::default(),
        &options,
        |_| {},
        &mut out,
    );
    let _ = std::fs::remove_dir_all(&options.out_dir);
    assert_eq!(codes(&analysis), []);
    let results: Vec<(String, TestOutcome)> = report
        .unwrap()
        .results
        .into_iter()
        .map(|r| (r.name, r.outcome))
        .collect();
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
fn build_target_from_pome_toml() {
    let read = |toml: &str| {
        let fs = MemoryFiles::new([("app/Pome.toml", toml), ("app/src/main.emel", "")]);
        let input = Input::resolve(&fs, Path::new("app")).unwrap();
        project_target(&fs, &input).map_err(|d| (d.code, d.message))
    };
    assert_eq!(read(""), Ok(None));
    assert_eq!(read("[build]\ntarget = \"c\"\n"), Ok(Some(Target::C)));
    assert_eq!(
        read("[build]\ntarget = \"node\"\n"),
        Err((
            Some("E0232"),
            "unknown target `node` in `[build].target`".to_owned()
        ))
    );
}
