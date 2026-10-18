//! The `Std.FileSystem` internal hosts on WASM (spec 15.2): the WASI file system and the opaque
//! `Outcome` handle.
//!
//! The bundled `host_*` functions are `suspend fn`s (spec 12.6), and the task attachment that
//! makes them callable lives in the scheduler/fiber track. So these tests drive the *same*
//! runtime functions through non-suspending `@external(wasm, "emela:fs", ...)` bindings: the
//! `(module, name)` registry, the runtime functions and the `Outcome` handle are the real ones,
//! only the wrapped `fn`s are ordinary (not `suspend`) so no scheduler is needed. The module is
//! validated with `wasmparser`; when `wasmtime` is present it is also run against a fresh
//! temporary directory (the WASI preopen).

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use common::{Run, compile_checked, wasmtime};

/// The runtime's file-system operations and the `Outcome` accessors, as ordinary externals. The
/// `(module, name)` pairs are exactly the ones `std/file_system.emel` binds to (`emela:fs`).
const PRELUDE: &str = r#"
@external
type Outcome

@external(wasm, "emela:fs", "read_text")
fn raw_read_text(path: String) -> Outcome use {}

@external(wasm, "emela:fs", "write_text")
fn raw_write_text(path: String, text: String) -> Outcome use {}

@external(wasm, "emela:fs", "append_text")
fn raw_append_text(path: String, text: String) -> Outcome use {}

@external(wasm, "emela:fs", "exists")
fn raw_exists(path: String) -> Bool use {}

@external(wasm, "emela:fs", "is_directory")
fn raw_is_directory(path: String) -> Bool use {}

@external(wasm, "emela:fs", "list_directory")
fn raw_list_directory(path: String) -> Outcome use {}

@external(wasm, "emela:fs", "create_directory")
fn raw_create_directory(path: String) -> Outcome use {}

@external(wasm, "emela:fs", "remove")
fn raw_remove(path: String) -> Outcome use {}

@external(wasm, "emela:fs", "outcome_error")
fn outcome_error(o: Outcome) -> String use {}

@external(wasm, "emela:fs", "outcome_message")
fn outcome_message(o: Outcome) -> String use {}

@external(wasm, "emela:fs", "outcome_text")
fn outcome_text(o: Outcome) -> String use {}

@external(wasm, "emela:fs", "outcome_name_count")
fn outcome_name_count(o: Outcome) -> Int use {}

@external(wasm, "emela:fs", "outcome_name_at")
fn outcome_name_at(o: Outcome, i: Int) -> String use {}

@external(wasm, "emela:fs", "release_outcome")
fn release_outcome(o: Outcome) -> () use {}

fn outcome_code(o: Outcome) -> String {
  code = outcome_error(o)
  release_outcome(o)
  code
}

fn read(path: String) -> String {
  o = raw_read_text(path)
  code = outcome_error(o)
  text = outcome_text(o)
  release_outcome(o)
  if code == "" { text } else { "error:#{code}" }
}

fn names_onto(o: Outcome, i: Int, acc: List[String]) -> List[String] {
  if i < 0 { acc } else { names_onto(o, i - 1, [outcome_name_at(o, i), ..acc]) }
}

fn names(path: String) -> List[String] {
  o = raw_list_directory(path)
  n = outcome_name_count(o)
  ns = names_onto(o, n - 1, [])
  release_outcome(o)
  ns
}

fn count(path: String) -> Int {
  o = raw_list_directory(path)
  n = outcome_name_count(o)
  release_outcome(o)
  n
}
"#;

