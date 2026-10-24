//! The C target's acceptance corpus (`crates/emela-codegen-c/tests/programs`) run on the WASM
//! target.
//!
//! Each `NAME.emel` has `NAME.expected`, the exit code, stdout and stderr of the JS target. The
//! WASM run must give the same result. The corpus declares `print` with `@external(js, ...)` and
//! `@external(c, ...)`. The WASM target supports only the externals of `Std`, so this harness
//! replaces the declaration with a function that prints through `Std.Console` and
//! `SystemConsole`, just as the C test's helper provides the C implementation.
//!
//! The target does not support every feature of the corpus yet. The skips are grouped by feature
//! below, so a program is skipped for one known reason rather than by one growable blocklist.
//! [`skipped_programs_still_fail`] reruns every skipped program: one that now matches its
//! `.expected` fails the test, so a skip cannot outlive the feature it waits for. It also checks
//! that the failure still has the expected message, so a skip cannot hide a different bug.
//!
//! Needs wasmtime 49 or later (`EMELA_WASMTIME` or `wasmtime`); the tests are skipped with a
//! message when it cannot start or is older.

mod wasm_common;

use std::path::{Path, PathBuf};

use emela_driver::{
    Analysis, CoreWasm, Input, Lower, MemoryFiles, Output, RunOptions, Target, run,
    with_program_timeout,
};
use wasm_common::{PROGRAM_TIMEOUT, have_wasmtime, matches_any, messages, out_dir};

/// One feature the WASM target does not have yet, and the corpus programs that use it.
struct Skipped {
    /// The feature the programs need (shown in the skip message).
    feature: &'static str,
    /// Why the WASM target cannot run them yet.
    reason: &'static str,
    /// A diagnostic or runtime message that must still appear while the feature is missing. Each
    /// names this feature (its own diagnostic, or its missing host function), so a failure from
    /// another missing feature does not keep the skip.
    expected_failures: &'static [&'static str],
    programs: &'static [&'static str],
}

/// The corpus programs whose feature the WASM target does not support yet.
const SKIPPED: &[Skipped] = &[
    Skipped {
        feature: "fiber",
        reason: "fibers are not supported on target `wasm` yet",
        expected_failures: &["fibers are not supported on target `wasm` yet"],
        programs: &[
            "fiber_deferred",
            "fiber_failures",
            "fiber_fairness",
            "fiber_main_fails",
            "fiber_notes",
            "fiber_scope",
            "fiber_unwinding",
            "generic_main",
            // The suspend state machine works, but these three reach it through `Fiber.fork` /
            // `Deferred`, so the fiber check still stops them first.
            "suspend_abandoned",
            "suspend_after_resume",
            "suspend_resume",
        ],
    },
    // The program's own `sleep` is a custom `suspend fn` external with no WASM host (the
    // `Std.Clock` host only covers the bundled handler), and its races need fibers.
    Skipped {
        feature: "custom sleep host and fiber",
        reason: "the program's `sleep` has no WASM host, and its races need fibers",
        expected_failures: &[
            "has no `@external` for target `wasm`",
            "fibers are not supported",
        ],
        programs: &["suspend_external"],
    },
];

/// `exit: N`, `--- stdout`, `--- stderr`: the format the C corpus test parses.
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

/// The corpus directory.
fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../emela-codegen-c/tests/programs")
}

/// The corpus programs, sorted by name.
fn programs() -> Vec<(String, PathBuf)> {
    let mut programs: Vec<(String, PathBuf)> = std::fs::read_dir(corpus())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "emel"))
        .map(|path| {
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            (name, path)
        })
        .collect();
    programs.sort();
    assert!(!programs.is_empty());
    programs
}

fn skip_of(name: &str) -> Option<&'static Skipped> {
    SKIPPED.iter().find(|skip| skip.programs.contains(&name))
}

/// Replaces the corpus `print` external with a function that writes the line through
/// `Std.Console` (whose WASI 0.3 stdout host the WASM target provides) and imports `Std.Console`.
/// Programs without `print` (for example `core_bool`) are returned unchanged.
fn with_wasm_print(src: &str) -> String {
    if !src.lines().any(|line| line.starts_with("fn print(")) {
        return src.to_owned();
    }
    let mut out = String::from("import Std.Console.{Console, SystemConsole}\n");
    let mut attributes = Vec::new();
    for line in src.lines() {
        if line.starts_with("@external(") {
            attributes.push(line);
            continue;
        }
        if line.starts_with("fn print(") {
            out.push_str(
                "fn print(s: String) -> () use {} {\n  with SystemConsole { (use Console).print_line(s) }\n}\n",
            );
        } else {
            for attribute in attributes.iter() {
                out.push_str(attribute);
                out.push('\n');
            }
            out.push_str(line);
            out.push('\n');
        }
        attributes.clear();
    }
    out
}

