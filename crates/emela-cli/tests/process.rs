//! `Std.Process` (`[api.std.process]`) through `emela test` and `emela run` on each target: the
//! arguments after `--`, standard input, and what both give under `emela test`. The programs are
//! in `process/`.
//!
//! JS always runs. C runs when the C compiler starts (`EMELA_CC` or `cc`), and WASM when wasmtime
//! 49 or later starts (`EMELA_WASMTIME` or `wasmtime`); otherwise they are skipped with a message.
//! SystemStdin is not available on WASM yet (`[target.wasm.external]`), so standard input is
//! checked on JS and C.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// A project with `src/main.emel`, removed on drop.
struct Project {
    dir: PathBuf,
}

impl Project {
    fn new(name: &str, main: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "emela-cli-process-{}-{name}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("Pome.toml"), "").unwrap();
        std::fs::write(dir.join("src/main.emel"), main).unwrap();
        Project { dir }
    }

    fn emela(&self, args: &[&std::ffi::OsStr]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_emela"));
        for var in emela_driver::AGENT_ENV_VARS {
            command.env_remove(var);
        }
        command
            .args(args)
            .current_dir(&self.dir)
            .env("NO_COLOR", "1");
        command
    }

    /// `emela <args>` with standard input `input`.
    fn run(&self, args: &[&str], input: &[u8]) -> Output {
        let args: Vec<&std::ffi::OsStr> = args.iter().map(std::ffi::OsStr::new).collect();
        self.run_os(&args, input)
    }

    fn run_os(&self, args: &[&std::ffi::OsStr], input: &[u8]) -> Output {
        let mut child = self
            .emela(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        child.wait_with_output().unwrap()
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn have_cc() -> bool {
    let (cc, flags) = emela_driver::cc_command();
    let ok = Command::new(&cc)
        .args(&flags)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!("skipping C: cannot start the C compiler `{}`", cc.display());
    }
    ok
}

fn have_wasmtime() -> bool {
    let wasmtime = emela_driver::wasmtime_program();
    let major = Command::new(&wasmtime)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            text(&o.stdout)
                .split_whitespace()
                .find(|w| w.starts_with(|c: char| c.is_ascii_digit()))
                .and_then(|w| w.split('.').next()?.parse::<u32>().ok())
        });
    let ok = major.is_some_and(|m| m >= 49);
    if !ok {
        eprintln!(
            "skipping WASM: `{}` is not wasmtime 49 or later",
            wasmtime.display()
        );
    }
    ok
}

/// JS, and C when cc starts.
fn js_and_c() -> Vec<&'static str> {
    let mut targets = vec!["js"];
    if have_cc() {
        targets.push("c");
    }
    targets
}

/// JS, C when cc starts, and WASM when wasmtime starts.
fn every_target() -> Vec<&'static str> {
    let mut targets = js_and_c();
    if have_wasmtime() {
        targets.push("wasm");
    }
    targets
}

fn assert_tests_pass(name: &str, source: &str, targets: &[&str]) {
    let project = Project::new(name, source);
    for target in targets {
        let out = project.run(&["test", "--target", target], b"");
        let stdout = text(&out.stdout);
        assert!(
            out.status.success() && stdout.contains(" 0 failed"),
            "{target}: {stdout}{}",
            text(&out.stderr)
        );
    }
}

#[test]
fn test_stdin_and_args_under_test_on_every_target() {
    assert_tests_pass(
        "test-stdin",
        include_str!("process/test_stdin.emel"),
        &every_target(),
    );
}

#[test]
fn system_stdin_reads_an_empty_input_under_test() {
    assert_tests_pass(
        "system-stdin",
        include_str!("process/system_stdin.emel"),
        &js_and_c(),
    );
}

#[test]
fn args_are_the_arguments_after_double_dash() {
    let project = Project::new("args", include_str!("process/args.emel"));
    for target in every_target() {
        let out = project.run(&["run", "--target", target, "--", "a", "b c", ""], b"");
        assert_eq!(
            (out.status.code(), text(&out.stdout)),
            (
                Some(0),
                "[\"a\", \"b c\", \"\"]\nTrue\n[Some(\"a\"), Some(\"\"), None, None]\n".into()
            ),
            "{target}: {}",
            text(&out.stderr)
        );
        let out = project.run(&["run", "--target", target], b"");
        assert_eq!(
            text(&out.stdout),
            "[]\nTrue\n[None, None, None, None]\n",
            "{target}"
        );
    }
}

