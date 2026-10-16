//! The C target (spec chapter 16): the output stage that turns the Core IR into C with
//! emela-codegen-c, compiles it with cc and runs the executable.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use emela_codegen_c::{
    BUNDLED_DIR, CError, CErrorKind, COutput, EMELA_H, EMELA_H_FILE, Entry, RUNTIME_C,
    RUNTIME_C_FILE, RUNTIME_H, RUNTIME_H_FILE, TestEntry, compile,
};

use crate::code;
use crate::diagnostic::Diagnostic;
use crate::output::{JsOutput, OutputFile};
use crate::pipeline::{Analysis, JsBackend, Runner};
use crate::run::{Output, RunOutput, exit_code};
use crate::testing::{TestBackend, TestModule};

/// The program written to the output directory.
pub const C_ENTRY_FILE: &str = "main.c";
/// The executable cc makes in the output directory.
pub const C_EXECUTABLE: &str = "main";
/// An environment variable overriding the C compiler; `cc` on PATH otherwise. It may carry flags
/// after the program, separated by spaces (`clang -fsanitize=address`).
pub const CC_ENV: &str = "EMELA_CC";

/// The flags every C build gets. `-fno-strict-aliasing`: the runtime reads static objects through
/// the type of a heap object. `-pthread`: completions may come from other threads (spec 12.6).
/// `-I.`: host C files include `"emela.h"` from the output directory.
const CC_FLAGS: [&str; 5] = ["-std=c11", "-O2", "-fno-strict-aliasing", "-pthread", "-I."];

/// Turns the Core IR into C and builds it with cc (`emela build --target c`). Host C files that
/// external functions point to are compiled with it.
#[derive(Debug, Default, Clone)]
pub struct CoreC {
    /// The C sources to compile, relative to the output directory or absolute: set by `emit`.
    sources: Vec<PathBuf>,
    /// The C compiler and its flags; [`cc_command`] if `None`.
    cc: Option<(PathBuf, Vec<String>)>,
}

impl CoreC {
    /// Builds with the C compiler `cc` instead of `EMELA_CC` or `cc` (tests).
    pub fn with_cc(cc: impl Into<PathBuf>, flags: &[&str]) -> Self {
        CoreC {
            sources: Vec::new(),
            cc: Some((cc.into(), flags.iter().map(|f| (*f).to_owned()).collect())),
        }
    }

    fn build(&self, out_dir: &Path) -> Result<PathBuf, Vec<Diagnostic>> {
        let (cc, flags) = self.cc.clone().unwrap_or_else(cc_command);
        build_executable(&cc, &flags, out_dir, &self.sources).map_err(|d| vec![d])
    }

    fn output(&mut self, out: COutput) -> JsOutput {
        let mut files = vec![
            OutputFile::new(C_ENTRY_FILE, out.main),
            OutputFile::new(RUNTIME_C_FILE, RUNTIME_C),
            OutputFile::new(RUNTIME_H_FILE, RUNTIME_H),
            OutputFile::new(EMELA_H_FILE, EMELA_H),
        ];
        self.sources = vec![PathBuf::from(C_ENTRY_FILE), PathBuf::from(RUNTIME_C_FILE)];
        for (path, contents) in out.bundled {
            let path = PathBuf::from(BUNDLED_DIR).join(path);
            self.sources.push(path.clone());
            files.push(OutputFile::new(path, contents));
        }
        self.sources
            .extend(out.host_files.into_iter().map(PathBuf::from));
        JsOutput {
            files,
            entry: C_ENTRY_FILE.into(),
        }
    }
}

impl JsBackend<emela_core::Module> for CoreC {
    fn emit(
        &mut self,
        module: &emela_core::Module,
        _: &Analysis,
    ) -> Result<JsOutput, Vec<Diagnostic>> {
        let out = compile(module, &Entry::Main).map_err(diagnostics)?;
        Ok(self.output(out))
    }

    fn finish(&mut self, out_dir: &Path, _entry: PathBuf) -> Result<PathBuf, Vec<Diagnostic>> {
        self.build(out_dir)
    }

    fn runner(&self) -> Runner {
        Runner::Native
    }
}