fn source(body: &str) -> String {
    format!("{PRELUDE}\nfn main() {{\n{body}\n}}\n")
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("emela-wasm-fs-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Runs `bytes` as `main.wasm` in `dir`, preopening it as the WASI current directory.
fn run_directed(dir: &Path, bytes: &[u8]) -> Run {
    let wt = wasmtime().expect("checked by the caller");
    std::fs::write(dir.join("main.wasm"), bytes).unwrap();
    let mut command = Command::new(wt);
    command
        .arg("run")
        .arg("--dir=.")
        .arg("main.wasm")
        .current_dir(dir);
    let ran = common::run_with_timeout(command, Duration::from_secs(60)).expect("timed out");
    Run {
        stdout: String::from_utf8_lossy(&ran.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&ran.stderr).into_owned(),
        code: ran.status.code().unwrap_or(-1),
    }
}

/// The module must be well formed even without `wasmtime`.
#[test]
fn the_module_is_valid() {
    let src = source("  dbg(raw_exists(\"x\"))");
    let bytes = compile_checked(&src);
    wasmparser::validate(&bytes).unwrap();
}

/// A program that does not use `Std.FileSystem` must not carry the fragment's WASI imports (the
/// fragment is on demand, so the modules of other programs keep their bytes).
#[test]
fn the_fragment_is_emitted_on_demand() {
    let plain = common::to_wasm("fn main() { dbg(1 + 1) }").unwrap();
    let text = String::from_utf8_lossy(&plain);
    assert!(
        !text.contains("path_open") && !text.contains("fd_readdir"),
        "the file-system imports leaked into a program that does not use them"
    );
}

const BODY: &str = r#"
  dbg(raw_exists("d/a.txt"))
  dbg(outcome_code(raw_create_directory("d")))
  dbg(raw_is_directory("d"))
  dbg(outcome_code(raw_write_text("d/a.txt", "hello")))
  dbg(read("d/a.txt"))
  dbg(outcome_code(raw_append_text("d/a.txt", "!")))
  dbg(read("d/a.txt"))
  dbg(outcome_code(raw_write_text("d/b.txt", "b")))
  dbg(names("d"))
  dbg(outcome_code(raw_list_directory("d/a.txt")))
  dbg(outcome_code(raw_write_text("nodir/x.txt", "y")))
  dbg(outcome_code(raw_create_directory("d/a.txt")))
  dbg(outcome_code(raw_remove("d/b.txt")))
  dbg(names("d"))
  dbg(outcome_code(raw_read_text("d")))
  dbg(outcome_code(raw_remove("d")))
  dbg(outcome_code(raw_remove("d/a.txt")))
  dbg(outcome_code(raw_remove("d")))
  dbg(outcome_code(raw_create_directory("e/f/g")))
  dbg(raw_is_directory("e"))
  dbg(raw_is_directory("e/f/g"))
  dbg(raw_exists("d"))
  dbg(outcome_code(raw_read_text("missing.txt")))
"#;

#[test]
fn wasi_file_system_works() {
    if wasmtime().is_none() {
        return;
    }
    let bytes = compile_checked(&source(BODY));
    let dir = temp_dir("run");
    let got = run_directed(&dir, &bytes);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.code, 0, "stderr:\n{}", got.stderr);
    let expected = concat!(
        "False\n",
        "\"\"\n",
        "True\n",
        "\"\"\n",
        "\"hello\"\n",
        "\"\"\n",
        "\"hello!\"\n",
        "\"\"\n",
        "[\"a.txt\", \"b.txt\"]\n",
        "\"NotADirectory\"\n",
        "\"FileNotFound\"\n",
        "\"NotADirectory\"\n",
        "\"\"\n",
        "[\"a.txt\"]\n",
        "\"IsADirectory\"\n",
        "\"DirectoryNotEmpty\"\n",
        "\"\"\n",
        "\"\"\n",
        "\"\"\n",
        "True\n",
        "True\n",
        "False\n",
        "\"FileNotFound\"\n",
    );
    assert_eq!(got.stderr, expected, "stdout:\n{}", got.stdout);
}

#[test]
fn read_text_rejects_invalid_utf8() {
    if wasmtime().is_none() {
        return;
    }
    let dir = temp_dir("utf8");
    std::fs::write(dir.join("bad.txt"), [0x66, 0xff, 0x6f]).unwrap();
    let bytes = compile_checked(&source("  dbg(outcome_code(raw_read_text(\"bad.txt\")))"));
    let got = run_directed(&dir, &bytes);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.code, 0, "stderr:\n{}", got.stderr);
    assert!(
        got.stderr.contains("InvalidText"),
        "invalid UTF-8 was not reported:\n{}",
        got.stderr
    );
}

