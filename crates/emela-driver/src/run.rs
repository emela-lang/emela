//! Runs the emitted JS on node.

use std::cell::{Cell, RefCell};
use std::ffi::OsString;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::code;
use crate::diagnostic::Diagnostic;

thread_local! {
    /// The working directory for the child runner in this thread (see [`with_program_cwd`]).
    static PROGRAM_CWD: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    /// The longest the child runner in this thread may run (see [`with_program_timeout`]).
    static PROGRAM_TIMEOUT: Cell<Option<Duration>> = const { Cell::new(None) };
}

/// Runs `f` with `dir` as the working directory of the node/wasmtime child it starts on this
/// thread. The test harnesses use it to give each example a directory of its own, so examples
/// that write files run in parallel and leave the source tree alone.
///
/// Only the child process changes directory; the driver's own filesystem stays where it was, so
/// a relative `entry` is made absolute before the child starts.
pub fn with_program_cwd<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<PathBuf>);
    impl Drop for Restore {
        fn drop(&mut self) {
            PROGRAM_CWD.with(|cwd| *cwd.borrow_mut() = self.0.take());
        }
    }

    let previous = PROGRAM_CWD.with(|cwd| cwd.replace(Some(dir.to_path_buf())));
    let _restore = Restore(previous);
    f()
}

pub(crate) fn apply_program_cwd(command: &mut Command) {
    PROGRAM_CWD.with(|cwd| {
        if let Some(dir) = cwd.borrow().as_ref() {
            command.current_dir(dir);
        }
    });
}

/// Runs `f` with `limit` as the longest time a node/wasmtime child started on this thread may
/// run. A child still running after `limit` is killed, and the run or test that started it fails
/// with a diagnostic saying so. The test harnesses use it so a program that hangs fails its own
/// test instead of hanging CI or leaving the child running.
pub fn with_program_timeout<T>(limit: Duration, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<Duration>);
    impl Drop for Restore {
        fn drop(&mut self) {
            PROGRAM_TIMEOUT.with(|timeout| timeout.set(self.0));
        }
    }

    let previous = PROGRAM_TIMEOUT.with(|timeout| timeout.replace(Some(limit)));
    let _restore = Restore(previous);
    f()
}

/// When a child started now must have finished (see [`with_program_timeout`]).
#[derive(Clone, Copy)]
pub(crate) struct Deadline {
    at: Instant,
    limit: Duration,
}

impl Deadline {
    /// The deadline of a child starting now, if this thread has a program timeout.
    pub(crate) fn start() -> Option<Deadline> {
        PROGRAM_TIMEOUT.with(Cell::get).map(|limit| Deadline {
            at: Instant::now() + limit,
            limit,
        })
    }

    pub(crate) fn remaining(self) -> Duration {
        self.at.saturating_duration_since(Instant::now())
    }

    /// Kills `child` for running past the deadline. The error is what the run reports.
    pub(crate) fn stop(self, child: &mut Child) -> io::Error {
        let _ = child.kill();
        let _ = child.wait();
        io::Error::new(
            io::ErrorKind::TimedOut,
            format!("did not finish within {:?} and was stopped", self.limit),
        )
    }
}

/// Waits for `child`, killing it at `deadline`.
pub(crate) fn wait_child(child: &mut Child, deadline: Option<Deadline>) -> io::Result<ExitStatus> {
    let Some(deadline) = deadline else {
        return child.wait();
    };
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if deadline.remaining().is_zero() {
            return Err(deadline.stop(child));
        }
        std::thread::sleep(Duration::from_millis(10).min(deadline.remaining()));
    }
}

/// Runs `command` to the end, through emela's stdio with [`Output::Inherit`] or capturing stdout
/// and stderr with [`Output::Capture`], within the program timeout if there is one.
fn run_child(command: &mut Command, output: Output) -> io::Result<RunOutput> {
    let Some(deadline) = Deadline::start() else {
        return match output {
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
    };
    if let Output::Capture = output {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
    }
    let mut child = command.spawn()?;
    // Read both pipes on other threads so a full pipe cannot stall the child.
    let stdout = child.stdout.take().map(read_to_end_on_thread);
    let stderr = child.stderr.take().map(read_to_end_on_thread);
    let status = wait_child(&mut child, Some(deadline))?;
    Ok(RunOutput {
        code: exit_code(status),
        stdout: join_reader(stdout),
        stderr: join_reader(stderr),
    })
}

/// Reads `pipe` to its end on another thread.
pub(crate) fn read_to_end_on_thread(mut pipe: impl Read + Send + 'static) -> JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        buf
    })
}

