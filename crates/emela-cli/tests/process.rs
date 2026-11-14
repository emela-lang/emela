//! `Std.Process` (`[api.std.process]`) through `emela test` and `emela run` on each target: the
//! arguments after `--`, standard input, and what both give under `emela test`. The programs are
//! in `process/`.
//!
//! JS always runs. C runs when the C compiler starts (`EMELA_CC` or `cc`), and WASM when wasmtime
//! 49 or later starts (`EMELA_WASMTIME` or `wasmtime`); otherwise they are skipped with a message.
//! SystemStdin is not available on WASM yet (`[target.wasm.external]`), so standard input is
//! checked on JS and C. Stdout and Stderr are checked on every target, except a closed standard
//! output, which is checked on JS and C.

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

/// JS, and native when cc starts.
fn js_and_native() -> Vec<&'static str> {
    let mut targets = vec!["js"];
    if have_cc() {
        targets.push("native");
    }
    targets
}

/// JS, native when cc starts, and WASM when wasmtime starts.
fn every_target() -> Vec<&'static str> {
    let mut targets = js_and_native();
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
        &js_and_native(),
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
    for target in js_and_native() {
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
    for target in js_and_native() {
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
    for target in js_and_native() {
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
    for target in js_and_native() {
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

// ---- Stdout and Stderr ----

/// What the program wrote to standard error, without the `running` line of `emela run`
/// (`[cmd.build]`).
fn program_stderr(stderr: &[u8]) -> String {
    text(stderr)
        .split_inclusive('\n')
        .filter(|line| !line.starts_with("running `"))
        .collect()
}

#[test]
fn test_stdout_and_test_stderr_on_every_target() {
    assert_tests_pass(
        "test-stdout",
        include_str!("process/test_stdout.emel"),
        &every_target(),
    );
}

/// `print` writes the text as is and `print_line` adds `\n`, with no conversion of line endings,
/// through `Platform` and in a function that is not a suspend fn.
#[test]
fn system_stdout_and_stderr_on_every_target() {
    let project = Project::new("stdout", include_str!("process/stdout.emel"));
    for target in every_target() {
        let out = project.run(&["run", "--target", target], b"");
        assert_eq!(
            (
                out.status.code(),
                text(&out.stdout),
                program_stderr(&out.stderr)
            ),
            (
                Some(0),
                "Hello, alice!\na\r\nb\n\nno newline".into(),
                "e1 e2\ne3\r\n".into()
            ),
            "{target}"
        );
    }
}

/// The lines `interleave.emel` writes for `n` (5000): to standard output, to standard error, and to both
/// when they are the same pipe.
fn interleaved(n: usize) -> (String, String, String) {
    let (mut out, mut err, mut both) = (String::new(), String::new(), String::new());
    for i in 0..n {
        let o = format!("out {i}\nconsole {i}\n");
        out.push_str(&o);
        both.push_str(&o);
        if i % 100 == 0 {
            let e = format!("err {i}\nconsole error {i}\n{i}\n");
            err.push_str(&e);
            both.push_str(&e);
        }
    }
    (out, err, both)
}

/// Writes mixed between Console, Stdout, Stderr and `dbg` keep their order, also when standard
/// output and standard error are one pipe and its reader is slow (more is written than a pipe
/// holds, so the writer has to wait).
#[test]
fn mixed_writes_keep_their_order() {
    const N: usize = 5000;
    let project = Project::new("interleave", include_str!("process/interleave.emel"));
    let (out, err, both) = interleaved(N);
    for target in every_target() {
        let built = project.run(&["build", "--target", target], b"");
        assert!(built.status.success(), "{target}: {}", text(&built.stderr));

        let separate = project.run(&["run", "--target", target], b"");
        assert_eq!(
            (
                separate.status.code(),
                text(&separate.stdout),
                program_stderr(&separate.stderr)
            ),
            (Some(0), out.clone(), err.clone()),
            "{target}"
        );

        let (mut reader, writer) = std::io::pipe().unwrap();
        let mut child = project
            .emela(&["run", "--target", target].map(std::ffi::OsStr::new))
            .stdin(Stdio::null())
            .stdout(writer.try_clone().unwrap())
            .stderr(writer)
            .spawn()
            .unwrap();
        // The command holds the only writing ends, so the read ends when it has ended. The reader
        // starts late, so that the program fills the pipe and has to wait.
        std::thread::sleep(Duration::from_millis(300));
        let mut all = Vec::new();
        reader.read_to_end(&mut all).unwrap();
        let status = child.wait().unwrap();
        assert_eq!(
            (status.code(), program_stderr(&all)),
            (Some(0), both.clone()),
            "{target}: 2>&1"
        );
    }
}

/// Reads `from` on another thread, sending each chunk read.
fn chunks(mut from: impl Read + Send + 'static) -> std::sync::mpsc::Receiver<Vec<u8>> {
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match from.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) if send.send(buf[..n].to_vec()).is_err() => break,
                Ok(_) => {}
            }
        }
    });
    receive
}

/// Receives chunks until what was received ends with `end`, or fails after a minute.
fn receive_until(chunks: &std::sync::mpsc::Receiver<Vec<u8>>, end: &str, what: &str) -> String {
    let mut seen = Vec::new();
    while !seen.ends_with(end.as_bytes()) {
        match chunks.recv_timeout(Duration::from_secs(60)) {
            Ok(chunk) => seen.extend(chunk),
            Err(_) => panic!("{what}: no `{end}`: {}", text(&seen)),
        }
    }
    text(&seen)
}