/// The whole text is written even when the host accepts only part of it at a time (W4-1): a large
/// write and an empty write both round-trip.
#[test]
fn write_text_writes_the_whole_text() {
    if wasmtime().is_none() {
        return;
    }
    let big = "abcdefghij".repeat(5000);
    let body = format!(
        "  big = \"{big}\"\n  \
           dbg(outcome_code(raw_write_text(\"big.txt\", big)))\n  \
           dbg(read(\"big.txt\") == big)\n  \
           dbg(outcome_code(raw_write_text(\"tiny.txt\", \"\")))\n  \
           dbg(read(\"tiny.txt\"))\n"
    );
    let dir = temp_dir("write");
    let bytes = compile_checked(&source(&body));
    let got = run_directed(&dir, &bytes);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.code, 0, "stderr:\n{}", got.stderr);
    assert_eq!(
        got.stderr, "\"\"\nTrue\n\"\"\n\"\"\n",
        "stdout:\n{}",
        got.stdout
    );
}

/// An existing directory is `IsADirectory` on write and append, decided by the stat rather than the
/// open's errno (W4-2).
#[test]
fn writing_to_a_directory_is_is_a_directory() {
    if wasmtime().is_none() {
        return;
    }
    let body = "  dbg(outcome_code(raw_create_directory(\"dir\")))\n  \
                   dbg(outcome_code(raw_write_text(\"dir\", \"x\")))\n  \
                   dbg(outcome_code(raw_append_text(\"dir\", \"x\")))\n  \
                   dbg(outcome_code(raw_read_text(\"dir\")))\n";
    let dir = temp_dir("dir-write");
    let bytes = compile_checked(&source(body));
    let got = run_directed(&dir, &bytes);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.code, 0, "stderr:\n{}", got.stderr);
    assert_eq!(
        got.stderr, "\"\"\n\"IsADirectory\"\n\"IsADirectory\"\n\"IsADirectory\"\n",
        "stdout:\n{}",
        got.stdout
    );
}

/// `create_directory` matches the C host on the path edges it skips: an empty path fails like
/// `mkdir ""` (FileNotFound), and a repeated or trailing slash is not a component (W4-3).
#[test]
fn create_directory_skips_empty_and_repeated_slashes() {
    if wasmtime().is_none() {
        return;
    }
    let body = "  dbg(outcome_code(raw_create_directory(\"\")))\n  \
                   dbg(outcome_code(raw_create_directory(\"t/\")))\n  \
                   dbg(raw_is_directory(\"t\"))\n  \
                   dbg(outcome_code(raw_create_directory(\"u//v\")))\n  \
                   dbg(raw_is_directory(\"u/v\"))\n  \
                   dbg(outcome_code(raw_create_directory(\"u/v\")))\n";
    let dir = temp_dir("mkdir-edges");
    let bytes = compile_checked(&source(body));
    let got = run_directed(&dir, &bytes);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.code, 0, "stderr:\n{}", got.stderr);
    assert_eq!(
        got.stderr, "\"FileNotFound\"\n\"\"\nTrue\n\"\"\nTrue\n\"\"\n",
        "stdout:\n{}",
        got.stdout
    );
}

/// Reading an empty file is `""` and the read does not depend on the stat size (W4-4).
#[test]
fn read_text_reads_an_empty_file() {
    if wasmtime().is_none() {
        return;
    }
    let dir = temp_dir("empty");
    std::fs::write(dir.join("empty.txt"), []).unwrap();
    let bytes = compile_checked(&source("  dbg(read(\"empty.txt\"))"));
    let got = run_directed(&dir, &bytes);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.code, 0, "stderr:\n{}", got.stderr);
    assert_eq!(got.stderr, "\"\"\n", "stdout:\n{}", got.stdout);
}