/// What [`read_to_end_on_thread`] read, or nothing if there was no pipe.
pub(crate) fn join_reader(reader: Option<JoinHandle<Vec<u8>>>) -> Vec<u8> {
    reader
        .map(|reader| reader.join().unwrap_or_default())
        .unwrap_or_default()
}

pub(crate) fn entry_for_child(entry: &Path) -> PathBuf {
    let has_cwd = PROGRAM_CWD.with(|cwd| cwd.borrow().is_some());
    if has_cwd && entry.is_relative() {
        std::env::current_dir()
            .map(|dir| dir.join(entry))
            .unwrap_or_else(|_| entry.to_path_buf())
    } else {
        entry.to_path_buf()
    }
}

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
///
/// The launcher does not import the runtime. So it tells a defect by its `name` ([`DEFECT_NAME`])
/// rather than `instanceof`, a failure by the [`FAIL_KEY`] symbol, and takes `$start` from the
/// entry, which exports it. Exceptions in host callbacks are caught with `uncaughtException`, and
/// an unresumed end with `beforeExit`. The exit code is set through `process.exitCode`, or
/// `process.exit` only in the callback of the last write, so that writes to a pipe are not cut
/// off.
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

/// The `wasmtime run` command for a WASI 0.3 component. Callers add stdin/args/output as needed.
///
/// The output is a component exporting `wasi:cli/run@0.3.0`; p3 is not enabled by every
/// wasmtime version, so the flag is explicit even where it is currently the default.
///
/// The program's work directory is preopened (the `--dir` equivalent), so `Std.FileSystem` sees
/// the same directory the JS and C runs use. The path is the child's work directory (see
/// [`with_program_cwd`]) or `.` when the driver did not set one; the guest resolves relative
/// paths against the preopen, which is what the JS and C handlers do against the process cwd.
///
/// The preopen is the sandbox root, not a view of the host's `/`: the guest can name only
/// paths below it, and an absolute path or a `..` step that leaves it fails with WASI's
/// `not-permitted` (reported by `Std.FileSystem` as `FileAccessDenied`). The directory is
/// preopened for every wasm run because `Std.Platform` constructs `SystemFileSystem` even when
/// the program uses only `Console`; a component that does not import the file system cannot
/// reach it.
pub(crate) fn wasmtime_run_command(wasmtime: &Path, entry: &Path) -> Command {
    let mut command = Command::new(wasmtime);
    command.arg("run").arg("-S").arg("p3=y");
    // The child's work directory is the root `Std.FileSystem` resolves relative paths
    // against. It is passed as an absolute path (see `preopen_dir`) so changing the child's
    // directory cannot resolve it a second time.
    command.arg("--dir").arg(preopen_dir());
    command.arg(entry_for_child(entry));
    apply_program_cwd(&mut command);
    command
}

/// The oldest wasmtime major version that runs WASI 0.3 components with `-S p3=y`.
pub(crate) const WASMTIME_MIN_MAJOR: u32 = 49;

/// E0909 when `wasmtime --version` reports a version older than [`WASMTIME_MIN_MAJOR`], which
/// would reject `-S p3=y` or the component with an unrelated error. Nothing is reported when the
/// version cannot be read: a wasmtime that does not start is reported by the run itself.
pub(crate) fn check_wasmtime_version(wasmtime: &Path) -> Result<(), Diagnostic> {
    let Ok(out) = Command::new(wasmtime).arg("--version").output() else {
        return Ok(());
    };
    let version = String::from_utf8_lossy(&out.stdout);
    match wasmtime_major(&version) {
        Some(major) if major < WASMTIME_MIN_MAJOR => Err(Diagnostic::error(format!(
            "`{}` is {}, but target `wasm` needs wasmtime {WASMTIME_MIN_MAJOR} or later",
            wasmtime.display(),
            version.trim()
        ))
        .with_code(code::WASMTIME_UNAVAILABLE)
        .with_note(format!(
            "install Wasmtime {WASMTIME_MIN_MAJOR} or later, or set {WASMTIME_ENV} to a newer wasmtime executable"
        ))),
        _ => Ok(()),
    }
}

