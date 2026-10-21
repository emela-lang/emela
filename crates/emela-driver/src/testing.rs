//! The `emela test` stage (spec chapter 13).
//!
//! Consists of collecting `@test` functions from source ([`collect_tests`], [`resolve_tests`];
//! called inside the [`Typecheck`] type checker), a frontend lowering to Core IR with the test
//! functions as roots ([`LowerTests`]), and turning the Core IR test functions into JS and calling
//! them one by one on node ([`TestBackend`], [`test`]).

mod collect;
mod report;

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use emela_codegen_js::{
    Options, RUNTIME, RUNTIME_FILE, RuntimeMode, TEST_EVENT_PREFIX, TestEntry,
    emit_module_exporting, emit_test_main,
};
use emela_core::{FnId, Roots};
use emela_resolve::hir::{DefId, DefKind, Program};
use emela_resolve::{BuiltinModules, ModuleId, NoBuiltins};
use serde::Deserialize;

pub use collect::{TEST_ANNOTATION, TestFn, collect_tests, is_test_fn};
pub use report::{TEST_FAILURE_EXIT_CODE, TestOutcome, TestReport, TestResult};

use crate::code;
use crate::diagnostic::Diagnostic;
use crate::frontend::{Typecheck, lower_typed};
use crate::js::{CoreJs, JS_ENTRY_FILE, bundled_outputs};
use crate::output::{BuildOutput, OutputFile, write_output};
use crate::pipeline::{
    Analysis, Backend, Checked, CoreSource, Frontend, Input, NoJsBackend, Parsed, Runner,
    build_only_diagnostic, check,
};
use crate::run::{Output, exit_code, spawn_error};
use crate::source::{FileId, FileSystem, SourceFile};
use crate::target::Target;

/// The default JS output directory for tests, relative to the project (or to the file's
/// directory for a single file). Kept apart from the normal build (`target/emela/js`) because the
/// output differs: it exports the test functions.
pub const JS_TEST_OUT_DIR: &str = "target/emela/test";
/// The default C output directory for tests (`--target c`), relative like [`JS_TEST_OUT_DIR`].
pub const C_TEST_OUT_DIR: &str = "target/emela/test-c";
/// Reserved output directory for embedded C test backends; the CLI does not run them.
pub const C_EMBEDDED_TEST_OUT_DIR: &str = "target/emela/test-c-embedded";
/// The default WASM output directory for tests (`--target wasm`), relative like
/// [`JS_TEST_OUT_DIR`].
pub const WASM_TEST_OUT_DIR: &str = "target/emela/test-wasm";

/// The file name of the test entry module, which node runs.
pub const JS_TEST_ENTRY_FILE: &str = "emela_test.mjs";

pub fn default_test_out_dir(input: &Input) -> PathBuf {
    input.base.join(JS_TEST_OUT_DIR)
}

/// The default output directory of `emela test` for the target.
pub fn default_test_out_dir_for(input: &Input, target: Target) -> PathBuf {
    match target {
        Target::Js => input.base.join(JS_TEST_OUT_DIR),
        Target::C => input.base.join(C_TEST_OUT_DIR),
        Target::CEmbedded => input.base.join(C_EMBEDDED_TEST_OUT_DIR),
        Target::Wasm => input.base.join(WASM_TEST_OUT_DIR),
    }
}

/// A `@test` function that passed type checking. `def` is its HIR definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestDef {
    pub test: TestFn,
    pub def: DefId,
}

/// Maps collected `@test` functions to HIR definitions, and reports references to test functions
/// from normal code as E0227. Test functions are removed from the normal build (spec 13.1), so
/// they cannot be referenced. References between test functions are allowed.
pub(crate) fn resolve_tests(
    analysis: &Analysis,
    hir: &Program,
    tests: Vec<TestFn>,
) -> (Vec<TestDef>, Vec<Diagnostic>) {
    // `TestFn::name_range` is the same range as the resolver's `DefData::span`.
    let tests: Vec<TestDef> = tests
        .into_iter()
        .filter_map(|test| {
            let def = hir.defs.iter().find_map(|(def, data)| {
                let span = data.span?;
                (data.kind == DefKind::Fn
                    && span.module == test.module
                    && span.range == test.name_range)
                    .then_some(def)
            })?;
            Some(TestDef { test, def })
        })
        .collect();

    let mut diagnostics = Vec::new();
    for (def, _) in hir.defs.iter() {
        if tests.iter().any(|t| t.def == def) {
            continue;
        }
        for (target, span) in emela_types::body_refs(hir, def) {
            if !tests.iter().any(|t| t.def == target) {
                continue;
            }
            let Some(&file) = analysis.files.get(span.module) else {
                continue;
            };
            diagnostics.push(
                Diagnostic::error(format!(
                    "test function `{}` can only be used from test functions",
                    hir.defs[target].name
                ))
                .with_code(code::TEST_FN_OUTSIDE_TESTS)
                .with_span(crate::diagnostic::Span::new(file, span.range))
                .with_note("test functions are removed from normal builds (13.1)"),
            );
        }
    }
    (tests, diagnostics)
}

