//! The examples (`examples/*`) on the JS and the WASM target (spec chapter 16: every target gives
//! the same observable result). Each run compares the exit code, stdout and stderr, and each
//! example's `@test` results must agree too. This mirrors `parity.rs` (JS against C) for WASM
//! without repeating the inline programs of `wasm_parity.rs`.
//!
//! The examples whose features the WASM target does not have yet are grouped by feature below.
//! [`skipped_examples_still_fail`] runs them on WASM (and, once the run matches JS, their tests)
//! and checks that the expected failure is still present: an example that now matches JS makes
//! the test fail, and one that fails for another reason does too, so a skip cannot outlive its
//! feature or hide a different bug.
//!
//! Needs node (`EMELA_NODE` or `node`) and wasmtime 49 or later (`EMELA_WASMTIME` or `wasmtime`);
//! without wasmtime the tests are skipped with a message.

mod wasm_common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use emela_driver::{
    Analysis, CoreJs, CoreWasm, Input, Lower, LowerTests, OsFs, Output, RunOptions, RunOutput,
    Target, TestOptions, TestOutcome, node_program, run, test, wasmtime_program, with_program_cwd,
    with_program_timeout,
};
use wasm_common::{PROGRAM_TIMEOUT, have_wasmtime, matches_any, messages, out_dir};

/// One feature the WASM target does not have yet, and the examples that use it.
struct Skipped {
    feature: &'static str,
    reason: &'static str,
    /// A diagnostic or runtime message that must still appear while the feature is missing. Each
    /// names this feature (its missing host function, or its own diagnostic), so a failure from
    /// another missing feature does not keep the skip.
    expected_failures: &'static [&'static str],
    examples: &'static [&'static str],
}

/// The examples whose feature the WASM target does not support yet.
///
/// `fibers`, `http` and `notes` also use Console, which is available now; they stay skipped for
/// their own feature below.
const SKIPPED: &[Skipped] = &[
    Skipped {
        feature: "file system",
        reason: "Std.FileSystem has no WASM host yet",
        expected_failures: &[
            "external function `host_read_text` has no `@external` for target `wasm`",
        ],
        examples: &["notes"],
    },
    Skipped {
        feature: "fiber",
        reason: "fibers are not supported on target `wasm` yet",
        expected_failures: &["fibers are not supported on target `wasm` yet"],
        examples: &["fibers"],
    },
    Skipped {
        feature: "http",
        reason: "Std.Http has no WASM host yet",
        expected_failures: &[
            "external function `host_request` has no `@external` for target `wasm`",
        ],
        examples: &["http"],
    },
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Runs `f` in a fresh temporary working directory for the child node/wasmtime process, and
/// removes it afterwards (including on panic). Each call gets its own directory, so tests that
/// run in parallel and examples that write files cannot see or delete each other's files.
fn in_temp_cwd<T>(name: &str, f: impl FnOnce() -> T) -> T {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = out_dir("examples-cwd", &format!("{name}-{id}"));
    std::fs::create_dir_all(&dir).unwrap();

    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    let _cleanup = Cleanup(dir.clone());
    with_program_cwd(&dir, f)
}

type Outcome = (i32, String, String);

/// Runs the project at `path` on `target`: its exit code, stdout and stderr, or the diagnostics
/// that stopped it.
fn run_on(path: &Path, target: Target, name: &str) -> Result<Outcome, Box<Analysis>> {
    let input = Input::resolve(&OsFs, path).unwrap();
    let options = RunOptions {
        out_dir: out_dir("examples", &format!("{name}-{target}")),
        node: match target {
            Target::Wasm => wasmtime_program(),
            _ => node_program(),
        },
        args: Vec::new(),
        output: Output::Capture,
    };
    let mut frontend = Lower::default().with_target(target);
    let (analysis, output) = with_program_timeout(PROGRAM_TIMEOUT, || match target {
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
    });
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
        out_dir: out_dir("examples", &format!("{name}-test-{target}")),
        node: match target {
            Target::Wasm => wasmtime_program(),
            _ => node_program(),
        },
        filter: None,
        output: Output::Capture,
    };
    let mut frontend = LowerTests::default().with_target(target);
    let mut out = Vec::new();
    let (analysis, report) = with_program_timeout(PROGRAM_TIMEOUT, || match target {
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
    });
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

fn skip_of(name: &str) -> Option<&'static Skipped> {
    SKIPPED.iter().find(|skip| skip.examples.contains(&name))
}

