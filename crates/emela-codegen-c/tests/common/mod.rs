//! Compiles Emela sources to C, builds them with cc and runs them.
//!
//! Programs print with `print(s)`, a pure external function named `emela_test_print` (declared by
//! [`PRELUDE`], or in `tests/programs` with `./print.c`, which the driver compiles) that [`HELPER`]
//! implements here (lowering without source paths leaves the file unresolved); `dbg` writes to
//! stderr. Runs set `EMELA_LEAK_CHECK`, so a program
//! that leaves allocations live exits with 99 and says so on stderr.
//!
//! Builds use the flags of `[target.c.cc]` without `-O2`.

#![allow(dead_code)]

#[path = "../../../emela-core/tests/support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use emela_codegen_c::{
    BUNDLED_DIR, CError, COutput, EMELA_H, EMELA_H_FILE, Entry, RUNTIME_C, RUNTIME_C_FILE,
    RUNTIME_H, RUNTIME_H_FILE, compile,
};
use emela_core::Roots;

/// The host side of `print`, and of `sleep` (`tests/programs/sleep.c`, a `suspend fn`).
pub const HELPER: &str = r#"#include <stdio.h>
#include <stdint.h>
#include "emela.h"
void emela_test_print(const char *s) { puts(s); }
static void emela_test_wake(em_task *task, void *ctx) { em_complete_int(task, (int32_t)(intptr_t)ctx); }
void emela_test_sleep(int32_t ms, em_task *task) {
  em_after(ms, task, emela_test_wake, (void *)(intptr_t)ms);
  em_on_cancel(task, emela_test_wake, (void *)(intptr_t)ms);
}
"#;

/// Declares `print` for the test programs.
pub const PRELUDE: &str = r#"@external(js, "console.log")
@external(c, "emela_test_print")
fn print(s: String) -> () use {}
"#;

/// Lowers `src` and compiles it to C.
pub fn to_c(src: &str) -> Result<COutput, Vec<CError>> {
    let m = support::try_lower(&[("main.emel", src)], Roots::Exported)
        .unwrap_or_else(|e| panic!("lowering failed: {e:#?}"));
    compile(&m, &Entry::Main)
}

/// The C compiler (`EMELA_CC` or `cc`), or `None` if it cannot be started.
pub fn cc() -> Option<&'static str> {
    static CC: OnceLock<Option<String>> = OnceLock::new();
    CC.get_or_init(|| {
        let cc = std::env::var("EMELA_CC")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "cc".to_owned());
        let ok = Command::new(&cc)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success());
        if !ok {
            eprintln!("skipping: cannot start the C compiler `{cc}` (set EMELA_CC)");
        }
        ok.then_some(cc)
    })
    .as_deref()
}

/// The sanitizer flags cc supports here: ASan and UBSan, or UBSan alone (ASan hangs on some
/// macOS setups), or none.
pub fn sanitizers() -> Option<&'static str> {
    static FLAGS: OnceLock<Option<&'static str>> = OnceLock::new();
    *FLAGS.get_or_init(|| {
        let cc = cc()?;
        let dir = out_dir("sanitizer-probe");
        std::fs::write(dir.join("p.c"), "int main(void) { return 0; }\n").unwrap();
        ["-fsanitize=address,undefined", "-fsanitize=undefined"]
            .into_iter()
            .find(|flag| {
                let built = Command::new(cc)
                    .args([*flag, "-o", "p", "p.c"])
                    .current_dir(&dir)
                    .output()
                    .is_ok_and(|o| o.status.success());
                built
                    && run_with_timeout(Command::new(dir.join("p")), Duration::from_secs(20))
                        .is_some_and(|o| o.status.success())
            })
    })
}

/// Runs the command, killing it after `limit`.
pub fn run_with_timeout(mut command: Command, limit: Duration) -> Option<Output> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if start.elapsed() < limit => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

pub fn out_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("codegen-c")
        .join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[derive(Debug)]
pub struct Run {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

/// Writes the C files of `out` to `dir`.
pub fn write(dir: &Path, out: &COutput) {
    std::fs::write(dir.join("main.c"), &out.main).unwrap();
    std::fs::write(dir.join(RUNTIME_C_FILE), RUNTIME_C).unwrap();
    std::fs::write(dir.join(RUNTIME_H_FILE), RUNTIME_H).unwrap();
    std::fs::write(dir.join(EMELA_H_FILE), EMELA_H).unwrap();
    std::fs::write(dir.join("helper.c"), HELPER).unwrap();
    for (path, contents) in &out.bundled {
        let p = dir.join(BUNDLED_DIR).join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, contents).unwrap();
    }
}