/// The major version in the output of `wasmtime --version` (`wasmtime 49.0.3 (...)`, or
/// `wasmtime-cli 20.0.0` for older releases).
fn wasmtime_major(version: &str) -> Option<u32> {
    let number = version.split_whitespace().nth(1)?;
    number.split('.').next()?.parse().ok()
}

/// The work directory to preopen. It is made absolute before the child changes directory, so a
/// relative [`with_program_cwd`] directory is not resolved a second time inside it.
fn preopen_dir() -> PathBuf {
    let dir = program_cwd().unwrap_or_else(|| PathBuf::from("."));
    if dir.is_relative() {
        std::env::current_dir()
            .map(|cwd| cwd.join(&dir))
            .unwrap_or(dir)
    } else {
        dir
    }
}

/// The work directory [`with_program_cwd`] set for this thread, if any.
fn program_cwd() -> Option<PathBuf> {
    PROGRAM_CWD.with(|cwd| cwd.borrow().clone())
}

/// A program ready to start: the command, and the diagnostic for failing to start it.
pub struct ProgramCommand {
    command: Command,
    spawn_error: Box<dyn Fn(&io::Error) -> Diagnostic>,
}

impl ProgramCommand {
    pub(crate) fn new(
        command: Command,
        spawn_error: impl Fn(&io::Error) -> Diagnostic + 'static,
    ) -> ProgramCommand {
        ProgramCommand {
            command,
            spawn_error: Box::new(spawn_error),
        }
    }

