//! End-to-end tests for `emela fmt` (spec 0035): in-place rewriting, the
//! `--check` mode and its exit codes, and the repository corpus staying in
//! canonical form. The formatting rules themselves are pinned by the unit
//! tests in `src/fmt.rs`; these tests cover the CLI surface.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> std::path::PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-fmt-test-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn emela(args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_emela"));
    for arg in args {
        command.arg(arg);
    }
    command
}

const UGLY: &str = "fn add (x:Int,y:Int)->Int uses {} {\n  x+y\n}\n";
const CANONICAL: &str = "fn add(x: Int, y: Int) -> Int uses {} {\n    x + y\n}\n";

#[test]
fn fmt_rewrites_in_place_and_prints_the_path() {
    let dir = temp_dir();
    let file = dir.join("main.emel");
    fs::write(&file, UGLY).unwrap();
    let output = emela(&["fmt"]).arg(&file).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("main.emel"),
        "changed file not listed: {stdout}"
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), CANONICAL);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn fmt_is_silent_and_clean_on_canonical_input() {
    let dir = temp_dir();
    let file = dir.join("main.emel");
    fs::write(&file, CANONICAL).unwrap();
    let output = emela(&["fmt"]).arg(&file).output().unwrap();
    assert!(output.status.success());
    assert!(
        output.stdout.is_empty(),
        "expected no output for a canonical file"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn fmt_check_fails_and_lists_files_without_writing() {
    let dir = temp_dir();
    let file = dir.join("main.emel");
    fs::write(&file, UGLY).unwrap();
    let output = emela(&["fmt", "--check"]).arg(&file).output().unwrap();
    assert!(
        !output.status.success(),
        "--check must fail on an unformatted file"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("main.emel"));
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        UGLY,
        "--check must not write"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn fmt_check_passes_on_canonical_input() {
    let dir = temp_dir();
    let file = dir.join("main.emel");
    fs::write(&file, CANONICAL).unwrap();
    let output = emela(&["fmt", "--check"]).arg(&file).output().unwrap();
    assert!(output.status.success());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn fmt_recurses_into_directories() {
    let dir = temp_dir();
    fs::create_dir_all(dir.join("nested")).unwrap();
    fs::write(dir.join("nested/one.emel"), UGLY).unwrap();
    fs::write(dir.join("two.emel"), UGLY).unwrap();
    fs::write(dir.join("not_emela.txt"), "left alone").unwrap();
    let output = emela(&["fmt"]).arg(&dir).output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        fs::read_to_string(dir.join("nested/one.emel")).unwrap(),
        CANONICAL
    );
    assert_eq!(fs::read_to_string(dir.join("two.emel")).unwrap(), CANONICAL);
    assert_eq!(
        fs::read_to_string(dir.join("not_emela.txt")).unwrap(),
        "left alone"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn unparsable_file_is_reported_and_left_untouched() {
    let dir = temp_dir();
    let good = dir.join("good.emel");
    let bad = dir.join("bad.emel");
    fs::write(&good, UGLY).unwrap();
    fs::write(&bad, "fn broken( -> {\n").unwrap();
    let output = emela(&["fmt"]).arg(&dir).output().unwrap();
    assert!(!output.status.success(), "a parse error must fail the run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("bad.emel"), "diagnostic missing: {stderr}");
    assert_eq!(fs::read_to_string(&bad).unwrap(), "fn broken( -> {\n");
    // The remaining files are still formatted (spec 0035 C3).
    assert_eq!(fs::read_to_string(&good).unwrap(), CANONICAL);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn formatted_output_still_checks() {
    let dir = temp_dir();
    let file = dir.join("main.emel");
    fs::write(
        &file,
        "enum Color {\n  Red,\n  Blue,\n}\n\nfn pick(n:Int)->Color {\n  if n>0 {Color::Red} else {Color::Blue}\n}\n\nfn main()->Unit uses {} {\n  let c=pick(3)\n  ()\n}\n",
    )
    .unwrap();
    let output = emela(&["fmt"]).arg(&file).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = emela(&["check"]).arg(&file).output().unwrap();
    assert!(
        output.status.success(),
        "formatted output no longer checks:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = fs::remove_dir_all(&dir);
}

/// The repository's own Emela sources stay canonical: `emela fmt --check`
/// over `examples/` and the embedded prelude is clean. This is the corpus
/// regression test — any formatter change that reshapes existing style shows
/// up here first.
#[test]
fn repository_corpus_is_canonical() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let examples = root.join("examples");
    let prelude = root.join("crates/emela/src/std/core.emel");
    let output = emela(&["fmt", "--check"])
        .arg(&examples)
        .arg(&prelude)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "repository corpus needs formatting:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