fn shared_objects(flags: &[&str]) -> (PathBuf, PathBuf) {
    type ObjectCache = Mutex<std::collections::HashMap<Vec<String>, (PathBuf, PathBuf)>>;
    static OBJECTS: OnceLock<ObjectCache> = OnceLock::new();
    let key: Vec<_> = flags.iter().map(|flag| (*flag).to_owned()).collect();
    let mut objects = OBJECTS.get_or_init(Default::default).lock().unwrap();
    if let Some(paths) = objects.get(&key) {
        return paths.clone();
    }

    let dir = out_dir(&format!("shared-{}", key.join("-")));
    std::fs::write(dir.join(RUNTIME_C_FILE), RUNTIME_C).unwrap();
    std::fs::write(dir.join(RUNTIME_H_FILE), RUNTIME_H).unwrap();
    std::fs::write(dir.join(EMELA_H_FILE), EMELA_H).unwrap();
    std::fs::write(dir.join("helper.c"), HELPER).unwrap();
    let cc = cc().expect("checked by the caller");
    let compile_object = |source: &str, name: &str| {
        let output = Command::new(cc)
            .args(["-std=c11", "-fno-strict-aliasing", "-pthread", "-I.", "-c"])
            .args(flags)
            .args([source, "-o", name])
            .current_dir(&dir)
            .output()
            .expect("cannot start cc");
        assert!(
            output.status.success(),
            "cc failed to compile shared {source}:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        dir.join(name)
    };
    let paths = (
        compile_object(RUNTIME_C_FILE, "runtime.o"),
        compile_object("helper.c", "helper.o"),
    );
    objects.insert(key, paths.clone());
    paths
}

/// Builds `out` in `dir` with `flags` and runs it with the leak check on.
pub fn build_and_run(dir: &Path, out: &COutput, flags: &[&str]) -> Run {
    let cc = cc().expect("checked by the caller");
    write(dir, out);
    let (runtime_object, helper_object) = shared_objects(flags);
    let mut sources = vec!["main.c".to_owned()];
    for (path, _) in &out.bundled {
        sources.push(format!("{BUNDLED_DIR}/{path}"));
    }
    sources.extend(out.host_files.iter().cloned());
    let built = Command::new(cc)
        .args([
            "-std=c11",
            "-fno-strict-aliasing",
            "-pthread",
            "-I.",
            "-o",
            "main",
        ])
        .args(flags)
        .args(&sources)
        .arg(&runtime_object)
        .arg(&helper_object)
        .arg("-lm")
        .current_dir(dir)
        .output()
        .expect("cannot start cc");
    assert!(
        built.status.success(),
        "cc failed:\n{}\n{}",
        String::from_utf8_lossy(&built.stderr),
        out.main
    );
    let mut command = Command::new(dir.join("main"));
    command
        .env("EMELA_LEAK_CHECK", "1")
        .env("ASAN_OPTIONS", "detect_leaks=0")
        // macOS fills freed and new memory, so that reading it shows.
        .env("MallocScribble", "1")
        .env("MallocPreScribble", "1")
        .current_dir(dir);
    let ran = run_with_timeout(command, Duration::from_secs(120)).expect("the program timed out");
    Run {
        stdout: String::from_utf8_lossy(&ran.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&ran.stderr).into_owned(),
        code: ran.status.code().unwrap_or(-1),
    }
}

/// Compiles `src` to C and runs it, once plainly and once with ASan and UBSan when cc supports
/// them (the outputs must agree). `None` without a C compiler.
pub fn run(name: &str, src: &str) -> Option<Run> {
    cc()?;
    let out = to_c(src).unwrap_or_else(|e| panic!("C compilation failed: {e:#?}"));
    let plain = build_and_run(&out_dir(name), &out, &["-O1"]);
    if let Some(flag) = sanitizers() {
        let checked = build_and_run(
            &out_dir(&format!("{name}-san")),
            &out,
            &["-O0", flag, "-fno-sanitize-recover=all"],
        );
        assert_eq!(
            (&plain.stdout, &plain.stderr, plain.code),
            (&checked.stdout, &checked.stderr, checked.code),
            "the sanitized build of {name} behaves differently"
        );
    }
    Some(plain)
}