    /// The command as one line, quoted for a POSIX shell, that `emela run` shows before starting
    /// the program (spec `[cmd.build]`). node's `-e` launcher is shown as `<launcher>`: it is a
    /// whole module, and the run passes it inline rather than from a file.
    pub fn command_line(&self) -> String {
        std::iter::once(self.command.get_program())
            .chain(self.command.get_args())
            .map(|arg| {
                if arg == LAUNCHER {
                    "<launcher>".to_string()
                } else {
                    shell_word(&arg.to_string_lossy())
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Runs the program to the end (see [`run_child`]).
    pub(crate) fn run(mut self, output: Output) -> Result<RunOutput, Diagnostic> {
        run_child(&mut self.command, output).map_err(|err| (self.spawn_error)(&err))
    }
}

/// `word` as it would be typed in a POSIX shell: as is when it has no characters the shell
/// treats specially, otherwise in single quotes.
fn shell_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./=:@%+,".contains(&b));
    if plain {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// The command that runs the `.wasm` component `entry` with `wasmtime` (WASI 0.3). `args` are
/// passed to the program. E0909 if the wasmtime is too old.
pub(crate) fn wasmtime_command(
    entry: &Path,
    args: &[OsString],
) -> Result<ProgramCommand, Diagnostic> {
    let wasmtime = wasmtime_program();
    check_wasmtime_version(&wasmtime)?;
    let mut command = wasmtime_run_command(&wasmtime, entry);
    command.args(args).stdin(Stdio::inherit());
    Ok(ProgramCommand::new(command, move |err| {
        wasmtime_spawn_error(&wasmtime, err)
    }))
}

/// Runs the `.wasm` component `entry` with `wasmtime` (WASI 0.3). `args` are passed to the
/// program.
pub fn run_wasmtime(
    entry: &Path,
    args: &[OsString],
    output: Output,
) -> Result<RunOutput, Diagnostic> {
    wasmtime_command(entry, args)?.run(output)
}

fn wasmtime_spawn_error(wasmtime: &Path, err: &io::Error) -> Diagnostic {
    match err.kind() {
        io::ErrorKind::TimedOut => Diagnostic::error(format!("`{}` {err}", wasmtime.display())),
        io::ErrorKind::NotFound => Diagnostic::error(format!("`{}` not found", wasmtime.display()))
            .with_code(code::WASMTIME_UNAVAILABLE)
            .with_note(format!(
                "install Wasmtime {WASMTIME_MIN_MAJOR} or later and add it to PATH, or set {WASMTIME_ENV} to the wasmtime executable"
            )),
        _ => Diagnostic::error(format!("cannot start `{}`: {err}", wasmtime.display()))
            .with_code(code::WASMTIME_UNAVAILABLE),
    }
}

/// The command that runs the `.mjs` `entry` on node. `args` are passed to the program.
pub(crate) fn node_command(node: &Path, entry: &Path, args: &[OsString]) -> ProgramCommand {
    let mut command = Command::new(node);
    command
        .arg("--input-type=module")
        .arg("-e")
        .arg(LAUNCHER)
        .arg("--")
        .arg(entry_for_child(entry))
        .args(args)
        .stdin(Stdio::inherit());
    apply_program_cwd(&mut command);
    let node = node.to_path_buf();
    ProgramCommand::new(command, move |err| spawn_error(&node, err))
}

/// The command that runs the script `script` on node as it is (`node serve.mjs <args...>`), for
/// the server of an HTTP service (`[cmd.build]`).
pub(crate) fn node_script_command(node: &Path, script: &Path, args: &[OsString]) -> ProgramCommand {
    let mut command = Command::new(node);
    command
        .arg(entry_for_child(script))
        .args(args)
        .stdin(Stdio::inherit());
    apply_program_cwd(&mut command);
    let node = node.to_path_buf();
    ProgramCommand::new(command, move |err| spawn_error(&node, err))
}

/// Runs the `.mjs` `entry` on node. `args` are passed to the program.
pub fn run_node(
    node: &Path,
    entry: &Path,
    args: &[OsString],
    output: Output,
) -> Result<RunOutput, Diagnostic> {
    node_command(node, entry, args).run(output)
}

/// A diagnostic about failing to start node.
pub(crate) fn spawn_error(node: &Path, err: &io::Error) -> Diagnostic {
    match err.kind() {
        io::ErrorKind::TimedOut => Diagnostic::error(format!("`{}` {err}", node.display())),
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
    fn command_line_shows_the_launcher_by_name() {
        let command = node_command(
            Path::new("node"),
            Path::new("target/emela/js/main.mjs"),
            &["a b".into(), "it's".into(), "--x=1".into()],
        );
        assert_eq!(
            command.command_line(),
            r#"node --input-type=module -e <launcher> -- target/emela/js/main.mjs 'a b' 'it'\''s' --x=1"#
        );
    }

    #[test]
    fn shell_words() {
        assert_eq!(shell_word("main.wasm"), "main.wasm");
        assert_eq!(shell_word("p3=y"), "p3=y");
        assert_eq!(shell_word(""), "''");
        assert_eq!(shell_word("$HOME"), "'$HOME'");
    }

    #[test]
    fn wasmtime_versions() {
        assert_eq!(
            wasmtime_major("wasmtime 49.0.3 (a1b2c3d 2026-01-20)\n"),
            Some(49)
        );
        assert_eq!(wasmtime_major("wasmtime-cli 20.0.0\n"), Some(20));
        assert_eq!(wasmtime_major("wasmtime\n"), None);
        assert_eq!(wasmtime_major(""), None);
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

    #[cfg(unix)]
    #[test]
    fn program_timeout_kills_the_child() {
        let started = Instant::now();
        let err = with_program_timeout(Duration::from_millis(100), || {
            run_child(Command::new("sleep").arg("30"), Output::Capture)
        })
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[cfg(unix)]
    #[test]
    fn program_timeout_keeps_the_output_of_a_child_that_finishes() {
        let out = with_program_timeout(Duration::from_secs(30), || {
            run_child(
                Command::new("sh").args(["-c", "echo out; echo err >&2; exit 3"]),
                Output::Capture,
            )
        })
        .unwrap();
        assert_eq!(out.code, 3);
        assert_eq!(out.stdout, b"out\n");
        assert_eq!(out.stderr, b"err\n");
    }
}