/// What is written with `print` reaches the host before SystemStdin waits for input.
#[test]
fn a_prompt_is_written_before_reading() {
    let project = Project::new("prompt", include_str!("process/prompt.emel"));
    for target in js_and_native() {
        let built = project.run(&["build", "--target", target], b"");
        assert!(built.status.success(), "{target}: {}", text(&built.stderr));
        let mut child = project
            .emela(&["run", "--target", target].map(std::ffi::OsStr::new))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = chunks(child.stdout.take().unwrap());
        // The input is written only once the prompt has been read.
        assert_eq!(receive_until(&stdout, "name? ", target), "name? ");
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(b"bob\n").unwrap();
        drop(stdin);
        let rest = receive_until(&stdout, "\n", target);
        let out = wait(child, Duration::from_secs(60), target);
        assert_eq!(
            (out.status.code(), rest),
            (Some(0), "Hello, bob!\n".into()),
            "{target}: {}",
            text(&out.stderr)
        );
    }
}

/// Runs `emela run --target <target> -- <args>`, reads the first line of standard output, closes
/// it, and returns the exit code and what the program wrote to standard error.
fn run_until_stdout_closes(
    project: &Project,
    target: &str,
    args: &[&str],
) -> (Option<i32>, String) {
    let mut command = vec!["run", "--target", target, "--"];
    command.extend_from_slice(args);
    let mut child = project
        .emela(&command.iter().map(std::ffi::OsStr::new).collect::<Vec<_>>())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut first = String::new();
    stdout.read_line(&mut first).unwrap();
    assert_eq!(first, "line 0\n", "{target} {args:?}");
    drop(stdout);
    let out = wait(child, Duration::from_secs(60), target);
    (out.status.code(), program_stderr(&out.stderr))
}

/// A write to a standard output whose reader has closed it unwinds the program as interrupted:
/// the releases of both fibers run, later writes to standard output are discarded, nothing after
/// the scope runs, and the program exits with 141 without writing `interrupted`. The same holds
/// for `print_line` of SystemConsole. On native, the releases show that the program was not
/// stopped by SIGPIPE, whose exit code is also 141.
#[test]
fn a_closed_stdout_ends_the_program_with_141() {
    let project = Project::new("closed-stdout", include_str!("process/closed_stdout.emel"));
    for target in js_and_native() {
        let built = project.run(&["build", "--target", target], b"");
        assert!(built.status.success(), "{target}: {}", text(&built.stderr));
        for how in ["stdout", "console"] {
            assert_eq!(
                run_until_stdout_closes(&project, target, &[how]),
                (Some(141), "released\nreleased\n".into()),
                "{target} {how}"
            );
        }
    }
}

/// The same in a program that never suspends and in one that suspends but forks no fiber (the
/// WASM runtime has a form for each): `Exit.attempt` does not catch what unwinds.
#[test]
fn a_closed_stdout_is_not_caught_by_attempt() {
    for (name, source) in [
        (
            "closed-stdout-immediate",
            include_str!("process/closed_stdout_immediate.emel"),
        ),
        (
            "closed-stdout-suspend",
            include_str!("process/closed_stdout_suspend.emel"),
        ),
    ] {
        let project = Project::new(name, source);
        for target in js_and_native() {
            let built = project.run(&["build", "--target", target], b"");
            assert!(
                built.status.success(),
                "{name} {target}: {}",
                text(&built.stderr)
            );
            assert_eq!(
                run_until_stdout_closes(&project, target, &[]),
                (Some(141), "released\n".into()),
                "{name} {target}"
            );
        }
    }
}

/// Writes to a standard error whose reader has closed it are discarded, and the program goes on.
#[test]
fn a_closed_stderr_is_discarded() {
    let project = Project::new("closed-stderr", include_str!("process/closed_stderr.emel"));
    for target in every_target() {
        let built = project.run(&["build", "--target", target], b"");
        assert!(built.status.success(), "{target}: {}", text(&built.stderr));
        let mut child = project
            .emela(&["run", "--target", target].map(std::ffi::OsStr::new))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // The command writes its `running` line first; the program writes after it.
        let mut stderr = BufReader::new(child.stderr.take().unwrap());
        let mut running = String::new();
        stderr.read_line(&mut running).unwrap();
        assert!(running.starts_with("running `"), "{target}: {running}");
        drop(stderr);
        let out = wait(child, Duration::from_secs(60), target);
        assert_eq!(
            (out.status.code(), text(&out.stdout)),
            (Some(0), "done\n".into()),
            "{target}"
        );
    }
}

/// Under `emela test`, what SystemStdout writes without a line ending is shown as is, and the
/// results after it are still read (`[cmd.test.output]`).
#[test]
fn system_stdout_under_test_on_every_target() {
    let project = Project::new(
        "stdout-under-test",
        "import Std.Platform.{Platform}\n\
         import Std.Process.{Stdout}\n\
         \n\
         @test\n\
         fn first() {\n  with Platform { (use Stdout).print(\"no newline\") }\n}\n\
         \n\
         @test\n\
         fn second() {\n  with Platform { (use Stdout).print_line(\"a line\") }\n}\n",
    );
    for target in every_target() {
        let out = project.run(&["test", "--target", target, "--reporter", "plain"], b"");
        let stdout = text(&out.stdout);
        assert!(
            out.status.success()
                && stdout.contains("no newline")
                && stdout.contains("a line\n")
                && stdout.contains("test first ... ok")
                && stdout.contains("test second ... ok")
                && stdout.contains("2 passed; 0 failed"),
            "{target}: {stdout}{}",
            text(&out.stderr)
        );
    }
}