/// A frontend that lowers to Core IR with the test functions as roots, after type checking
/// ([`Typecheck`]). The default for `emela test`. Lowering is skipped if name resolution or type
/// checking reported any error.
pub struct LowerTests<B = NoBuiltins> {
    typecheck: Typecheck<B>,
    target: Target,
}

impl Default for LowerTests<NoBuiltins> {
    fn default() -> Self {
        LowerTests {
            typecheck: Typecheck::default(),
            target: Target::Js,
        }
    }
}

impl<B> LowerTests<B> {
    /// Checks external functions against `target` (spec 12.1) instead of JS.
    pub fn with_target(mut self, target: Target) -> Self {
        self.target = target;
        self
    }
}

impl<B: BuiltinModules> Frontend for LowerTests<B> {
    type Program = TestModule;

    fn parse(&mut self, module: ModuleId, file: FileId, source: &SourceFile) -> Parsed {
        self.typecheck.parse(module, file, source)
    }

    fn core_sources(&self) -> Vec<CoreSource> {
        self.typecheck.core_sources()
    }

    fn check(&mut self, analysis: &Analysis, order: &[ModuleId]) -> Checked<TestModule> {
        let checked = self.typecheck.check(analysis, order);
        let mut diagnostics = checked.diagnostics;
        let program = checked.program.and_then(|typed| {
            let lowered = lower_typed(
                analysis,
                &typed,
                Roots::Tests,
                self.target,
                &mut diagnostics,
            )?;
            let tests = typed
                .tests
                .iter()
                .zip(lowered.tests)
                .map(|(t, function)| TestCase {
                    name: t.test.display_name.clone(),
                    function,
                })
                .collect();
            Some(TestModule {
                module: lowered.module,
                tests,
            })
        });
        Checked {
            program,
            diagnostics,
        }
    }
}

/// Core IR including test functions. Lowering builds it for tests (the normal build excludes
/// test functions).
#[derive(Debug, Clone, Default)]
pub struct TestModule {
    pub module: emela_core::Module,
    /// In declaration order.
    pub tests: Vec<TestCase>,
}

/// `function` must take no arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestCase {
    /// The name used for display and filtering ([`TestFn::display_name`]).
    pub name: String,
    pub function: FnId,
}

/// The plug-in point for test emission (JS, or C with [`crate::CoreC`]). Called only when there are no errors.
pub trait TestBackend<P> {
    /// In declaration order.
    fn test_names(&self, program: &P) -> Vec<String>;

    /// Emits an entry module that calls the `selected` tests (indices into
    /// [`TestBackend::test_names`], ascending) in order, plus the files it reads. `entry` is the
    /// entry module ([`TestBackend::finish`] turns it into what runs).
    fn emit_tests(
        &mut self,
        program: &P,
        selected: &[usize],
        analysis: &Analysis,
    ) -> Result<BuildOutput, Vec<Diagnostic>>;

    /// Called after the output is written; returns what runs (see [`Backend::finish`]).
    fn finish(&mut self, _out_dir: &Path, entry: PathBuf) -> Result<PathBuf, Vec<Diagnostic>> {
        Ok(entry)
    }

    /// How what [`TestBackend::finish`] returned runs.
    fn runner(&self) -> Runner {
        Runner::Node
    }
}

impl<P> TestBackend<P> for NoJsBackend {
    fn test_names(&self, _: &P) -> Vec<String> {
        Vec::new()
    }

    fn emit_tests(
        &mut self,
        program: &P,
        _: &[usize],
        analysis: &Analysis,
    ) -> Result<BuildOutput, Vec<Diagnostic>> {
        Backend::emit(self, program, analysis)
    }
}

