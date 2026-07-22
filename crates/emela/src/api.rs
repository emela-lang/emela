//! Filesystem-free, string-based entry points for embedding the Emela
//! compiler in-process — for example the WebAssembly playground that runs the
//! compiler in the browser.
//!
//! These mirror the `check`, `ir`, and `build` CLI commands but take the source
//! as a string and never touch the filesystem. The embedded std modules (spec
//! 0038) resolve as usual — `import std.io` works with no filesystem — but
//! there is no package search path, so any other `import` fails to resolve;
//! everything else compiles exactly as the CLI would.

use emela_codegen::{Artifact, EmitMode};

use crate::driver;
use crate::error::Result;

/// Type-checks `source`, returning `Ok(())` when it is well-typed.
///
/// `label` is the name shown in diagnostics (e.g. `"playground.emel"`).
pub fn check_source(label: &str, source: &str) -> Result<()> {
    driver::check_source(label, source)
}

/// Lowers `source` to the codegen IR and renders it as text.
pub fn ir_source(label: &str, source: &str) -> Result<String> {
    driver::ir_source(label, source)
}

/// Compiles `source` with the named built-in backend (e.g. `"js-node"`,
/// `"wasm-wasi"`). `mode` selects the primary artifact or its textual form
/// (e.g. WAT for the wasm backend).
pub fn compile_source(
    label: &str,
    source: &str,
    backend: &str,
    mode: EmitMode,
) -> Result<Artifact> {
    driver::compile_source(label, source, backend, mode)
}

/// The captured result of compiling and running `source` in-process — the
/// playground's "Run" button. Mirrors `emela run`: the source is compiled with
/// the `wasm-wasi` backend and executed via the embedded `wasmi` interpreter,
/// with stdout/stderr captured into strings instead of written to the process
/// streams (so it works inside a wasm build, where there are no process
/// streams).
#[cfg(feature = "run")]
#[derive(Debug, Default)]
pub struct RunOutput {
    /// Everything the program wrote to stdout.
    pub stdout: String,
    /// Everything the program wrote to stderr.
    pub stderr: String,
    /// The process exit code, when the program exited cleanly via `proc_exit`.
    pub exit_code: Option<i32>,
    /// A runtime trap message (`panic`/`unreachable`), when execution trapped
    /// instead of exiting cleanly.
    pub trap: Option<String>,
}

/// Compiles `source` with the `wasm-wasi` backend and runs it in-process,
/// returning its captured output.
///
/// A compile or type error surfaces as `Err`; a program that runs but traps
/// returns `Ok` with [`RunOutput::trap`] set. Networking (sockets / HTTP) is
/// linked but non-functional under a `wasm32-unknown-unknown` host, so programs
/// that use it trap at run time.
#[cfg(feature = "run")]
pub fn run_source(label: &str, source: &str) -> Result<RunOutput> {
    use crate::run::{Captured, RunOutcome};

    let artifact = driver::compile_source(label, source, "wasm-wasi", EmitMode::Default)?;
    let (outcome, Captured { stdout, stderr }) = crate::run::execute_captured(&artifact.bytes)?;
    let (exit_code, trap) = match outcome {
        RunOutcome::Exit(code) => (Some(code), None),
        RunOutcome::Trap(message) => (None, Some(message)),
    };
    Ok(RunOutput {
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        exit_code,
        trap,
    })
}
