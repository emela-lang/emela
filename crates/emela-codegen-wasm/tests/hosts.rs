//! The WASI 0.3 hosts the bundled `Std` handlers use: Console, Env, Random and Clock.now. The
//! Clock sleeps have their own tests in `tests/clock.rs`; the file system and HTTP are not part of
//! this target yet.

mod common;

use common::*;

/// The interfaces every command component imports (spec `[target.wasm.output]`).
///
/// `wasi:cli/types@0.3.0` is here because `write-via-stream` of stdout and stderr returns a future
/// of an `error-code`, which that interface defines.
const ALWAYS_IMPORTED: &[&str] = &[
    "wasi:cli/exit@0.3.0",
    "wasi:cli/stderr@0.3.0",
    "wasi:cli/stdout@0.3.0",
    "wasi:cli/types@0.3.0",
];

/// Asserts that `src` imports exactly [`ALWAYS_IMPORTED`] plus `extra`, and returns its bytes.
///
/// The set, not the order, is compared: the order is pinned by `tests/component.rs`.
fn assert_imports(src: &str, extra: &[&str]) -> Vec<u8> {
    let bytes = compile_checked(src);
    let mut expected: Vec<String> = ALWAYS_IMPORTED
        .iter()
        .chain(extra)
        .map(|name| (*name).to_owned())
        .collect();
    expected.sort();
    let mut actual = component_imports(&bytes);
    actual.sort();
    assert_eq!(actual, expected, "the component imports");
    bytes
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
fn a_console_program_imports_only_the_output_interfaces() {
    // A program with only `SystemConsole` must not import the environment, random or clock.
    assert_imports(
        r#"
import Std.Console.{Console, SystemConsole}

fn main() {
  with SystemConsole {
    (use Console).print_line("console")
  }
}
"#,
        &[],
    );
}

#[test]
fn platform_with_console_imports_only_the_output_interfaces() {
    // `Std.Platform` bundles every system handler, so their external functions are in the
    // module. Only the operations the entry performs reach a host: creating the handlers does
    // not import anything (spec `[target.wasm.output]`).
    let bytes = assert_imports(
        r#"
import Std.Console.{Console}
import Std.Platform.{Platform}

fn main() {
  with Platform {
    (use Console).print_line("platform")
  }
}
"#,
        &[],
    );
    if wasmtime().is_none() {
        return;
    }
    let run = run_bytes(&out_dir("platform-console"), &bytes);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stdout, "platform\n");
    assert_eq!(run.stderr, "");
}

#[test]
fn platform_with_env_imports_only_the_environment() {
    let bytes = assert_imports(
        r#"
import Std.Env.{Env}
import Std.Platform.{Platform}

fn main() {
  with Platform {
    dbg((use Env).get("EMELA_TEST_UNSET"))
  }
}
"#,
        &["wasi:cli/environment@0.3.0"],
    );
    if wasmtime().is_none() {
        return;
    }
    let run = run_bytes(&out_dir("platform-env"), &bytes);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stderr, "None\n");
}

#[test]
fn platform_with_random_imports_only_random() {
    let bytes = assert_imports(
        r#"
import Std.Platform.{Platform}
import Std.Random.{Random}

fn main() {
  with Platform {
    dbg((use Random).float() >= 0.0 && (use Random).float() < 1.0)
  }
}
"#,
        &["wasi:random/random@0.3.0"],
    );
    if wasmtime().is_none() {
        return;
    }
    let run = run_bytes(&out_dir("platform-random"), &bytes);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stderr, "True\n");
}

#[test]
fn platform_with_clock_now_imports_only_the_system_clock() {
    let bytes = assert_imports(
        r#"
import Std.Clock.{Clock}
import Std.Instant.{Instant}
import Std.Platform.{Platform}

fn main() {
  with Platform {
    dbg(Instant.to_unix_milliseconds((use Clock).now()) > Int64.from_int(0))
  }
}
"#,
        &["wasi:clocks/system-clock@0.3.0"],
    );
    if wasmtime().is_none() {
        return;
    }
    let run = run_bytes(&out_dir("platform-clock"), &bytes);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stderr, "True\n");
}

