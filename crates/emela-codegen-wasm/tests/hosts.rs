//! The synchronous WASI 0.3 hosts the bundled `Std` handlers use: Console, Env, Random and
//! Clock.now. Sleep, the test clock's virtual sleep, the file system and HTTP are not part of
//! this target yet.

mod common;

use common::*;
use wasmparser::{Parser, Payload};

/// The interface names a component imports.
fn component_imports(bytes: &[u8]) -> Vec<String> {
    let mut imports = Vec::new();
    for payload in Parser::new(0).parse_all(bytes) {
        if let Payload::ComponentImportSection(section) = payload.unwrap() {
            for import in section {
                imports.push(import.unwrap().name.full_name().into_owned());
            }
        }
    }
    imports
}

fn stderr(name: &str, src: &str) -> Option<String> {
    run(name, src).map(|r| {
        assert_eq!(r.code, 0, "{name} exited with {}: {}", r.code, r.stderr);
        r.stderr
    })
}

#[test]
fn console_writes_lines_to_stdout_and_stderr() {
    let Some(run) = run(
        "console-lines",
        r#"
import Std.Console.{Console, SystemConsole}

fn main() {
  with SystemConsole {
    c = use Console
    c.print_line("one")
    c.print_line("two")
    c.print_error_line("three")
    c.print_line("")
  }
}
"#,
    ) else {
        return;
    };
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stdout, "one\ntwo\n\n");
    assert_eq!(run.stderr, "three\n");
}

#[test]
fn env_reads_set_empty_and_unset_variables() {
    if wasmtime().is_none() {
        return;
    }
    let bytes = compile_checked(
        r#"
import Std.Env.{Env, SystemEnv}

fn main() {
  with SystemEnv {
    dbg((use Env).get("EMELA_TEST_SET"))
    dbg((use Env).get("EMELA_TEST_EMPTY"))
    dbg((use Env).get("EMELA_TEST_UNSET"))
  }
}
"#,
    );
    let dir = out_dir("env");
    let run = run_bytes_with_command(&dir, &bytes, |command| {
        command
            .arg("-S")
            .arg("inherit-env=y")
            .env("EMELA_TEST_SET", "hello")
            .env("EMELA_TEST_EMPTY", "");
    });
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        run.stderr, "Some(\"hello\")\nSome(\"\")\nNone\n",
        "{}",
        run.stderr
    );
}

#[test]
fn env_uses_the_first_of_duplicate_names() {
    // `get-environment` is a list and need not deduplicate. Like C's `getenv`, the first entry
    // wins. The wasmtime CLI keeps the first `--env` for a repeated name (and the runtime takes
    // the first of the list it is given), so the program sees `first`.
    if wasmtime().is_none() {
        return;
    }
    let bytes = compile_checked(
        r#"
import Std.Env.{Env, SystemEnv}

fn main() {
  with SystemEnv {
    dbg((use Env).get("EMELA_TEST_DUP"))
  }
}
"#,
    );
    let dir = out_dir("env-duplicate");
    let run = run_bytes_with_command(&dir, &bytes, |command| {
        command
            .arg("-S")
            .arg("inherit-env=y")
            .arg("--env")
            .arg("EMELA_TEST_DUP=first")
            .arg("--env")
            .arg("EMELA_TEST_DUP=second");
    });
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stderr, "Some(\"first\")\n", "{}", run.stderr);
}

#[test]
fn env_never_observes_non_utf8_bytes() {
    // `get-environment` returns WIT `string`s, so a non-UTF-8 variable has no representation.
    // wasmtime 49 rejects the inherited environment before the program starts (a host may also
    // skip the variable or replace the bytes with U+FFFD); it must never reach `Env` as raw
    // bytes. A UTF-8 value with non-ASCII characters still works.
    if wasmtime().is_none() {
        return;
    }
    #[cfg(unix)]
    {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let bytes = compile_checked(
            r#"
import Std.Env.{Env, SystemEnv}

fn main() {
  with SystemEnv {
    dbg((use Env).get("EMELA_TEST_BAD"))
    dbg((use Env).get("EMELA_TEST_SET"))
    dbg((use Env).get("EMELA_TEST_UNICODE"))
  }
}
"#,
        );
        let dir = out_dir("env-non-utf8");
        let bad = OsStr::from_bytes(b"\xff");
        let run = run_bytes_with_command(&dir, &bytes, |command| {
            command
                .arg("-S")
                .arg("inherit-env=y")
                .env("EMELA_TEST_SET", "ok")
                .env("EMELA_TEST_UNICODE", "東京")
                .env("EMELA_TEST_BAD", bad);
        });
        if run.code != 0 {
            // The host refused the environment: nothing reached the program.
            assert!(
                run.stderr.to_lowercase().contains("utf-8"),
                "unexpected failure: {}",
                run.stderr
            );
        } else {
            // A host that exposes a lossy value can only give a valid UTF-8 string.
            assert!(
                run.stderr == "None\nSome(\"ok\")\nSome(\"東京\")\n"
                    || run.stderr == "Some(\"\u{FFFD}\")\nSome(\"ok\")\nSome(\"東京\")\n",
                "{}",
                run.stderr
            );
        }
    }
}

