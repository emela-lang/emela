//! Programs compiled to C, built with cc and run. Each `tests/programs/NAME.emel` has the output
//! of the JS target in `NAME.expected` (the exit code, stdout and stderr, written by running the
//! program with `emela run`), and the C build must give the same. Every run checks for leaked
//! allocations, and runs again with ASan and UBSan when cc supports them.

mod common;

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::{PRELUDE, cc, run, to_c};
use emela_codegen_c::CErrorKind;

struct Expected {
    code: i32,
    stdout: String,
    stderr: String,
}

fn parse_expected(text: &str) -> Expected {
    let (head, rest) = text.split_once("\n--- stdout\n").expect("an exit line");
    let (stdout, stderr) = rest.split_once("--- stderr\n").expect("a stderr section");
    Expected {
        code: head
            .trim_start_matches("exit: ")
            .parse()
            .expect("an exit code"),
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
    }
}

#[test]
fn programs_behave_as_on_js() {
    if cc().is_none() {
        return;
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/programs");
    let mut programs: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "emel"))
        .collect();
    programs.sort();
    assert!(!programs.is_empty());
    // Each program builds twice (plainly and with the sanitizers) in its own directory, so they
    // run in parallel: this test is the slowest of the suite otherwise.
    let failures = in_parallel(&programs, |path| {
        let name = path.file_stem().unwrap().to_str().unwrap().to_owned();
        let src = std::fs::read_to_string(path).unwrap();
        let expected = parse_expected(
            &std::fs::read_to_string(path.with_extension("expected"))
                .unwrap_or_else(|_| panic!("{name}.expected is missing")),
        );
        let got = run(&name, &src).expect("cc is available");
        ((got.code, &got.stdout, &got.stderr)
            != (expected.code, &expected.stdout, &expected.stderr))
            .then(|| {
                format!(
                    "{name}: expected exit {}\n{}{}\ngot exit {}\n{}{}",
                    expected.code,
                    expected.stdout,
                    expected.stderr,
                    got.code,
                    got.stdout,
                    got.stderr
                )
            })
    });
    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
}

/// Calls `check` on every item from as many threads as there are CPUs, and returns the failures
/// in the order of `items`.
fn in_parallel<T: Sync>(items: &[T], check: impl Fn(&T) -> Option<String> + Sync) -> Vec<String> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let next = AtomicUsize::new(0);
    let mut results: Vec<(usize, String)> = std::thread::scope(|s| {
        let workers: Vec<_> = (0..threads.min(items.len()))
            .map(|_| {
                s.spawn(|| {
                    let mut failures = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(i) else { break };
                        failures.extend(check(item).map(|f| (i, f)));
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

/// The message of the only error compiling `src` (after [`PRELUDE`]) to C.
fn error_of(src: &str) -> (CErrorKind, String) {
    let errors = to_c(&format!("{PRELUDE}{src}")).expect_err("compiling should fail");
    assert_eq!(errors.len(), 1, "{errors:#?}");
    (errors[0].kind, errors[0].message.clone())
}

#[test]
fn grapheme_builtins_are_rejected() {
    assert_eq!(
        error_of("fn main() { print(Int.to_string(String.length(\"a\"))) }\n"),
        (
            CErrorKind::Unsupported,
            "`String.length` needs the Unicode grapheme cluster tables, which target `c` does not support yet".into()
        )
    );
    assert_eq!(
        error_of("fn main() { print(String.join(String.split(\"ab\", \"\"), \",\")) }\n").1,
        "`String.split` with an empty separator needs the Unicode grapheme cluster tables, which target `c` does not support yet"
    );
}

/// An empty separator that turns up at run time splits ASCII text by characters, and is a defect
/// on other text (C has no grapheme cluster tables yet): releases run and `Fiber.attempt` catches
/// it.
#[test]
fn split_by_graphemes_at_run_time_is_a_defect() {
    if cc().is_none() {
        return;
    }
    let src = r#"
fn sep(n: Int) -> String { if n == 0 { "" } else { "," } }

effect Name {
  fn name() -> String
}

handler Res: Name {
  init { Res }
  fn name() { "res" }
  release { print("release res") }
}

fn main() -> () use { Async } {
  print(String.join(String.split("abc", sep(0)), "|"))
  r = Fiber.attempt(fn() { String.split("aé", sep(0)) })
  r match {
    Success(_) -> print("split")
    Failure(c) -> print(Cause.to_string(c))
  }
  with Res { print(String.join(String.split("é", sep(0)), "|")) }
}
"#;
    let src = format!(
        "import Std.Fiber.{{Fiber, Exit, Success, Failure}}\nimport Std.Cause.{{Cause}}\n{PRELUDE}{src}"
    );
    let got = run("split_defect", &src).expect("cc is available");
    let message = "`String.split` with an empty separator on non-ASCII text needs the Unicode \
                   grapheme cluster tables, which target `c` does not support yet";
    assert_eq!(
        (got.code, got.stdout.as_str(), got.stderr.as_str()),
        (
            101,
            format!("a|b|c\ndefect: {message}\nrelease res\n").as_str(),
            format!("defect: {message}\n").as_str()
        )
    );
}

#[test]
fn callbacks_to_c_are_rejected() {
    let (kind, message) = error_of(
        r#"
@external(c, "emela_on")
fn on(f: fn(Int) -> Int) -> () use {}

fn main() { on(fn(x) { x }) }
"#,
    );
    assert_eq!(kind, CErrorKind::Callback);
    assert_eq!(
        message,
        "external function `on` takes a callback, which target `c` does not support yet"
    );
}