// `SystemClock.sleep` and `Std.FileSystem` are not specified for WASM yet:
// `[target.wasm.external]` lists them as not available and the proposal that makes them available
// is still a draft. The implementation carries their hosts when a program reaches them, but this
// file does not pin their imports as part of the contract until the specification says so. Their
// behaviour is tested in `tests/clock.rs` and `tests/filesystem.rs`, next to the rest of those
// suites.

#[test]
fn an_unused_http_handler_is_not_an_error() {
    // `Std.Http` has no WASM host, so `SystemHttp` is only diagnosed when the entry performs
    // `Http.send`. Merely providing it through `Platform` does not report (spec
    // `[target.wasm.external]`).
    assert_imports(
        r#"
import Std.Console.{Console}
import Std.Platform.{Platform}

fn main() {
  with Platform {
    (use Console).print_line("no http")
  }
}
"#,
        &[],
    );
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
fn an_http_call_through_a_function_value_is_diagnosed() {
    // A closure that really calls a `suspend fn` external without a `wasm` host is followed when
    // the program calls it, so it is a diagnostic rather than an internal error or a silent
    // success.
    assert_host_call_is_diagnosed(
        r#"
import Std.Http.{Http, InvalidUrl, ConnectionFailed, ResponseNotText, SystemHttp}
import Std.Http.Request.{Get}

fn main() {
  with SystemHttp {
    look = fn() { (use Http).send(Request.new(Get, "https://example.com")).status }
    dbg(look() escape {
      InvalidUrl(url: _) -> 0
      ConnectionFailed(url: _, message: _) -> 0
      ResponseNotText(url: _) -> 0
    })
  }
}
"#,
    );
}

#[test]
fn an_http_call_is_an_unsupported_external() {
    // Http has no WASI 0.3 host on this target yet, so reaching it is a diagnostic rather than a
    // component that traps at run time.
    assert_host_call_is_diagnosed(
        r#"
import Std.Http.{Http, InvalidUrl, ConnectionFailed, ResponseNotText, SystemHttp}
import Std.Http.Request.{Get}

fn main() {
  with SystemHttp {
    dbg((use Http).send(Request.new(Get, "https://example.com")).status escape {
      InvalidUrl(url: _) -> 0
      ConnectionFailed(url: _, message: _) -> 0
      ResponseNotText(url: _) -> 0
    })
  }
}
"#,
    );
}

/// Asserts that `src` is rejected because it reaches `Std.Http`'s `send` host.
fn assert_host_call_is_diagnosed(src: &str) {
    let errors = to_wasm(src).expect_err("a reachable Http call must not compile");
    assert!(
        errors.iter().any(|error| {
            error.message.contains("host_send") && error.message.contains("`suspend fn` external")
        }),
        "{errors:#?}"
    );
}

#[test]
fn an_http_call_in_a_closure_passed_to_a_function_is_diagnosed() {
    assert_host_call_is_diagnosed(
        r#"
import Std.Http.{Http, InvalidUrl, ConnectionFailed, ResponseNotText, SystemHttp}
import Std.Http.Request.{Get}

fn twice(f: fn() -> Int fails InvalidUrl | ConnectionFailed | ResponseNotText) {
  dbg(f() escape {
    InvalidUrl(url: _) -> 0
    ConnectionFailed(url: _, message: _) -> 0
    ResponseNotText(url: _) -> 0
  })
  dbg(f() escape {
    InvalidUrl(url: _) -> 0
    ConnectionFailed(url: _, message: _) -> 0
    ResponseNotText(url: _) -> 0
  })
}

fn main() {
  with SystemHttp {
    twice(fn() { (use Http).send(Request.new(Get, "https://example.com")).status })
  }
}
"#,
    );
}

#[test]
fn an_http_call_in_a_captured_closure_is_diagnosed() {
    assert_host_call_is_diagnosed(
        r#"
import Std.Http.{Http, InvalidUrl, ConnectionFailed, ResponseNotText, SystemHttp}
import Std.Http.Request.{Get}

fn main() {
  with SystemHttp {
    look = fn() { (use Http).send(Request.new(Get, "https://example.com")).status }
    outer = fn() { look() }
    dbg(outer() escape {
      InvalidUrl(url: _) -> 0
      ConnectionFailed(url: _, message: _) -> 0
      ResponseNotText(url: _) -> 0
    })
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