impl TestBackend<TestModule> for CoreC {
    fn test_names(&self, program: &TestModule) -> Vec<String> {
        program.tests.iter().map(|t| t.name.clone()).collect()
    }

    fn emit_tests(
        &mut self,
        program: &TestModule,
        selected: &[usize],
        _: &Analysis,
    ) -> Result<JsOutput, Vec<Diagnostic>> {
        let tests = selected
            .iter()
            .map(|&i| TestEntry {
                name: program.tests[i].name.clone(),
                function: program.tests[i].function,
            })
            .collect();
        let out = compile(&program.module, &Entry::Tests(tests)).map_err(diagnostics)?;
        Ok(self.output(out))
    }

    fn finish(&mut self, out_dir: &Path, _entry: PathBuf) -> Result<PathBuf, Vec<Diagnostic>> {
        self.build(out_dir)
    }

    fn runner(&self) -> Runner {
        Runner::Native
    }
}

fn diagnostics(errors: Vec<CError>) -> Vec<Diagnostic> {
    errors
        .into_iter()
        .map(|e| {
            let d = Diagnostic::error(e.message);
            match e.kind {
                CErrorKind::Callback => d.with_code(code::CALLBACK_EXTERNAL),
                CErrorKind::Unsupported | CErrorKind::Internal => d,
            }
        })
        .collect()
}

/// `EMELA_CC` (the program and its flags) if set, otherwise `cc`.
pub fn cc_command() -> (PathBuf, Vec<String>) {
    let configured = std::env::var(CC_ENV).ok().filter(|v| !v.trim().is_empty());
    match configured {
        Some(value) => {
            let mut words = value.split_whitespace().map(str::to_owned);
            let program = words.next().expect("not blank");
            (PathBuf::from(program), words.collect())
        }
        None => (PathBuf::from("cc"), Vec::new()),
    }
}

/// Compiles `sources` (relative to `out_dir`, or absolute) with `cc` and `flags` into the
/// executable `out_dir/main`. E0907 if cc cannot start, E0908 if it fails (with its messages as a
/// note).
pub fn build_executable(
    cc: &Path,
    flags: &[String],
    out_dir: &Path,
    sources: &[PathBuf],
) -> Result<PathBuf, Diagnostic> {
    let output = Command::new(cc)
        .args(flags)
        .args(CC_FLAGS)
        .arg("-o")
        .arg(C_EXECUTABLE)
        .args(sources)
        .arg("-lm")
        .current_dir(out_dir)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| cc_spawn_error(cc, &err))?;
    if !output.status.success() {
        let messages = String::from_utf8_lossy(&output.stderr);
        let mut d = Diagnostic::error(format!(
            "`{}` failed to compile `{}`",
            cc.display(),
            out_dir.join(C_ENTRY_FILE).display()
        ))
        .with_code(code::CC_FAILED);
        let messages = messages.trim();
        if !messages.is_empty() {
            d = d.with_note(truncate(messages, 4000));
        }
        return Err(d);
    }
    Ok(out_dir.join(C_EXECUTABLE))
}

fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n...", &text[..end])
}

fn cc_spawn_error(cc: &Path, err: &io::Error) -> Diagnostic {
    let d = Diagnostic::error(format!("cannot start `{}`: {err}", cc.display()))
        .with_code(code::CC_UNAVAILABLE);
    match err.kind() {
        io::ErrorKind::NotFound => d.with_note(format!(
            "install a C11 compiler and add it to PATH as `cc`, or set {CC_ENV} to the compiler"
        )),
        _ => d,
    }
}

/// Runs the executable `program` with `args`.
pub fn run_native(
    program: &Path,
    args: &[OsString],
    output: Output,
) -> Result<RunOutput, Diagnostic> {
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::inherit());
    let result = match output {
        Output::Inherit => command.status().map(|status| RunOutput {
            code: exit_code(status),
            stdout: Vec::new(),
            stderr: Vec::new(),
        }),
        Output::Capture => command.output().map(|out| RunOutput {
            code: exit_code(out.status),
            stdout: out.stdout,
            stderr: out.stderr,
        }),
    };
    result.map_err(|err| Diagnostic::error(format!("cannot start `{}`: {err}", program.display())))
}
