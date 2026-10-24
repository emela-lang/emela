//! Integration tests that run `emela test` as a process.
//!
//! Covers argument parsing, the `@test` checks (E0226), and lowering the source's test functions
//! and running them on node. Details of the result output are tested in emela-driver's
//! tests/test_runner.rs.

use std::path::PathBuf;
use std::process::Command;

struct Project {
    dir: PathBuf,
}

impl Project {
    fn new(name: &str, files: &[(&str, &str)]) -> Self {
        let dir =
            std::env::temp_dir().join(format!("emela-cli-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, text) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        Project { dir }
    }

    fn emela(&self, args: &[&str]) -> (Option<i32>, String, String) {
        let output = Command::new(env!("CARGO_BIN_EXE_emela"))
            .args(args)
            .current_dir(&self.dir)
            .env("NO_COLOR", "1")
            .output()
            .unwrap();
        (
            output.status.code(),
            String::from_utf8(output.stdout).unwrap(),
            String::from_utf8(output.stderr).unwrap(),
        )
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn invalid_test_functions_are_errors() {
    let project = Project::new(
        "invalid",
        &[
            ("Pome.toml", ""),
            (
                "src/main.emel",
                "@test\nfn takes(x: Int) {\n  assert x == 1\n}\n",
            ),
        ],
    );
    let (code, stdout, stderr) = project.emela(&["test"]);
    assert_eq!(code, Some(1));
    assert_eq!(stdout, "");
    insta::assert_snapshot!(stderr);
}

fn have_tool(var: &str, default: &str) -> bool {
    let program = std::env::var(var)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default.to_owned());
    // `EMELA_CC` may carry flags (`cc -m32`); only its program is probed.
    let program = program.split_whitespace().next().unwrap_or(default);
    Command::new(program)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn const_memoisation_and_release_notes_agree_on_targets() {
    // The same two-test pairs on every target: `BROKEN` is evaluated once and both tests get the
    // same defect; `SELF` is evaluated again in each test; a failing release adds a note to the
    // self-reference defect without wrapping it (spec 5.6, 14.4).
    if !have_tool("EMELA_NODE", "node")
        || !have_tool("EMELA_CC", "cc")
        || !have_tool("EMELA_WASMTIME", "wasmtime")
    {
        eprintln!("skipping: need node, cc and wasmtime");
        return;
    }
    let project = Project::new(
        "const-notes",
        &[
            ("Pome.toml", ""),
            (
                "src/main.emel",
                r#"
const BROKEN = 1 / zero()

fn zero() -> Int {
  dbg("eval zero")
  0
}

const SELF: Int = if probe() { SELF } else { 0 }

fn probe() -> Bool {
  dbg("probe")
  True
}

effect Pool {
  fn get() -> Int
}

handler PoolH: Pool {
  init { PoolH }
  release { panic("rel") }
  fn get() { 0 }
}

const SELF_REL: Int = with PoolH { if probe_rel() { SELF_REL } else { 0 } }

fn probe_rel() -> Bool {
  dbg("probe rel")
  True
}

@test
fn broken_first() { dbg(BROKEN) }

@test
fn broken_second() { dbg(BROKEN) }

@test
fn self_first() { dbg(SELF) }

@test
fn self_second() { dbg(SELF) }

@test
fn self_rel_first() { dbg(SELF_REL) }

@test
fn self_rel_second() { dbg(SELF_REL) }
"#,
            ),
        ],
    );
    let mut outputs = Vec::new();
    for target in ["js", "c", "wasm"] {
        let (code, stdout, stderr) = project.emela(&["test", "--target", target]);
        assert_eq!(code, Some(1), "{target}: {stdout}\n{stderr}");
        outputs.push((target, stdout, stderr));
    }
    let (_, js_stdout, js_stderr) = &outputs[0];
    for (target, stdout, stderr) in &outputs {
        assert_eq!(stdout, js_stdout, "{target}: stdout");
        assert_eq!(stderr, js_stderr, "{target}: stderr");
    }

    assert_eq!(
        js_stderr.matches("\"eval zero\"\n").count(),
        1,
        "{js_stderr}"
    );
    assert_eq!(js_stderr.matches("\"probe\"\n").count(), 2, "{js_stderr}");
    assert_eq!(
        js_stderr.matches("\"probe rel\"\n").count(),
        2,
        "{js_stderr}"
    );
    // The report writes the defect messages to stdout; `dbg` wrote the stderr lines above.
    assert_eq!(
        js_stdout
            .matches("defect: while evaluating const BROKEN: division by zero\n")
            .count(),
        2,
        "{js_stdout}"
    );
    assert_eq!(
        js_stdout
            .matches("defect: const SELF refers to itself while being evaluated\n")
            .count(),
        2,
        "{js_stdout}"
    );
    assert_eq!(
        js_stdout
            .matches("defect: const SELF_REL refers to itself while being evaluated\n")
            .count(),
        2,
        "{js_stdout}"
    );
    assert_eq!(
        js_stdout
            .matches("note: the release of handler PoolH also failed: rel\n")
            .count(),
        2,
        "{js_stdout}"
    );
    assert!(
        !js_stdout.contains("while evaluating const SELF"),
        "{js_stdout}"
    );
}

#[test]
fn valid_tests_run() {
    let project = Project::new(
        "valid",
        &[
            ("Pome.toml", ""),
            (
                "src/main.emel",
                "@test\nfn adds() {\n  assert 1 + 2 == 3\n}\n\n@test\nfn subtracts() {\n  assert 3 - 1 == 2\n}\n",
            ),
        ],
    );
    for (args, ran) in [
        (&["test"][..], 2),
        (&["test", ".", "adds"], 1),
        (&["test", "src/main.emel"], 2),
    ] {
        let (code, stdout, stderr) = project.emela(args);
        assert_eq!((code, stderr.as_str()), (Some(0), ""), "{args:?}");
        assert!(
            stdout.starts_with(&format!("running {ran} test")),
            "{stdout}"
        );
        assert!(stdout.contains("test adds ... ok"), "{stdout}");
    }
}
