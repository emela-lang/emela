//! The `Std.Env` internal hosts (`emela:env` `has` / `get`), which read WASI's environment with
//! `environ_sizes_get` / `environ_get`. `wasmtime` does not inherit the environment by default, so
//! the test passes a variable with `--env` and checks that `SystemEnv.get` finds it and falls back
//! to `None` for one that is not set.

mod common;

use std::process::Command;
use std::time::Duration;

use common::{out_dir, run_with_timeout, to_wasm, wasmtime};

const SRC: &str = r#"
import Std.Console.{Console, SystemConsole}
import Std.Env.{Env, SystemEnv}

fn main() {
  with SystemConsole, SystemEnv {
    env = use Env
    console = use Console
    console.print_line(Option.or(env.get("EMELA_TEST_SET"), "missing"))
    console.print_line(Option.or(env.get("EMELA_TEST_SET_ABSENT"), "fallback"))
    console.print_line(Option.or(env.get("EMELA"), "prefix-is-not-a-match"))
  }
}
"#;

#[test]
fn system_env_reads_the_environment() {
    let Some(wasmtime) = wasmtime() else {
        return;
    };
    let bytes = to_wasm(SRC).unwrap_or_else(|e| panic!("WASM compilation failed: {e:#?}"));
    wasmparser::validate(&bytes).unwrap();
    let dir = out_dir("env");
    std::fs::write(dir.join("main.wasm"), &bytes).unwrap();
    let mut command = Command::new(wasmtime);
    command
        .arg("run")
        .arg("--env")
        .arg("EMELA_TEST_SET=hello")
        .arg("main.wasm")
        .current_dir(&dir);
    let ran = run_with_timeout(command, Duration::from_secs(60)).expect("the program timed out");
    assert_eq!(ran.status.code(), Some(0), "stderr: {:?}", ran.stderr);
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout),
        "hello\nfallback\nprefix-is-not-a-match\n"
    );
    assert_eq!(String::from_utf8_lossy(&ran.stderr), "");
}
