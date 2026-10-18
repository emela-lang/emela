//! `emela test` of a `c-embedded` project on the board or an emulator
//! (`docs/design/c-embedded-architecture.md`, 決定: テスト).
//!
//! The selected tests are written out as one firmware (embedded C), and a runner the project
//! provides (`[embedded.test] runner`) builds it, runs it and passes what the firmware writes to
//! the console to its stdout. The firmware writes the result lines of the host test runners. A
//! defect stops the board after the result of its test, so the runner is started again from the
//! next test, which the firmware reads from `emela_test_start.h`.

use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use emela_codegen_c_embedded::{TEST_START_H_FILE, TestCase as EmbeddedTest, compile_tests};

use super::report;
use super::{TestModule, TestOptions, TestOutcome, TestReport, TestResult, read_results};
use crate::diagnostic::Diagnostic;
use crate::output::{OutputFile, write_files};
use crate::pipeline::{Analysis, Frontend, Input, check};
use crate::run::exit_code;
use crate::source::FileSystem;

/// How the tests run on the board.
#[derive(Debug, Clone)]
pub struct DeviceTests {
    /// The embedded C settings of the project.
    pub options: emela_codegen_c_embedded::Options,
    /// The runner: called as `runner <output directory>` from the project's directory.
    pub runner: PathBuf,
}

/// Checks, writes the selected tests out as a firmware to `options.out_dir`, and runs it with the
/// runner until every test has a result. Results go to `out` as they arrive; `before_run` is
/// called once before the runner first starts (the CLI prints diagnostics there).
pub fn test_on_device<F: Frontend<Program = TestModule>>(
    fs: &dyn FileSystem,
    input: &Input,
    frontend: &mut F,
    device: &DeviceTests,
    options: &TestOptions,
    before_run: impl FnOnce(&Analysis),
    out: &mut dyn Write,
) -> (Analysis, Option<TestReport>) {
    let (mut analysis, program) = check(fs, input, frontend);
    let prepared = match program {
        Some(program) if !analysis.has_errors() => prepare(&program, device, options),
        _ => Err(Vec::new()),
    };
    if let Err(diagnostics) = &prepared {
        analysis.diagnostics.extend(diagnostics.iter().cloned());
    }
    before_run(&analysis);
    let Ok((selected, filtered_out)) = prepared else {
        return (analysis, None);
    };
    let _ = report::write_header(out, selected.len());
    let mut results: Vec<TestResult> = Vec::new();
    let mut stderr = Vec::new();
    while results.len() < selected.len() {
        let start = results.len();
        let header = format!("#define EMELA_TEST_START {start}\n");
        if let Err(diagnostic) = std::fs::write(options.out_dir.join(TEST_START_H_FILE), header)
            .map_err(|e| Diagnostic::error(format!("cannot write the tests' start: {e}")))
        {
            analysis.diagnostics.push(diagnostic);
            return (analysis, None);
        }
        let run = match run_once(&device.runner, input, &options.out_dir, out) {
            Ok(run) => run,
            Err(diagnostic) => {
                analysis.diagnostics.push(diagnostic);
                return (analysis, None);
            }
        };
        stderr.extend(run.stderr);
        let got = run.results.len();
        results.extend(run.results);
        if run.finished {
            break;
        }
        if got == 0 {
            // The runner ended without a result (a fault, a timeout, or a build error): the test
            // that was to run fails, and the next run starts after it.
            let result = TestResult {
                name: selected[start].clone(),
                outcome: TestOutcome::Aborted {
                    code: run.code,
                    not_run: 0,
                },
            };
            let _ = report::write_result(out, &result);
            results.push(result);
        }
    }
    let report = TestReport {
        results,
        filtered_out,
        stderr,
    };
    let _ = report::write_summary(out, &report);
    (analysis, Some(report))
}

/// Writes the firmware of the tests the filter selects. Returns their names and the number
/// filtered out.
fn prepare(
    program: &TestModule,
    device: &DeviceTests,
    options: &TestOptions,
) -> Result<(Vec<String>, usize), Vec<Diagnostic>> {
    let tests: Vec<EmbeddedTest> = program
        .tests
        .iter()
        .filter(|t| {
            options
                .filter
                .as_deref()
                .is_none_or(|filter| t.name.contains(filter))
        })
        .map(|t| EmbeddedTest {
            name: t.name.clone(),
            function: t.function,
        })
        .collect();
    let filtered_out = program.tests.len() - tests.len();
    let names: Vec<String> = tests.iter().map(|t| t.name.clone()).collect();
    if tests.is_empty() {
        return Ok((names, filtered_out));
    }
    let output = compile_tests(&program.module, &tests, &device.options).map_err(|errors| {
        errors
            .into_iter()
            .map(|error| Diagnostic::error(error.message))
            .collect::<Vec<_>>()
    })?;
    let mut files: Vec<OutputFile> = output
        .files
        .into_iter()
        .map(|(name, contents)| OutputFile::new(&name, contents))
        .collect();
    for (name, path) in output.copies {
        let contents = std::fs::read(&path).map_err(|e| {
            vec![Diagnostic::error(format!(
                "cannot read `{}`, which an external function points to: {e}",
                path.display()
            ))]
        })?;
        files.push(OutputFile::new(&name, contents));
    }
    // The runner builds every source in the directory: first remove what the last run wrote
    // (listed in the manifest), and nothing else.
    let manifest = options.out_dir.join(MANIFEST_FILE);
    if let Ok(previous) = std::fs::read_to_string(&manifest) {
        for name in previous
            .lines()
            .filter(|n| !n.is_empty() && !n.contains(['/', '\\']))
        {
            let _ = std::fs::remove_file(options.out_dir.join(name));
        }
    }
    let mut listed: String = files
        .iter()
        .map(|f| format!("{}\n", f.path.display()))
        .collect();
    listed.push_str(TEST_START_H_FILE);
    listed.push('\n');
    files.push(OutputFile::new(MANIFEST_FILE, listed));
    write_files(&options.out_dir, &files).map_err(|d| vec![d])?;
    Ok((names, filtered_out))
}

/// The files the last run wrote, removed before the next one writes.
const MANIFEST_FILE: &str = "emela_files.txt";

/// One run of the runner.
struct Run {
    results: Vec<TestResult>,
    /// Whether the firmware wrote the done line.
    finished: bool,
    /// The runner's exit code.
    code: i32,
    stderr: Vec<u8>,
}

fn run_once(
    runner: &Path,
    input: &Input,
    dir: &Path,
    out: &mut dyn Write,
) -> Result<Run, Diagnostic> {
    let runner_error = |e: std::io::Error| {
        Diagnostic::error(format!(
            "cannot run the test runner `{}`: {e}",
            runner.display()
        ))
        .with_note("set `[embedded.test] runner` in Pome.toml to a program that builds the firmware in the directory it is given, runs it, and writes its console output to stdout")
    };
    let mut child = Command::new(runner)
        .arg(dir)
        .current_dir(&input.base)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(runner_error)?;
    let stderr = child.stderr.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            buf
        })
    });
    let stdout = BufReader::new(child.stdout.take().expect("stdout is piped"));
    let (results, finished) = read_results(stdout, out);
    let status = child.wait().map_err(runner_error)?;
    let stderr = stderr
        .map(|thread| thread.join().unwrap_or_default())
        .unwrap_or_default();
    let code = exit_code(status);
    // A runner that fails without a result shows why (the build's errors, for example).
    if results.is_empty() && !finished {
        let _ = out.write_all(&stderr);
    }
    Ok(Run {
        results,
        finished,
        code,
        stderr,
    })
}