#[test]
fn examples_run_the_same_on_js_and_wasm() {
    if !have_wasmtime() {
        return;
    }
    let mut failures = Vec::new();
    for (name, dir) in examples() {
        if let Some(skip) = skip_of(&name) {
            eprintln!("skipping {name} ({}): {}", skip.feature, skip.reason);
            continue;
        }
        let js = in_temp_cwd(&format!("run-js-{name}"), || {
            run_on(&dir, Target::Js, &name)
        })
        .unwrap_or_else(|a| panic!("{name} does not run on JS:\n{}", messages(&a)));
        match in_temp_cwd(&format!("run-wasm-{name}"), || {
            run_on(&dir, Target::Wasm, &name)
        }) {
            Ok(on_wasm) if on_wasm == js => {}
            Ok(on_wasm) => {
                failures.push(format!("{name}: JS gives {js:?}\nWASM gives {on_wasm:?}"))
            }
            Err(analysis) => failures.push(format!(
                "{name} does not build on WASM:\n{}",
                messages(&analysis)
            )),
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
}

#[test]
fn example_tests_give_the_same_results_on_js_and_wasm() {
    if !have_wasmtime() {
        return;
    }
    let mut failures = Vec::new();
    for (name, dir) in examples() {
        if skip_of(&name).is_some() {
            continue;
        }
        let js = in_temp_cwd(&format!("test-js-{name}"), || {
            test_on(&dir, Target::Js, &name)
        })
        .unwrap_or_else(|a| panic!("the tests of {name} fail on JS:\n{}", messages(&a)));
        match in_temp_cwd(&format!("test-wasm-{name}"), || {
            test_on(&dir, Target::Wasm, &name)
        }) {
            Ok(on_wasm) if on_wasm == js => {}
            Ok(on_wasm) => {
                failures.push(format!("{name}: JS gives {js:?}\nWASM gives {on_wasm:?}"))
            }
            Err(analysis) => failures.push(format!(
                "the tests of {name} do not build on WASM:\n{}",
                messages(&analysis)
            )),
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
}

/// The `@test` results on WASM that differ from JS, one `name: outcome` per line, so a skip can
/// check the failure is its own.
fn differing_tests(js: &[(String, TestOutcome)], wasm: &[(String, TestOutcome)]) -> String {
    wasm.iter()
        .filter(|result| !js.contains(result))
        .map(|(name, outcome)| format!("{name}: {outcome:?}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every skip names an example, and one that now runs (and whose tests run) on WASM makes this
/// fail: once the run and the `@test` results agree with JS, the feature landed and the entry
/// must be removed from [`SKIPPED`]. A different failure also fails, so the skip stays tied to
/// its feature.
#[test]
fn skipped_examples_still_fail() {
    if !have_wasmtime() {
        return;
    }
    let mut failures = Vec::new();
    for skip in SKIPPED {
        for &name in skip.examples {
            let dir = root().join("examples").join(name);
            if !dir.join("Pome.toml").exists() {
                failures.push(format!(
                    "{name} ({}) is not an example: remove it from the skip list",
                    skip.feature
                ));
                continue;
            }
            let on_wasm = in_temp_cwd(&format!("stale-run-wasm-{name}"), || {
                run_on(&dir, Target::Wasm, name)
            });
            let on_wasm = match on_wasm {
                Err(analysis) => {
                    let message = messages(&analysis);
                    if !matches_any(&message, skip.expected_failures) {
                        failures.push(format!(
                            "{name} ({}) fails for another reason:\n{message}",
                            skip.feature
                        ));
                    }
                    continue;
                }
                Ok(on_wasm) => on_wasm,
            };
            let js = in_temp_cwd(&format!("stale-run-js-{name}"), || {
                run_on(&dir, Target::Js, name)
            })
            .unwrap_or_else(|a| panic!("{name} does not run on JS:\n{}", messages(&a)));
            if on_wasm != js {
                let text = format!("{}{}", on_wasm.1, on_wasm.2);
                if !matches_any(&text, skip.expected_failures) {
                    failures.push(format!(
                        "{name} ({}) fails for another reason:\n{text}",
                        skip.feature
                    ));
                }
                continue;
            }

            // The run matches JS; the tests must match too for the skip to be stale.
            let wasm_tests = in_temp_cwd(&format!("stale-test-wasm-{name}"), || {
                test_on(&dir, Target::Wasm, name)
            });
            let js_tests = in_temp_cwd(&format!("stale-test-js-{name}"), || {
                test_on(&dir, Target::Js, name)
            })
            .unwrap_or_else(|a| panic!("the tests of {name} do not run on JS:\n{}", messages(&a)));
            let text = match wasm_tests {
                Ok(wasm_tests) if wasm_tests == js_tests => {
                    failures.push(format!(
                        "{name} now matches JS on WASM (run and @test): the {} skip is stale",
                        skip.feature
                    ));
                    continue;
                }
                Ok(wasm_tests) => differing_tests(&js_tests, &wasm_tests),
                Err(analysis) => messages(&analysis),
            };
            if !matches_any(&text, skip.expected_failures) {
                failures.push(format!(
                    "{name} ({}) tests fail for another reason:\n{text}",
                    skip.feature
                ));
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