impl TestBackend<TestModule> for CoreJs {
    fn test_names(&self, program: &TestModule) -> Vec<String> {
        program.tests.iter().map(|t| t.name.clone()).collect()
    }

    fn emit_tests(
        &mut self,
        program: &TestModule,
        selected: &[usize],
        analysis: &Analysis,
    ) -> Result<BuildOutput, Vec<Diagnostic>> {
        // Files `@external` points to are copied into the output and imported relatively.
        let (module, host_files) = crate::host_files::place(&program.module, analysis, "js")?;
        let module = &module;
        // Export the selected test functions under their Emela names so the entry module can call
        // them.
        let exports: Vec<_> = selected
            .iter()
            .map(|&i| program.tests[i].function)
            .collect();
        let options = Options {
            runtime: RuntimeMode::Import(None),
        };
        let entries: Vec<TestEntry> = selected
            .iter()
            .map(|&i| {
                let test = &program.tests[i];
                TestEntry {
                    name: &test.name,
                    export: &module.functions[test.function].name,
                }
            })
            .collect();
        let main = emit_test_main(&format!("./{JS_ENTRY_FILE}"), &entries);
        let mut files = vec![
            OutputFile::new(
                JS_ENTRY_FILE,
                emit_module_exporting(module, &options, &exports),
            ),
            OutputFile::new(RUNTIME_FILE, RUNTIME),
            OutputFile::new(JS_TEST_ENTRY_FILE, main),
        ];
        files.extend(bundled_outputs(module));
        files.extend(host_files);
        Ok(BuildOutput {
            files,
            entry: JS_TEST_ENTRY_FILE.into(),
        })
    }
}

/// How `emela test` runs.
#[derive(Debug, Clone)]
pub struct TestOptions {
    pub out_dir: PathBuf,
    /// The node executable ([`crate::node_program`]).
    pub node: PathBuf,
    /// A substring of test names; `None` runs all.
    pub filter: Option<String>,
    /// How node's stderr is handled. Stdout is always captured to read results.
    pub output: Output,
}

/// The `emela test` stage: checks, writes the tests out as JS, and runs them one by one on node.
///
/// Results are written to `out` as they arrive, in a format close to `cargo test`. `before_node`
/// is called once after emission and before node starts (even if checking or emission failed);
/// the CLI prints diagnostics there. The result is `None` if diagnostics had errors or node could
/// not start. A diagnostic about failing to start node is appended to `Analysis` after
/// `before_node`.
pub fn test<F: Frontend, B: TestBackend<F::Program>>(
    fs: &dyn FileSystem,
    input: &Input,
    frontend: &mut F,
    backend: &mut B,
    options: &TestOptions,
    before_node: impl FnOnce(&Analysis),
    out: &mut dyn Write,
) -> (Analysis, Option<TestReport>) {
    let (mut analysis, program) = check(fs, input, frontend);
    if backend.runner() == Runner::BuildOnly {
        analysis.diagnostics.push(build_only_diagnostic("test"));
        before_node(&analysis);
        return (analysis, None);
    }
    let prepared = prepare(&mut analysis, program, backend, options);
    before_node(&analysis);
    let Some((entry, selected, filtered_out)) = prepared else {
        return (analysis, None);
    };
    let (program, command) = match backend.runner() {
        Runner::BuildOnly => {
            analysis.diagnostics.push(build_only_diagnostic("test"));
            return (analysis, None);
        }
        Runner::Node => {
            let mut command = Command::new(&options.node);
            command.arg(&entry);
            (options.node.clone(), command)
        }
        Runner::Native => (entry.clone(), Command::new(&entry)),
        Runner::Wasmtime => {
            let wasmtime = crate::run::wasmtime_program();
            let mut command = Command::new(&wasmtime);
            command.arg("run").arg(&entry);
            (wasmtime, command)
        }
    };
    match run_tests(&program, command, &selected, options.output, out) {
        Ok((results, stderr)) => {
            let report = TestReport {
                results,
                filtered_out,
                stderr,
            };
            let _ = report::write_summary(out, &report);
            (analysis, Some(report))
        }
        Err(diagnostic) => {
            analysis.diagnostics.push(diagnostic);
            (analysis, None)
        }
    }
}

