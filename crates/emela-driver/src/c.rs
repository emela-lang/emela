//! The native target (`[target.native]`): the output stage that turns the Core IR into C with
//! emela-codegen-c, compiles it with the C compiler of the machine and runs the executable.

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
use crate::output::{BuildOutput, OutputFile};
use crate::pipeline::{Analysis, Backend, Runner};
use crate::run::{Output, ProgramCommand, RunOutput};
use crate::testing::{TestBackend, TestModule};

/// The emitted C program in the output directory. Its name is implementation-defined
/// (`[target.native.output]`).
pub const C_ENTRY_FILE: &str = "main.c";
/// The executable the C compiler makes in the output directory (`[target.native.output]`).
pub const C_EXECUTABLE: &str = "main";
/// The environment variable that names the C compiler; `cc` on PATH otherwise. The whole value
/// names one program and is not split at whitespace (`[target.native.cc]`).
pub const CC_ENV: &str = "EMELA_CC";
/// The environment variable that holds flags for the C compiler, split at whitespace and passed
/// after every source file (`[target.native.cc]`).
pub const CC_ARGS_ENV: &str = "EMELA_CC_ARGS";

/// The flags every native build gets, before the sources. `-std=c11`: the C sources of the
/// project are guaranteed C11 or later (`[target.native.cc]`); a later `-std=` in
/// `EMELA_CC_ARGS` overrides it. `-fno-strict-aliasing`: the runtime reads static objects through
/// the type of a heap object. `-pthread`: completions may come from other threads (spec 12.6).
/// `-I.`: host C files include `"emela.h"` from the output directory.
const CC_FLAGS: [&str; 5] = ["-std=c11", "-O2", "-fno-strict-aliasing", "-pthread", "-I."];

/// Turns the Core IR into C and builds it with the C compiler (`emela build --target native`).
/// Host C files that external functions point to are compiled with it.
#[derive(Debug, Default, Clone)]
pub struct CoreC {
    /// The C sources to compile, relative to the output directory or absolute: set by `emit`.
    sources: Vec<PathBuf>,
    /// The C compiler and the flags passed after the sources; [`cc_command`] if `None`.
    cc: Option<(PathBuf, Vec<String>)>,
}

impl CoreC {
    /// Builds with the C compiler `cc` and the flags `flags`, passed where `EMELA_CC_ARGS` is,
    /// instead of [`cc_command`] (tests).
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

    fn output(&mut self, out: COutput) -> BuildOutput {
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
        BuildOutput {
            files,
            entry: C_ENTRY_FILE.into(),
        }
    }
}