#[test]
fn random_int_covers_the_signed_range_and_float_is_in_the_unit_interval() {
    let Some(out) = stderr(
        "random",
        r#"
import Std.Random.{Random, SystemRandom}

fn main() {
  with SystemRandom {
    ints = List.map(List.range(0, 200), fn(_) { (use Random).int() })
    floats = List.map(List.range(0, 200), fn(_) { (use Random).float() })
    dbg(List.all(ints, fn(x) { x >= -2147483647 - 1 && x <= 2147483647 }))
    dbg(List.any(ints, fn(x) { x < 0 }))
    dbg(List.all(floats, fn(x) { x >= 0.0 && x < 1.0 }))
    dbg(List.any(floats, fn(x) { x > 0.5 }))
  }
}
"#,
    ) else {
        return;
    };
    assert_eq!(out, "True\nTrue\nTrue\nTrue\n");
}

#[test]
fn clock_now_is_unix_epoch_milliseconds() {
    let Some(out) = stderr(
        "clock-now",
        r#"
import Std.Clock.{Clock, SystemClock}
import Std.Instant.{Instant}

fn main() {
  with SystemClock {
    before = Instant.to_unix_milliseconds((use Clock).now())
    dbg(before > 1577836800000)
    dbg(before < 4102444800000)
  }
}
"#,
    ) else {
        return;
    };
    // `system-clock.now` is not monotonic, so two calls need not be non-decreasing.
    assert_eq!(out, "True\nTrue\n");
}

#[test]
fn unused_hosts_are_not_imported() {
    if wasmtime().is_none() {
        return;
    }
    // A program with only SystemConsole must not import the environment, random or clock.
    let console = compile_checked(
        r#"
import Std.Console.{Console, SystemConsole}

fn main() {
  with SystemConsole {
    (use Console).print_line("console")
  }
}
"#,
    );
    let imports = component_imports(&console);
    assert!(
        imports.iter().any(|i| i == "wasi:cli/stdout@0.3.0"),
        "{imports:?}"
    );
    for unused in [
        "wasi:cli/environment@0.3.0",
        "wasi:random/random@0.3.0",
        "wasi:clocks/system-clock@0.3.0",
    ] {
        assert!(!imports.iter().any(|i| i == unused), "{imports:?}");
    }

    // `Std.Platform` bundles FileSystem and Http, but an unused operation is not reachable and
    // those interfaces are not part of the command world, so they are not imports either.
    let platform = compile_checked(
        r#"
import Std.Console.{Console}
import Std.Platform.{Platform}

fn main() {
  with Platform {
    (use Console).print_line("platform")
  }
}
"#,
    );
    let imports = component_imports(&platform);
    assert!(
        !imports
            .iter()
            .any(|i| i.contains("http") || i.contains("filesystem")),
        "{imports:?}"
    );
    // `Std.Platform` bundles the synchronous handlers, so their external functions are
    // reachable even when only `Console` is used: the environment, random and system-clock
    // interfaces are imported. This is a known limitation (see NOTES.md).
    for bundled in [
        "wasi:cli/environment@0.3.0",
        "wasi:random/random@0.3.0",
        "wasi:clocks/system-clock@0.3.0",
    ] {
        assert!(
            imports.iter().any(|i| i == bundled),
            "`Std.Platform` should import `{bundled}`: {imports:?}"
        );
    }
    let dir = out_dir("platform-console");
    let run = run_bytes(&dir, &platform);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stdout, "platform\n");
    assert_eq!(run.stderr, "");
}

