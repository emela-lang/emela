//! End-to-end tests for records (spec 0006): declaration, literal
//! construction, and field access, driven through the compiled `emela` binary
//! like a user would.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "emela-records-{label}-{}-{id}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_source(label: &str, source: &str) -> Output {
    let dir = temp_dir(label);
    let input = dir.join("main.emel");
    fs::write(&input, source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("run")
        .arg(&input)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(&dir);
    output
}

fn check_source(label: &str, source: &str) -> Output {
    let dir = temp_dir(label);
    let input = dir.join("main.emel");
    fs::write(&input, source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("check")
        .arg(&input)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(&dir);
    output
}

/// Declaration, literal (in declaration order and reversed), nested field
/// chains, and postfix access on a call result all execute end to end.
#[test]
fn records_construct_and_access_fields() {
    let output = run_source(
        "basic",
        r#"import std.io

record User {
    id: Int
    name: String
}

record Pair {
    user: User
    label: String
}

fn make(n: String) -> User {
    User {
        id: 7
        name: n
    }
}

fn describe(p: Pair) -> String {
    p.user.name ++ " / " ++ p.label
}

fn main() -> Unit uses { Io } {
    let pair = Pair {
        label: "admin"
        user: make("alice")
    }
    Io.print(describe(pair) ++ "\n")
    Io.print(make("bob").name ++ "\n")
}
"#,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "alice / admin\nbob\n"
    );
}

/// A literal whose written order differs from declaration order evaluates the
/// fields in written order (spec 0003 left-to-right).
#[test]
fn reversed_literal_evaluates_in_written_order() {
    let output = run_source(
        "order",
        r#"import std.io

record Two {
    first: Int
    second: Int
}

fn trace(label: String, value: Int) -> Int uses { Io } {
    Io.print(label)
    value
}

fn main() -> Unit uses { Io } {
    let two = Two {
        second: trace("b", 2)
        first: trace("a", 1)
    }
    Io.print("\n")
    Io.print(two.first)
    Io.print(two.second)
    Io.print("\n")
}
"#,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "ba\n12\n");
}

/// Unknown fields, missing fields, and unknown records are rejected.
#[test]
fn record_literal_errors_are_reported() {
    let source = "record User {\n    id: Int\n    name: String\n}\n\nfn main() -> Unit {\n    let u = User {\n        id: 1\n        extra: 2\n    }\n    ()\n}\n";
    let output = check_source("unknown-field", source);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no field `extra`"), "{stderr}");

    let source = "record User {\n    id: Int\n    name: String\n}\n\nfn main() -> Unit {\n    let u = User {\n        id: 1\n    }\n    ()\n}\n";
    let output = check_source("missing-field", source);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Missing fields"), "{stderr}");
    assert!(stderr.contains("`name`"), "{stderr}");

    let source = "fn main() -> Unit {\n    let u = Ghost {\n        id: 1\n    }\n    ()\n}\n";
    let output = check_source("unknown-record", source);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not a declared record"), "{stderr}");
}

/// Field access on a non-record and an unknown field on a record are rejected
/// with a record-aware diagnostic.
#[test]
fn field_access_errors_are_reported() {
    let source = "record User {\n    id: Int\n}\n\nfn main() -> Unit {\n    let u = User {\n        id: 1\n    }\n    let x = u.missing\n    ()\n}\n";
    let output = check_source("bad-access", source);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no field `missing`"), "{stderr}");
}

/// Records travel through module imports like enums (spec 0037 R2(b)): the
/// type is usable bare-named from the importer.
#[test]
fn records_import_across_modules() {
    let dir = temp_dir("import");
    fs::write(
        dir.join("shape.emel"),
        "module shape\n\nrecord Point {\n    x: Int\n    y: Int\n}\n\npub fn origin() -> Point {\n    Point {\n        x: 0\n        y: 0\n    }\n}\n",
    )
    .unwrap();
    let input = dir.join("main.emel");
    fs::write(
        &input,
        "import shape\n\nfn main() -> Int {\n    let p = shape.origin()\n    let q = Point {\n        x: 40\n        y: 2\n    }\n    q.x + q.y + p.x\n}\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("run")
        .arg(&input)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(&dir);
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.status.code(), Some(42));
}
