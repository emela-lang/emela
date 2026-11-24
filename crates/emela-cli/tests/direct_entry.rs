//! `main.mjs` launches `main` when it is the entry module of the JS host, and not when it is
//! imported (`[target.js.module]`).

use std::path::{Path, PathBuf};
use std::process::Command;

const MAIN: &str = "import Std.Console.{Console, SystemConsole}\nimport Std.Process.{Args, SystemArgs}\n\nfn main() {\n  with SystemConsole, SystemArgs {\n    console <- Console\n    args <- Args\n    console.print_line(\"main ran with #{args.list()}\")\n  }\n}\n";

/// Builds a project whose `main` prints a line, and returns the directory of `main.mjs`.
fn build(dir: &Path) -> PathBuf {
    std::fs::write(dir.join("Pome.toml"), "").unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/main.emel"), MAIN).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("build")
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .env("EMELA_POMEPROXY", "off")
        .env("EMELA_POME_CACHE", dir.join(".cache"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    dir.join("target/emela/js")
}

/// Runs the JS host in `dir` with `args`, and returns the exit code, standard output and standard
/// error.
fn host(dir: &Path, args: &[&str]) -> (Option<i32>, String, String) {
    let out = Command::new(emela_driver::js_host_program())
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into(),
        String::from_utf8_lossy(&out.stderr).into(),
    )
}

#[test]
fn starting_the_host_with_main_mjs_launches_main() {
    let dir = tempfile::tempdir().unwrap();
    let out = build(dir.path());
    let (code, stdout, stderr) = host(&out, &["main.mjs", "a", "b"]);
    assert_eq!(
        (code, stdout.as_str()),
        (Some(0), "main ran with [\"a\", \"b\"]\n"),
        "{stderr}"
    );
}

#[test]
fn importing_main_mjs_from_another_module_does_not_launch_main() {
    let dir = tempfile::tempdir().unwrap();
    let out = build(dir.path());
    std::fs::write(
        out.join("other.mjs"),
        "await import(\"./main.mjs\");\nconsole.log(\"imported\");\n",
    )
    .unwrap();
    let (code, stdout, stderr) = host(&out, &["other.mjs"]);
    assert_eq!((code, stdout.as_str()), (Some(0), "imported\n"), "{stderr}");
}

/// The first argument after the code is a word, not a file. Whichever way the host tells the entry
/// module, importing `main.mjs` neither launches `main` nor throws.
#[test]
fn importing_main_mjs_from_evaluated_code_does_not_launch_main() {
    let dir = tempfile::tempdir().unwrap();
    let out = build(dir.path());
    let (code, stdout, stderr) = host(
        &out,
        &[
            "-e",
            "import(\"./main.mjs\").then(() => console.log(\"imported\"))",
            "no-such-file",
        ],
    );
    assert_eq!((code, stdout.as_str()), (Some(0), "imported\n"), "{stderr}");
}