type Outcome = (i32, String, String);

/// Runs `src` on the WASM target, or returns the diagnostics that stopped it.
fn run_wasm(name: &str, src: &str) -> Result<Outcome, Box<Analysis>> {
    let fs = MemoryFiles::new([("app/Pome.toml", ""), ("app/src/main.emel", src)]);
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let options = RunOptions {
        out_dir: out_dir("corpus", name),
        node: emela_driver::wasmtime_program(),
        args: Vec::new(),
        output: Output::Capture,
    };
    let (analysis, output) = with_program_timeout(PROGRAM_TIMEOUT, || {
        run(
            &fs,
            &input,
            &mut Lower::default().with_target(Target::Wasm),
            &mut CoreWasm,
            &options,
            |_, _| {},
        )
    });
    let _ = std::fs::remove_dir_all(&options.out_dir);
    match output {
        Some(output) if !analysis.has_errors() => Ok((
            output.code,
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )),
        _ => Err(Box::new(analysis)),
    }
}

#[test]
fn programs_behave_as_on_js() {
    if !have_wasmtime() {
        return;
    }
    let mut failures = Vec::new();
    for (name, path) in programs() {
        if let Some(skip) = skip_of(&name) {
            eprintln!("skipping {name} ({}): {}", skip.feature, skip.reason);
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap();
        let expected = parse_expected(
            &std::fs::read_to_string(path.with_extension("expected"))
                .unwrap_or_else(|_| panic!("{name}.expected is missing")),
        );
        match run_wasm(&name, &with_wasm_print(&src)) {
            Ok(got) => {
                if (got.0, got.1.as_str(), got.2.as_str())
                    != (
                        expected.code,
                        expected.stdout.as_str(),
                        expected.stderr.as_str(),
                    )
                {
                    failures.push(format!(
                        "{name}: expected exit {}\n{}{}\ngot exit {}\n{}{}",
                        expected.code, expected.stdout, expected.stderr, got.0, got.1, got.2
                    ));
                }
            }
            Err(analysis) => failures.push(format!(
                "{name}: WASM did not build:\n{}",
                messages(&analysis)
            )),
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
}

/// Every skip names a corpus program, and no skipped program matches its `.expected` yet: once it
/// does, the feature landed and the entry must be removed from [`SKIPPED`]. A program that now
/// fails for a different reason also fails the test, so the skip stays tied to its feature.
#[test]
fn skipped_programs_still_fail() {
    if !have_wasmtime() {
        return;
    }
    let mut failures = Vec::new();
    for skip in SKIPPED {
        for &name in skip.programs {
            let path = corpus().join(format!("{name}.emel"));
            if !path.exists() {
                failures.push(format!(
                    "{name} ({}) is not in the corpus: remove it from the skip list",
                    skip.feature
                ));
                continue;
            }
            let src = with_wasm_print(&std::fs::read_to_string(&path).unwrap());
            let expected = parse_expected(
                &std::fs::read_to_string(path.with_extension("expected"))
                    .unwrap_or_else(|_| panic!("{name}.expected is missing")),
            );
            match run_wasm(name, &src) {
                Ok(got)
                    if (got.0, got.1.as_str(), got.2.as_str())
                        == (
                            expected.code,
                            expected.stdout.as_str(),
                            expected.stderr.as_str(),
                        ) =>
                {
                    failures.push(format!(
                        "{name} now matches its expected output on WASM: the {} skip is stale",
                        skip.feature
                    ));
                }
                Ok(got) => {
                    let text = format!("{}{}", got.1, got.2);
                    if !matches_any(&text, skip.expected_failures) {
                        failures.push(format!(
                            "{name} ({}) still fails, but not with the expected message:\n{text}",
                            skip.feature
                        ));
                    }
                }
                Err(analysis) => {
                    let diagnostics = messages(&analysis);
                    if !matches_any(&diagnostics, skip.expected_failures) {
                        failures.push(format!(
                            "{name} ({}) still fails, but not with the expected message:\n{diagnostics}",
                            skip.feature
                        ));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
}

/// Every skip category names its feature and the failure that must still be present, so an entry
/// cannot be an unexplained exclusion.
#[test]
fn skip_categories_are_explained() {
    for skip in SKIPPED {
        assert!(!skip.feature.is_empty());
        assert!(!skip.reason.is_empty());
        assert!(!skip.expected_failures.is_empty());
    }
}
