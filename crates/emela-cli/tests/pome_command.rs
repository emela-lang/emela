//! Integration tests that run `emela pome` as a process, without the network
//! (`EMELA_POMEPROXY=off`). Resolution and fetching are tested in emela-pome.

use std::path::Path;
use std::process::Command;

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn emela(dir: &Path, args: &[&str]) -> Run {
    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .env("EMELA_POMEPROXY", "off")
        .env("EMELA_POME_CACHE", dir.join(".cache"))
        .output()
        .unwrap();
    Run {
        code: output.status.code(),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    }
}

fn project(deps: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    let manifest = format!(
        "[pome]\nname = \"github.com/me/app\"\nversion = \"0.1.0\"\nemela = \"0.21.0\"\n\n[dependencies]\n{deps}"
    );
    std::fs::write(dir.path().join("Pome.toml"), manifest).unwrap();
    dir
}

#[test]
fn install_writes_an_empty_lock_without_dependencies() {
    let dir = project("");
    let run = emela(&dir.path().join("src"), &["pome", "install"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(run.stdout, "Pome.lock is up to date\n");
    let lock = std::fs::read_to_string(dir.path().join("Pome.lock")).unwrap();
    assert_eq!(
        lock,
        "# Written by `emela pome`. Do not edit.\nversion = 1\n"
    );

    let list = emela(dir.path(), &["pome", "list"]);
    assert_eq!(list.stdout, "github.com/me/app\n");
}

#[test]
fn reports_errors_with_exit_code_1() {
    let dir = project("\"github.com/acme/x\" = \"v1.0.0\"\n");
    for (args, message) in [
        (&["pome", "install"][..], "nowhere to get it from"),
        (
            &["pome", "add", "github.com/acme/y@v1.0.0"],
            "nowhere to get it from",
        ),
        (
            &["pome", "add", "github.com/acme/y@v2.0.0"],
            "github.com/acme/y/v2",
        ),
        (
            &["pome", "remove", "github.com/acme/none"],
            "is not in [dependencies]",
        ),
        (&["pome", "list"], "run `emela pome install`"),
        (&["pome", "add", "not a path"], "invalid source path"),
    ] {
        let run = emela(dir.path(), args);
        assert_eq!(run.code, Some(1), "{args:?}");
        assert!(
            run.stderr.starts_with("error: ") && run.stderr.contains(message),
            "{args:?}: {}",
            run.stderr
        );
    }
    // Nothing was written.
    assert!(!dir.path().join("Pome.lock").exists());
}

#[test]
fn needs_a_project() {
    let dir = tempfile::tempdir().unwrap();
    let run = emela(dir.path(), &["pome", "install"]);
    assert_eq!(run.code, Some(1));
    assert!(
        run.stderr.contains("no `Pome.toml` found"),
        "{}",
        run.stderr
    );
}