/// Writes the selected tests out as JS. Returns the entry module's path, the selected test names,
/// and the number filtered out.
fn prepare<P, B: TestBackend<P>>(
    analysis: &mut Analysis,
    program: Option<P>,
    backend: &mut B,
    options: &TestOptions,
) -> Option<(PathBuf, Vec<String>, usize)> {
    if analysis.has_errors() {
        return None;
    }
    let Some(program) = program else {
        analysis.diagnostics.push(Diagnostic::error(
            "internal error: the type checker returned no program",
        ));
        return None;
    };
    let names = backend.test_names(&program);
    let selected: Vec<usize> = names
        .iter()
        .enumerate()
        .filter(|(_, name)| {
            options
                .filter
                .as_deref()
                .is_none_or(|filter| name.contains(filter))
        })
        .map(|(i, _)| i)
        .collect();
    let output = match backend.emit_tests(&program, &selected, analysis) {
        Ok(output) => output,
        Err(diagnostics) => {
            analysis.diagnostics.extend(diagnostics);
            return None;
        }
    };
    let entry = match write_output(&options.out_dir, &output) {
        Ok(entry) => entry,
        Err(diagnostic) => {
            analysis.diagnostics.push(diagnostic);
            return None;
        }
    };
    let entry = match backend.finish(&options.out_dir, entry) {
        Ok(entry) => entry,
        Err(diagnostics) => {
            analysis.diagnostics.extend(diagnostics);
            return None;
        }
    };
    let filtered_out = names.len() - selected.len();
    let selected = selected.into_iter().map(|i| names[i].clone()).collect();
    Some((entry, selected, filtered_out))
}

/// A result line written by the test runner (`test_runner.mjs`).
#[derive(Deserialize)]
#[serde(tag = "event", rename_all = "lowercase")]
enum Event {
    Result {
        name: String,
        outcome: Outcome,
        #[serde(default)]
        message: String,
    },
    Done,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Outcome {
    Ok,
    Defect,
    Error,
    Interrupted,
}

/// Runs the tests with `command` (node on the entry module, or the test executable; `program`
/// names it in diagnostics), writing results to `out` as they are collected. Stdout lines that are
/// not result lines pass through to `out`.
fn run_tests(
    program: &Path,
    mut command: Command,
    selected: &[String],
    output: Output,
    out: &mut dyn Write,
) -> Result<(Vec<TestResult>, Vec<u8>), Diagnostic> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(match output {
            Output::Inherit => Stdio::inherit(),
            Output::Capture => Stdio::piped(),
        });
    let mut child = command.spawn().map_err(|err| spawn_error(program, &err))?;
    // Read stderr on another thread so a full pipe cannot stall node.
    let stderr = child.stderr.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            buf
        })
    });

    let _ = report::write_header(out, selected.len());
    let mut results = Vec::new();
    let mut finished = false;
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout is piped"));
    let mut line = Vec::new();
    loop {
        line.clear();
        match stdout.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let event = line
            .strip_prefix(TEST_EVENT_PREFIX.as_bytes())
            .and_then(|json| serde_json::from_slice::<Event>(json).ok());
        match event {
            Some(Event::Result {
                name,
                outcome,
                message,
            }) => {
                let outcome = match outcome {
                    Outcome::Ok => TestOutcome::Passed,
                    Outcome::Defect => TestOutcome::Defect(message),
                    Outcome::Error => TestOutcome::Error(message),
                    Outcome::Interrupted => TestOutcome::Interrupted,
                };
                let result = TestResult { name, outcome };
                let _ = report::write_result(out, &result);
                results.push(result);
            }
            Some(Event::Done) => finished = true,
            None => {
                let _ = out.write_all(&line);
            }
        }
    }
    let status = child.wait().map_err(|err| spawn_error(program, &err))?;
    let stderr = stderr
        .map(|thread| thread.join().unwrap_or_default())
        .unwrap_or_default();
    if !finished || results.len() < selected.len() {
        // node exited midway (`process.exit` from an external function, or an output error). The
        // test that was running fails, and the rest are reported as not run.
        let code = exit_code(status);
        if let Some(name) = selected.get(results.len()) {
            let result = TestResult {
                name: name.clone(),
                outcome: TestOutcome::Aborted {
                    code,
                    not_run: selected.len() - results.len() - 1,
                },
            };
            let _ = report::write_result(out, &result);
            results.push(result);
        }
    }
    Ok((results, stderr))
}