/// More than `NAMES_CAP` (16) names grow the array, and the result is sorted by name (W4-4 / A-5).
#[test]
fn list_directory_grows_and_sorts() {
    if wasmtime().is_none() {
        return;
    }
    let dir = temp_dir("grow");
    std::fs::create_dir_all(dir.join("many")).unwrap();
    // Created in reverse so a missing sort is visible.
    for i in (0..20).rev() {
        std::fs::write(dir.join("many").join(format!("m{i:02}")), "x").unwrap();
    }
    let bytes = compile_checked(&source("  dbg(count(\"many\"))\n  dbg(names(\"many\"))\n"));
    let got = run_directed(&dir, &bytes);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.code, 0, "stderr:\n{}", got.stderr);
    let mut want = String::from("20\n[");
    for i in 0..20 {
        if i > 0 {
            want.push_str(", ");
        }
        want.push_str(&format!("\"m{i:02}\""));
    }
    want.push_str("]\n");
    assert_eq!(got.stderr, want, "stdout:\n{}", got.stdout);
}

/// A directory whose entries do not fit the 4096-byte readdir buffer is read across several
/// `fd_readdir` calls with the continuation cookie, and `outcome_name_at` is checked at the edges
/// (W4-4 / A-5).
#[test]
fn list_directory_pages_with_the_readdir_cookie() {
    if wasmtime().is_none() {
        return;
    }
    let dir = temp_dir("paging");
    std::fs::create_dir_all(dir.join("big")).unwrap();
    for i in 0..150 {
        std::fs::write(dir.join("big").join(format!("p{i:03}")), "x").unwrap();
    }
    let body = "  dbg(count(\"big\"))\n  \
                     o = raw_list_directory(\"big\")\n  \
                     dbg(outcome_name_at(o, 0))\n  \
                     dbg(outcome_name_at(o, 149))\n  \
                     dbg(outcome_name_at(o, 150))\n  \
                     dbg(outcome_name_at(o, -1))\n  \
                     release_outcome(o)\n";
    let bytes = compile_checked(&source(body));
    let got = run_directed(&dir, &bytes);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.code, 0, "stderr:\n{}", got.stderr);
    assert_eq!(
        got.stderr, "150\n\"p000\"\n\"p149\"\n\"\"\n\"\"\n",
        "stdout:\n{}",
        got.stdout
    );
}

/// `outcome_message` carries the WASM-specific `FileSystemFailure` message (`"<CODE>: <call>
/// '<path>'"`, W4-7); an over-long name is the easy way to reach it.
#[test]
fn outcome_message_reports_a_filesystem_failure() {
    if wasmtime().is_none() {
        return;
    }
    let long = "a".repeat(300);
    let want = format!("ENAMETOOLONG: open '{long}'");
    let body = format!(
        "  o = raw_read_text(\"{long}\")\n  \
           dbg(outcome_error(o))\n  \
           dbg(outcome_message(o) == \"{want}\")\n  \
           release_outcome(o)\n"
    );
    let dir = temp_dir("message");
    let bytes = compile_checked(&source(&body));
    let got = run_directed(&dir, &bytes);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.code, 0, "stderr:\n{}", got.stderr);
    assert_eq!(
        got.stderr, "\"FileSystemFailure\"\nTrue\n",
        "stdout:\n{}",
        got.stdout
    );
}

/// `is_directory` is false for a plain file and for a missing path (A-5).
#[test]
fn is_directory_is_false_for_a_file_and_a_missing_path() {
    if wasmtime().is_none() {
        return;
    }
    let body = "  dbg(raw_is_directory(\"missing\"))\n  \
                   dbg(outcome_code(raw_create_directory(\"dd\")))\n  \
                   dbg(outcome_code(raw_write_text(\"dd/f\", \"x\")))\n  \
                   dbg(raw_is_directory(\"dd/f\"))\n  \
                   dbg(raw_is_directory(\"dd\"))\n  \
                   dbg(raw_exists(\"missing\"))\n";
    let dir = temp_dir("is-dir");
    let bytes = compile_checked(&source(body));
    let got = run_directed(&dir, &bytes);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.code, 0, "stderr:\n{}", got.stderr);
    assert_eq!(
        got.stderr, "False\n\"\"\n\"\"\nFalse\nTrue\nFalse\n",
        "stdout:\n{}",
        got.stdout
    );
}

