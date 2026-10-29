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
        self.emela_env(args, &[])
    }

    /// Runs `emela` outside any coding agent (`[cmd.test.reporter]`), with `env` added.
    fn emela_env(&self, args: &[&str], env: &[(&str, &str)]) -> (Option<i32>, String, String) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_emela"));
        for var in emela_driver::AGENT_ENV_VARS {
            command.env_remove(var);
        }
        let output = command
            .args(args)
            .current_dir(&self.dir)
            .env("NO_COLOR", "1")
            .envs(env.iter().copied())
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

const FAILING: &[(&str, &str)] = &[
    ("Pome.toml", ""),
    (
        "src/main.emel",
        "@test\nfn adds() {\n  assert 1 + 2 == 3\n}\n\n@test\nfn adds_wrong() {\n  assert 1 + 2 == 4\n}\n",
    ),
];

const AGENT_OUTPUT: &str = "FAIL adds_wrong src/main.emel:8:3\n\
    defect: assertion failed: 1 + 2 == 4\n  left: 3\n right: 4\n\
    \n\
    test result: FAILED. 1 passed; 1 failed; 0 filtered out\n";

/// `[cmd.test.reporter]`: `--reporter` selects the reporter; every reporter writes to stdout, and
/// the exit code does not depend on it.
#[test]
fn reporter_flag_selects_the_reporter() {
    let project = Project::new("reporter-flag", FAILING);
    let (code, plain, stderr) = project.emela(&["test", "--reporter", "plain"]);
    assert_eq!((code, stderr.as_str()), (Some(1), ""));
    assert!(
        plain.starts_with("running 2 tests\ntest adds ... ok\n"),
        "{plain}"
    );

    let (code, agent, stderr) = project.emela(&["test", "--reporter", "agent"]);
    assert_eq!((code, stderr.as_str()), (Some(1), ""));
    assert_eq!(agent, AGENT_OUTPUT);

    let (code, pretty, stderr) = project.emela(&["test", "--reporter", "pretty"]);
    assert_eq!((code, stderr.as_str()), (Some(1), ""));
    assert!(pretty.contains("adds_wrong"), "{pretty}");
    assert!(
        pretty.contains("defect: assertion failed: 1 + 2 == 4"),
        "{pretty}"
    );
    // Stdout is not a terminal here, so `pretty` uses no color.
    assert!(!pretty.contains('\x1b'), "{pretty}");

    let passing = Project::new(
        "reporter-flag-ok",
        &[
            ("Pome.toml", ""),
            (
                "src/main.emel",
                "@test\nfn adds() {\n  assert 1 + 2 == 3\n}\n",
            ),
        ],
    );
    for reporter in ["plain", "agent", "pretty"] {
        let (code, _, _) = passing.emela(&["test", "--reporter", reporter]);
        assert_eq!(code, Some(0), "{reporter}");
    }
}

/// `[cmd.test.reporter]`: another name is an error in the command-line arguments, reported as an
/// unknown `--target` is, and no test is run.
#[test]
fn unknown_reporter_is_an_argument_error() {
    let project = Project::new("reporter-unknown", FAILING);
    let (target_code, _, target_err) = project.emela(&["test", "--target", "json"]);
    let (code, stdout, stderr) = project.emela(&["test", "--reporter", "json"]);
    assert_eq!(code, target_code);
    assert_eq!(code, Some(2));
    assert_eq!(stdout, "");
    assert!(
        stderr.contains("invalid value 'json' for '--reporter"),
        "{stderr}"
    );
    assert!(
        target_err.contains("invalid value 'json' for '--target"),
        "{target_err}"
    );
    assert!(!project.dir.join("target").exists());
}

/// `[cmd.test.reporter]`: without `--reporter`, `agent` when `AI_AGENT` is set and not empty, and
/// `plain` when stdout is not a terminal.
#[test]
fn reporter_defaults_from_the_environment() {
    let project = Project::new("reporter-default", FAILING);
    let (code, stdout, _) = project.emela_env(&["test"], &[("AI_AGENT", "1")]);
    assert_eq!(code, Some(1));
    assert_eq!(stdout, AGENT_OUTPUT);

    for env in [&[][..], &[("AI_AGENT", "")]] {
        let (code, stdout, _) = project.emela_env(&["test"], env);
        assert_eq!(code, Some(1));
        assert!(stdout.starts_with("running 2 tests\n"), "{env:?}: {stdout}");
    }

    // `--reporter` comes first.
    let (_, stdout, _) = project.emela_env(&["test", "--reporter", "plain"], &[("AI_AGENT", "1")]);
    assert!(stdout.starts_with("running 2 tests\n"), "{stdout}");
}

/// `[cmd.test.reporter]`: diagnostics found by checking go to stderr whatever the reporter.
#[test]
fn diagnostics_go_to_stderr_with_every_reporter() {
    let project = Project::new(
        "reporter-diagnostics",
        &[
            ("Pome.toml", ""),
            (
                "src/main.emel",
                "@test\nfn takes(x: Int) {\n  assert x == 1\n}\n",
            ),
        ],
    );
    let (_, _, plain_err) = project.emela(&["test", "--reporter", "plain"]);
    for reporter in ["agent", "pretty"] {
        let (code, stdout, stderr) = project.emela(&["test", "--reporter", reporter]);
        assert_eq!(code, Some(1), "{reporter}");
        assert_eq!(stdout, "", "{reporter}");
        assert_eq!(stderr, plain_err, "{reporter}");
    }
}