#[test]
fn test_console_and_test_clock_now_still_work() {
    let Some(out) = stderr(
        "test-handlers",
        r#"
import Std.Clock.{Clock, TestClock}
import Std.Console.{Console, TestConsole}
import Std.Instant.{Instant}
import Std.State.{State}

fn main() {
  (_, lines) = State.run([], fn() {
    with TestConsole {
      c = use Console
      c.print_line("a")
      c.print_error_line("dropped")
      c.print_line("b")
    }
  })
  dbg(lines)
  start = Instant.from_unix_milliseconds(Int64.from_int(1000))
  (_, t) = State.run(start, fn() {
    with TestClock {
      (use Clock).now()
    }
  })
  dbg(Instant.to_unix_milliseconds(t))
}
"#,
    ) else {
        return;
    };
    assert_eq!(out, "[\"a\", \"b\"]\n1000\n");
}

#[test]
fn sleep_through_a_function_value_is_diagnosed() {
    // A closure that really calls `sleep` is followed when the program calls it, so it is a
    // diagnostic rather than an internal error or a silent success.
    let errors = to_wasm(
        r#"
import Std.Clock.{Clock, SystemClock}

fn main() {
  with SystemClock {
    nap = fn() { (use Clock).sleep(1) }
    nap()
  }
}
"#,
    )
    .expect_err("a called closure that sleeps must not compile");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("sleep") && error.message.contains("suspend")),
        "{errors:#?}"
    );
}

#[test]
fn sleep_is_still_an_unsupported_external() {
    // Clock.sleep is not implemented for this target, so reaching it is a diagnostic rather than
    // a component that traps at run time.
    let errors = to_wasm(
        r#"
import Std.Clock.{Clock, SystemClock}

fn main() {
  with SystemClock {
    (use Clock).sleep(1)
  }
}
"#,
    )
    .expect_err("sleep must not compile");
    assert!(
        errors.iter().any(|error| {
            error.message.contains("sleep")
                && (error.message.contains("suspending") || error.message.contains("`wasm` import"))
        }),
        "{errors:#?}"
    );
}

/// Asserts that `src` is rejected because it reaches `sleep`.
fn assert_sleep_is_diagnosed(src: &str) {
    let errors = to_wasm(src).expect_err("a reachable sleep must not compile");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("sleep") && error.message.contains("suspend")),
        "{errors:#?}"
    );
}

#[test]
fn sleep_in_a_closure_passed_to_a_function_is_diagnosed() {
    assert_sleep_is_diagnosed(
        r#"
import Std.Clock.{Clock, SystemClock}

fn twice(f: fn() -> ()) {
  f()
  f()
}

fn main() {
  with SystemClock {
    twice(fn() { (use Clock).sleep(1) })
  }
}
"#,
    );
}

#[test]
fn sleep_in_a_captured_closure_is_diagnosed() {
    assert_sleep_is_diagnosed(
        r#"
import Std.Clock.{Clock, SystemClock}

fn main() {
  with SystemClock {
    nap = fn() { (use Clock).sleep(1) }
    outer = fn() { nap() }
    outer()
  }
}
"#,
    );
}

#[test]
fn an_unused_delegating_sleep_does_not_report() {
    // `Logged.sleep` performs the outer `Clock.sleep`, but nothing performs `Logged.sleep`, so
    // only `now` is reachable.
    let Some(out) = stderr(
        "unused-delegating-sleep",
        r#"
import Std.Clock.{Clock, SystemClock}
import Std.Instant.{Instant}

handler Logged: Clock {
  fn now() { (use Clock).now() }
  suspend fn sleep(ms, resume) {
    (use Clock).sleep(ms)
    resume(())
  }
}

fn main() {
  with SystemClock {
    with Logged {
      t = (use Clock).now()
      dbg(Instant.to_unix_milliseconds(t) > Int64.from_int(0))
    }
  }
}
"#,
    ) else {
        return;
    };
    assert_eq!(out, "True\n");
}

#[test]
fn a_release_that_reads_the_clock_runs() {
    // The release may suspend by type (Clock has `sleep`), so it is held; without fibers the
    // hold has nothing to protect and the release runs straight through.
    let Some(out) = stderr(
        "release-reads-clock",
        r#"
import Std.Clock.{Clock, SystemClock}
import Std.Instant.{Instant}

effect Res {
  fn id() -> Int
}

handler Timed(id: Int): Res {
  init { Timed(id: 7) }

  release {
    t = (use Clock).now()
    dbg(Instant.to_unix_milliseconds(t) > Int64.from_int(0))
  }

  fn id() { self.id }
}

fn main() {
  with SystemClock, Timed {
    dbg((use Res).id())
  }
}
"#,
    ) else {
        return;
    };
    assert_eq!(out, "7\nTrue\n");
}
