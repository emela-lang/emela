//! Runs the emitted JS on node.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use crate::code;
use crate::diagnostic::Diagnostic;

/// An environment variable overriding the node executable; `node` on PATH otherwise.
pub const NODE_ENV: &str = "EMELA_NODE";
/// The `name` of the defect class, matching `$Defect` in emela-codegen-js's runtime.
pub const DEFECT_NAME: &str = "Defect";
/// The exit code when stopped by a defect.
pub const DEFECT_EXIT_CODE: i32 = 101;
/// The exit code when stopped by an error that reached main (spec 14.3).
pub const ERROR_EXIT_CODE: i32 = 1;
/// The exit code when main's continuation unwound while suspended (spec 14.5).
pub const INTERRUPTED_EXIT_CODE: i32 = 130;
/// The `Symbol.for` key identifying failure values (`$Fail` in emela-codegen-js's runtime). The
/// key's function returns the error value shown.
pub const FAIL_KEY: &str = "emela.fail";

/// A small launcher module that imports the entry, calls `main`, and maps errors reaching main
/// and defects to exit codes. Passed to node's `-e`, so it is never written to a file.
///
/// If the entry does not export `main`, it is only imported (for JS that runs on import). If
/// `main` returns a failure (`$Fail`), it prints `error: <the error shown>` and exits with 1;
/// any other return value is discarded (spec 14.3).
///
/// A suspending main (one returning a generator) is driven by the runtime's `$start`, which the
/// entry exports, and waits on the event loop. If the loop empties without a resume, the function
/// `$start` returned runs the releases of open `with`s from the inside out, then it prints
/// `interrupted` and exits with 130 (spec 14.5). If a release stays suspended, it is called again
/// the next time the loop empties. A continuation dropped without a resume inside a
/// non-suspending main (the runtime's `$Interrupted`) is handled the same way. main is the root
/// fiber of the run, and only the root fiber's outcome reaches `$start`'s `done`. A child fiber's
/// interruption (spec 11.4, including dropped continuations) is received by its scope as that
/// child's Interrupt and never gets here (spec 11.5). Unwinding when the loop empties includes
/// child fibers. A defect, including one inside a host callback, prints `defect: <message>` and
/// exits with 101 (spec 7.5).
const LAUNCHER: &str = r#"
import { pathToFileURL } from "node:url";
let over = false;
function finish(value) {
  over = true;
  const show = value?.[Symbol.for("emela.fail")];
  if (typeof show === "function") {
    process.stderr.write(`error: ${show.call(value)}\n`);
    process.exitCode = 1;
  }
}
function crash(error) {
  if (over) return;
  over = true;
  if (error instanceof Error && error.name === "Interrupted") {
    process.exitCode = 130;
    process.stderr.write("interrupted\n", () => process.exit(130));
  } else if (error instanceof Error && error.name === "Defect") {
    process.exitCode = 101;
    process.stderr.write(`defect: ${error.message}\n`, () => process.exit(101));
  } else {
    process.exitCode = 1;
    process.stderr.write(`${error?.stack ?? String(error)}\n`, () => process.exit(1));
  }
}
let cancel;
process.on("uncaughtException", crash);
process.on("beforeExit", () => {
  if (over) return;
  if (cancel !== undefined) {
    cancel();
    return;
  }
  over = true;
  process.stderr.write("interrupted\n");
  process.exitCode = 130;
});
try {
  const entry = await import(pathToFileURL(process.argv[1]).href);
  if (typeof entry.main === "function") {
    const value = entry.main();
    if (typeof entry.$start === "function" && Object.prototype.toString.call(value) === "[object Generator]") {
      cancel = entry.$start(value, (ok, result) => (ok ? finish(result) : crash(result)));
    } else {
      finish(value);
    }
  } else {
    over = true;
  }
} catch (error) {
  crash(error);
}
"#;