/// Invalid UTF-8 in an argument becomes U+FFFD. wasmtime cannot pass such an argument, so WASM
/// is not checked.
#[cfg(unix)]
#[test]
fn invalid_utf8_in_an_argument_is_replaced() {
    use std::os::unix::ffi::OsStrExt;

    let project = Project::new("args-invalid", include_str!("process/args.emel"));
    let arg = std::ffi::OsStr::from_bytes(b"\xffx\xe3\x81");
    for target in js_and_c() {
        let args = ["run", "--target", target, "--"].map(std::ffi::OsStr::new);
        let mut args = args.to_vec();
        args.push(arg);
        let out = project.run_os(&args, b"");
        let first = text(&out.stdout).lines().next().map(str::to_owned);
        assert_eq!(
            first.as_deref(),
            Some("[\"\u{fffd}x\u{fffd}\"]"),
            "{target}: {}",
            text(&out.stderr)
        );
    }
}

#[test]
fn quote_reads_each_line() {
    let project = Project::new("quote", include_str!("process/quote.emel"));
    for target in js_and_c() {
        let out = project.run(&["run", "--target", target, "--", "# "], b"x\r\ny\n\nlast");
        assert_eq!(
            (out.status.code(), text(&out.stdout)),
            (Some(0), "# x\n# y\n# \n# last\n".into()),
            "{target}: {}",
            text(&out.stderr)
        );
        let out = project.run(&["run", "--target", target], b"");
        assert_eq!(text(&out.stdout), "", "{target}");
    }
}

/// Waits for `child` up to `limit`; kills it and fails when it is still running.
fn wait(mut child: Child, limit: Duration, what: &str) -> Output {
    let start = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > limit {
            child.kill().unwrap();
            panic!("{what} is still running after {limit:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child.wait_with_output().unwrap()
}

/// Line terminators, invalid UTF-8, U+0000, `read_text` after `read_line`, the end of the input,
/// and no input lost when the first read is interrupted while it waits.
#[test]
fn stdin_lines_text_and_interruption() {
    let project = Project::new("stdin", include_str!("process/stdin.emel"));
    for target in js_and_c() {
        let mut child = project
            .emela(&["run", "--target", target].map(std::ffi::OsStr::new))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // The input is written only after the first read has timed out.
        let mut stderr = BufReader::new(child.stderr.take().unwrap());
        let mut seen = String::new();
        loop {
            let mut line = String::new();
            if stderr.read_line(&mut line).unwrap() == 0 {
                panic!("{target}: no `waiting` on standard error: {seen}");
            }
            seen.push_str(&line);
            if line == "waiting\n" {
                break;
            }
        }
        let mut stdin = child.stdin.take().unwrap();
        stdin
            .write_all(b"one\nbad\xffline\nn\x00ul\r\ntwo\nrest\r\nmore")
            .unwrap();
        drop(stdin);
        let out = wait(child, Duration::from_secs(60), target);
        let mut rest = String::new();
        stderr.read_to_string(&mut rest).unwrap();
        assert_eq!(
            (out.status.code(), text(&out.stdout)),
            (
                Some(0),
                [
                    "None",
                    "[111, 110, 101]",
                    "InvalidInput",
                    "[110, 0, 117, 108]",
                    "[116, 119, 111]",
                    "[114, 101, 115, 116, 13, 10, 109, 111, 114, 101]",
                    "None",
                    "True",
                    "",
                ]
                .join("\n")
            ),
            "{target}: {seen}{rest}"
        );
    }
}

/// A program whose reads are done ends with its main, although standard input is still open.
#[test]
fn open_stdin_does_not_keep_the_program_running() {
    let project = Project::new("open-stdin", include_str!("process/open_stdin.emel"));
    for target in js_and_c() {
        // Built first, so that the limit below is about the program and not the compiler.
        let built = project.run(&["build", "--target", target], b"");
        assert!(built.status.success(), "{target}: {}", text(&built.stderr));
        let mut child = project
            .emela(&["run", "--target", target].map(std::ffi::OsStr::new))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(b"first\nsecond\n").unwrap();
        // `stdin` stays open until the program has ended.
        let out = wait(child, Duration::from_secs(60), target);
        drop(stdin);
        assert_eq!(
            (out.status.code(), text(&out.stdout)),
            (Some(0), "Some(\"first\")\n".into()),
            "{target}: {}",
            text(&out.stderr)
        );
    }
}