/// A path outside the preopen — absolute or escaping with `..` — is `FileAccessDenied`, not
/// `FileNotFound` (W4-6: WASI answers `ENOTCAPABLE`).
#[test]
fn paths_outside_the_preopen_are_access_denied() {
    if wasmtime().is_none() {
        return;
    }
    let body = "  dbg(outcome_code(raw_read_text(\"/etc/hostname\")))\n  \
                   dbg(outcome_code(raw_read_text(\"../outside.txt\")))\n  \
                   dbg(outcome_code(raw_read_text(\"..\")))\n";
    let dir = temp_dir("escape");
    let bytes = compile_checked(&source(body));
    let got = run_directed(&dir, &bytes);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.code, 0, "stderr:\n{}", got.stderr);
    assert_eq!(
        got.stderr, "\"FileAccessDenied\"\n\"FileAccessDenied\"\n\"FileAccessDenied\"\n",
        "stdout:\n{}",
        got.stdout
    );
}

/// Whether the host's file system can hold a name that is not valid UTF-8. APFS (macOS) rejects
/// the write with `EILSEQ`; Linux and the WASI hosts used in CI accept it.
#[cfg(unix)]
fn can_create_a_non_utf8_name(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStringExt;
    let probe = dir.join("non-utf8-probe");
    if std::fs::create_dir_all(&probe).is_err() {
        return false;
    }
    let name = std::ffi::OsString::from_vec(vec![b'x', 0xff, b'y']);
    let ok = std::fs::write(probe.join(name), "").is_ok();
    let _ = std::fs::remove_dir_all(&probe);
    ok
}

/// A directory entry whose name is not valid UTF-8 (W4-5). WASI's `fd_readdir` does not carry a
/// UTF-8 invariant, and the names are copied as raw bytes like the C host's `strdup` of `d_name`;
/// wasmtime, however, refuses a host name that is not valid UTF-8 and fails the whole call as a
/// `FileSystemFailure`. Either way the operation never invents replacement characters (Node's
/// `readdir` does), so the test accepts the raw entry or the failure, but not `InvalidText`.
///
/// The setup needs the host to hold a non-UTF-8 name, which APFS on macOS refuses (`EILSEQ`), so
/// the test is skipped there rather than failing.
#[cfg(unix)]
#[test]
fn list_directory_keeps_or_rejects_a_non_utf8_name() {
    use std::os::unix::ffi::OsStringExt;
    if wasmtime().is_none() {
        return;
    }
    let dir = temp_dir("raw-name");
    if !can_create_a_non_utf8_name(&dir) {
        let _ = std::fs::remove_dir_all(&dir);
        eprintln!("skipping: this file system rejects a non-UTF-8 name (EILSEQ)");
        return;
    }
    std::fs::create_dir_all(dir.join("raw")).unwrap();
    let name = std::ffi::OsString::from_vec(vec![b'x', 0xff, b'y']);
    if let Err(err) = std::fs::write(dir.join("raw").join(name), "hi") {
        let _ = std::fs::remove_dir_all(&dir);
        eprintln!("skipping: cannot create a non-UTF-8 name: {err}");
        return;
    }
    let body = "  n = count(\"raw\")\n  \
                     c = outcome_code(raw_list_directory(\"raw\"))\n  \
                     dbg(n)\n  dbg(c)\n";
    let bytes = compile_checked(&source(body));
    let got = run_directed(&dir, &bytes);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.code, 0, "stderr:\n{}", got.stderr);
    assert!(
        got.stderr == "1\n\"\"\n" || got.stderr == "0\n\"FileSystemFailure\"\n",
        "a non-UTF-8 name should be kept raw or fail, not replaced:\n{}",
        got.stderr
    );
}
