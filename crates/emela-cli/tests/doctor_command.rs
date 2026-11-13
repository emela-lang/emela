//! `emela doctor` run as a process (`[cmd.doctor]`). The environment is cleared, so `PATH` is an
//! empty directory and no program is found: the tests check which targets are required and the
//! exit code, not the programs of this machine.

use std::path::Path;
use std::process::{Command, Output};

/// A directory with the files `files` (path, contents).
fn dir_with(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (path, text) in files {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

fn project(target: Option<&str>) -> tempfile::TempDir {
    let manifest = match target {
        Some(target) => format!("[build]\ntarget = \"{target}\"\n"),
        None => String::new(),
    };
    dir_with(&[
        ("Pome.toml", &manifest),
        ("src/main.emel", "fn main() {}\n"),
    ])
}

/// Runs `emela` in `cwd` with only `PATH` (an empty directory) set.
fn emela(cwd: &Path, args: &[&str]) -> Output {
    let empty = tempfile::tempdir().unwrap();
    Command::new(env!("CARGO_BIN_EXE_emela"))
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", empty.path())
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

/// The target lines of the text output, with ` (required)` where it is.
fn target_lines(output: &Output) -> Vec<String> {
    stdout(output)
        .lines()
        .filter(|line| {
            ["js", "c", "wasm", "c-embedded"]
                .iter()
                .any(|t| *line == *t || *line == format!("{t} (required)"))
        })
        .map(str::to_owned)
        .collect()
}

#[test]
fn the_first_line_is_the_version() {
    let dir = dir_with(&[]);
    let version = emela(dir.path(), &["--version"]);
    let doctor = emela(dir.path(), &["doctor"]);
    let text = stdout(&doctor);
    let mut lines = text.lines();
    assert_eq!(lines.next(), stdout(&version).lines().next());
    assert_eq!(lines.next(), Some(""));
}

#[test]
fn outside_a_project_nothing_is_required() {
    let dir = dir_with(&[]);
    let output = emela(dir.path(), &["doctor"]);
    assert_eq!(target_lines(&output), ["js", "c", "wasm"]);
    // Every program is missing, but none is required.
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
}

#[test]
fn the_target_of_the_project_is_required() {
    let dir = project(Some("wasm"));
    let output = emela(dir.path(), &["doctor"]);
    assert_eq!(target_lines(&output), ["js", "c", "wasm (required)"]);
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn a_project_without_a_target_requires_js() {
    // The build target of a project is `[build].target`, else `js` (`[cmd.build]`).
    let dir = project(None);
    let output = emela(dir.path(), &["doctor"]);
    assert_eq!(target_lines(&output), ["js (required)", "c", "wasm"]);
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn the_target_flag_replaces_the_project_target() {
    let dir = project(Some("wasm"));
    let output = emela(dir.path(), &["doctor", "--target", "c"]);
    assert_eq!(target_lines(&output), ["js", "c (required)", "wasm"]);
}

#[test]
fn the_target_flag_outside_a_project() {
    let dir = dir_with(&[]);
    let output = emela(dir.path(), &["doctor", "--target", "wasm"]);
    assert_eq!(target_lines(&output), ["js", "c", "wasm (required)"]);
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn the_path_names_the_project() {
    let dir = dir_with(&[
        ("app/Pome.toml", "[build]\ntarget = \"c\"\n"),
        ("app/src/main.emel", "fn main() {}\n"),
    ]);
    let output = emela(dir.path(), &["doctor", "app"]);
    assert_eq!(target_lines(&output), ["js", "c (required)", "wasm"]);
}

#[test]
fn an_unknown_target_in_the_project_is_an_error() {
    let dir = project(Some("cobol"));
    let output = emela(dir.path(), &["doctor"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("E0232"), "{stderr}");
    assert_ne!(output.status.code(), Some(0));
}

#[test]
fn json_is_one_object() {
    let dir = project(Some("wasm"));
    let version = stdout(&emela(dir.path(), &["--version"]));
    let output = emela(dir.path(), &["doctor", "--format", "json"]);
    let json: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    // `emela --version` prints `emela <version>`.
    assert_eq!(
        json["emela"]
            .as_str()
            .map(|v| format!("emela {v}"))
            .as_deref(),
        version.lines().next()
    );
    let targets: Vec<_> = json["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["target"].as_str().unwrap(),
                t["required"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(targets, [("js", false), ("c", false), ("wasm", true)]);
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn no_color_outside_a_terminal() {
    // Standard output is a pipe here, so the output has no escape sequences (`[diag]`).
    let dir = dir_with(&[]);
    let output = emela(dir.path(), &["doctor"]);
    assert!(!stdout(&output).contains('\x1b'));
}

#[test]
fn nothing_is_written_to_standard_error() {
    let dir = project(Some("wasm"));
    let output = emela(dir.path(), &["doctor"]);
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
}