/// How the child process's stdio is handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    /// Passes emela's stdio through (the CLI).
    Inherit,
    /// Captures stdout and stderr (tests).
    Capture,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput {
    /// node's exit code; 128 plus the signal number if stopped by a signal.
    pub code: i32,
    /// Filled only with [`Output::Capture`].
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// `EMELA_NODE` if set, otherwise `node`.
pub fn node_program() -> PathBuf {
    std::env::var_os(NODE_ENV)
        .filter(|v| !v.is_empty())
        .map_or_else(|| PathBuf::from("node"), PathBuf::from)
}

/// An environment variable overriding the `wasmtime` executable; `wasmtime` on PATH otherwise.
pub const WASMTIME_ENV: &str = "EMELA_WASMTIME";

/// `EMELA_WASMTIME` if set, otherwise `wasmtime`.
pub fn wasmtime_program() -> PathBuf {
    std::env::var_os(WASMTIME_ENV)
        .filter(|v| !v.is_empty())
        .map_or_else(|| PathBuf::from("wasmtime"), PathBuf::from)
}

/// Runs the `.wasm` `entry` with `wasmtime` (WASI). `args` are passed to the program.
pub fn run_wasmtime(
    entry: &Path,
    args: &[OsString],
    output: Output,
) -> Result<RunOutput, Diagnostic> {
    let wasmtime = wasmtime_program();
    let mut command = Command::new(&wasmtime);
    command
        .arg("run")
        .arg(entry)
        .args(args)
        .stdin(Stdio::inherit());
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
    result.map_err(|err| wasmtime_spawn_error(&wasmtime, &err))
}

fn wasmtime_spawn_error(wasmtime: &Path, err: &io::Error) -> Diagnostic {
    match err.kind() {
        io::ErrorKind::NotFound => Diagnostic::error(format!("`{}` not found", wasmtime.display()))
            .with_code(code::WASMTIME_UNAVAILABLE)
            .with_note(format!(
                "install Wasmtime and add it to PATH, or set {WASMTIME_ENV} to the wasmtime executable"
            )),
        _ => Diagnostic::error(format!("cannot start `{}`: {err}", wasmtime.display()))
            .with_code(code::WASMTIME_UNAVAILABLE),
    }
}

/// Runs the `.mjs` `entry` on node. `args` are passed to the program.
pub fn run_node(
    node: &Path,
    entry: &Path,
    args: &[OsString],
    output: Output,
) -> Result<RunOutput, Diagnostic> {
    let mut command = Command::new(node);
    command
        .arg("--input-type=module")
        .arg("-e")
        .arg(LAUNCHER)
        .arg("--")
        .arg(entry)
        .args(args)
        .stdin(Stdio::inherit());
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
    result.map_err(|err| spawn_error(node, &err))
}

/// A diagnostic about failing to start node.
pub(crate) fn spawn_error(node: &Path, err: &io::Error) -> Diagnostic {
    match err.kind() {
        io::ErrorKind::NotFound => Diagnostic::error(format!("`{}` not found", node.display()))
            .with_code(code::NODE_UNAVAILABLE)
            .with_note(format!(
                "install Node.js and add it to PATH, or set {NODE_ENV} to the node executable"
            )),
        _ => Diagnostic::error(format!("cannot start `{}`: {err}", node.display()))
            .with_code(code::NODE_UNAVAILABLE),
    }
}

pub(crate) fn exit_code(status: ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_uses_constants() {
        assert!(LAUNCHER.contains(&format!("\"{DEFECT_NAME}\"")));
        assert!(LAUNCHER.contains(&format!("process.exitCode = {DEFECT_EXIT_CODE};")));
        assert!(LAUNCHER.contains(&format!("process.exitCode = {ERROR_EXIT_CODE};")));
        assert!(LAUNCHER.contains(&format!("process.exitCode = {INTERRUPTED_EXIT_CODE};")));
        assert!(LAUNCHER.contains(&format!("Symbol.for(\"{FAIL_KEY}\")")));
        // The runtime's `$Fail` and the test runner use the same key.
        let key = format!("Symbol.for(\"{FAIL_KEY}\")");
        assert!(emela_codegen_js::RUNTIME.contains(&key));
        assert!(emela_codegen_js::TEST_RUNNER.contains(&key));
    }

    #[test]
    fn missing_node() {
        let err = run_node(
            Path::new("/nonexistent/emela-test-node"),
            Path::new("main.mjs"),
            &[],
            Output::Capture,
        )
        .unwrap_err();
        assert_eq!(err.message, "`/nonexistent/emela-test-node` not found");
        assert_eq!(err.notes.len(), 1);
    }
}