impl Backend<emela_core::Module> for CoreC {
    fn emit(
        &mut self,
        module: &emela_core::Module,
        _: &Analysis,
    ) -> Result<BuildOutput, Vec<Diagnostic>> {
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
    ) -> Result<BuildOutput, Vec<Diagnostic>> {
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

/// emela-codegen-c's errors as diagnostics (spec `[target.native.unsupported]`). They have no location:
/// the Heap IR that codegen reads carries no source positions.
fn diagnostics(errors: Vec<CError>) -> Vec<Diagnostic> {
    errors
        .into_iter()
        .map(|e| {
            let d = Diagnostic::error(e.message);
            match e.kind {
                CErrorKind::Callback => d.with_code(code::CALLBACK_EXTERNAL),
                CErrorKind::BoundaryType => d.with_code(code::BOUNDARY_TYPE_ON_C),
                CErrorKind::Unsupported | CErrorKind::Internal => d,
            }
        })
        .collect()
}

/// The C compiler and the flags of `EMELA_CC_ARGS` (`[target.native.cc]`): `EMELA_CC` if it is
/// not only whitespace, as one program, otherwise `cc`.
pub(crate) fn cc_command_in(env: &dyn Fn(&str) -> Option<OsString>) -> (PathBuf, Vec<String>) {
    let program = env(CC_ENV)
        .filter(|v| !v.to_string_lossy().trim().is_empty())
        .map_or_else(|| PathBuf::from("cc"), PathBuf::from);
    let args = env(CC_ARGS_ENV)
        .map(|v| {
            v.to_string_lossy()
                .split_whitespace()
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    (program, args)
}

/// The C compiler and the flags of `EMELA_CC_ARGS`, from the environment of this process
/// ([`cc_command_in`]).
pub fn cc_command() -> (PathBuf, Vec<String>) {
    cc_command_in(&|key| std::env::var_os(key))
}

/// The command that compiles and links `sources` (relative to `dir`, or absolute) into the
/// executable `dir/main`, as every native build does (`[target.native.cc]`): the fixed flags,
/// the sources, the math library, and then `args` (`EMELA_CC_ARGS`), so that they take precedence
/// and a library given there is linked after the sources that use it. `emela doctor` compiles its
/// test program with the same command.
pub(crate) fn cc_invocation(
    cc: &Path,
    args: &[String],
    dir: &Path,
    sources: &[PathBuf],
) -> Command {
    let mut command = Command::new(cc);
    command
        .args(CC_FLAGS)
        .arg("-o")
        .arg(C_EXECUTABLE)
        .args(sources)
        .arg("-lm")
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null());
    command
}

/// Compiles `sources` (relative to `out_dir`, or absolute) with `cc` into the executable
/// `out_dir/main`, with `args` after the sources. E0907 if cc cannot start, E0908 if it fails
/// (with its messages as a note).
pub fn build_executable(
    cc: &Path,
    args: &[String],
    out_dir: &Path,
    sources: &[PathBuf],
) -> Result<PathBuf, Diagnostic> {
    let output = cc_invocation(cc, args, out_dir, sources)
        .output()
        .map_err(|err| cc_spawn_error(cc, &err))?;
    if !output.status.success() {
        let messages = String::from_utf8_lossy(&output.stderr);
        // The emitted files are implementation-defined, so the message names none of them
        // (`[target.native.cc]`).
        let mut d = Diagnostic::error(format!(
            "`{}` failed to compile the output of target `native`",
            cc.display(),
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

/// E0907 (`[target.native.cc]`). A program with whitespace in its name came from `EMELA_CC`, and
/// most likely carries flags, so the help points to `EMELA_CC_ARGS`.
fn cc_spawn_error(cc: &Path, err: &io::Error) -> Diagnostic {
    let d = Diagnostic::error(format!("cannot start `{}`: {err}", cc.display()))
        .with_code(code::CC_UNAVAILABLE);
    if has_whitespace(cc) {
        d.with_help(cc_args_help())
    } else if err.kind() == io::ErrorKind::NotFound {
        d.with_help(cc_missing_help())
    } else {
        d
    }
}

fn has_whitespace(program: &Path) -> bool {
    program.to_string_lossy().contains(char::is_whitespace)
}

/// The command that runs the executable `program` with `args`.
pub(crate) fn native_command(program: &Path, args: &[OsString]) -> ProgramCommand {
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::inherit());
    let program = program.to_path_buf();
    ProgramCommand::new(command, move |err| {
        Diagnostic::error(format!("cannot start `{}`: {err}", program.display()))
    })
}

/// Runs the executable `program` with `args`.
pub fn run_native(
    program: &Path,
    args: &[OsString],
    output: Output,
) -> Result<RunOutput, Diagnostic> {
    native_command(program, args).run(output)
}

pub(crate) fn cc_missing_help() -> String {
    format!("install a C11 compiler and add it to PATH as `cc`, or set {CC_ENV} to the compiler")
}

/// The help of E0907 when the value of `EMELA_CC` contains whitespace (`[target.native.cc]`).
pub(crate) fn cc_args_help() -> String {
    format!("{CC_ENV} names only the compiler; pass flags in {CC_ARGS_ENV}")
}

/// The help of E0907 for the program `cc`: [`cc_args_help`] when its name contains whitespace,
/// otherwise [`cc_missing_help`].
pub(crate) fn cc_unavailable_help(cc: &Path) -> String {
    if has_whitespace(cc) {
        cc_args_help()
    } else {
        cc_missing_help()
    }
}
