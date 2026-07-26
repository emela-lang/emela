//! End-to-end tests for effect-row subsumption on function values (spec 0023):
//! a function whose `uses` row is a subset is acceptable where a wider row is
//! wanted — as a call argument or a record field. The reverse (a wider row
//! where a narrower one is wanted) is rejected. Effects are backed by the
//! embedded `std.io` (spec 0038), which resolves with no `--package`.

use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

/// Runs `emela check` against a single self-contained file (no package).
fn check_single(source: &str) -> std::process::Output {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-subsume-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
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

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A pure function value (`uses {}`) is acceptable where a `uses { Io }`
/// parameter is wanted: `{} ⊆ { Io }` (spec 0023 subsumption).
#[test]
fn pure_handler_accepted_as_argument() {
    let output = check_single(
        "import std.io\n\
         fn apply(f: (Int) -> Int uses { Io }, x: Int) -> Int uses { Io } { f(x) }\n\
         fn pure_inc(x: Int) -> Int uses {} { x + 1 }\n\
         fn main() -> Unit uses { Io } {\n\
             let r = apply(pure_inc, 5)\n\
             ()\n\
         }\n",
    );
    assert!(
        output.status.success(),
        "expected check to pass:\n{}",
        stderr(&output)
    );
}

/// The same subsumption applies when storing a handler into a record field
/// whose declared row is wider — the shape quince's Router relies on.
#[test]
fn pure_handler_accepted_in_record_field() {
    let output = check_single(
        "import std.io\n\
         record Handler { run: (Int) -> Int uses { Io } }\n\
         fn pure_inc(x: Int) -> Int uses {} { x + 1 }\n\
         fn main() -> Unit uses { Io } {\n\
             let h = Handler { run: pure_inc }\n\
             let f = h.run\n\
             let g = f(5)\n\
             ()\n\
         }\n",
    );
    assert!(
        output.status.success(),
        "expected check to pass:\n{}",
        stderr(&output)
    );
}

/// The reverse is rejected: an effectful function (`uses { Io }`) is not
/// acceptable where a pure `uses {}` parameter is wanted, since it would let
/// `apply_pure` perform an effect it never declared.
#[test]
fn effectful_handler_rejected_where_pure_expected() {
    let output = check_single(
        "import std.io\n\
         fn apply_pure(f: (Int) -> Int uses {}, x: Int) -> Int uses {} { f(x) }\n\
         fn io_inc(x: Int) -> Int uses { Io } {\n\
             let p = Io.print(\"x\\n\")\n\
             x + 1\n\
         }\n\
         fn main() -> Unit uses { Io } {\n\
             let r = apply_pure(io_inc, 5)\n\
             ()\n\
         }\n",
    );
    assert!(!output.status.success(), "expected check to fail");
    let err = stderr(&output);
    assert!(
        err.contains("Type mismatch"),
        "unexpected diagnostic:\n{err}"
    );
}

/// Asserts a program is rejected with a `Type mismatch` naming both rows: the
/// shape every inference site below has to produce.
fn assert_row_mismatch(source: &str) {
    let output = check_single(source);
    assert!(!output.status.success(), "expected check to fail");
    let err = stderr(&output);
    assert!(
        err.contains("Type mismatch"),
        "unexpected diagnostic:\n{err}"
    );
    assert!(
        err.contains("expected `(Int) -> Int uses {}`, but found `(Int) -> Int uses { Io }`"),
        "unexpected diagnostic:\n{err}"
    );
}

/// A pure-declared record field rejects an effectful handler. `match_type` binds
/// the field's type parameters but never compared rows, so this used to be
/// accepted — and a `uses {}` function could then run the effect through the
/// stored handler.
#[test]
fn effectful_handler_rejected_in_record_field() {
    assert_row_mismatch(
        "import std.io\n\
         record Box { run: (Int) -> Int uses {} }\n\
         fn io_inc(x: Int) -> Int uses { Io } {\n\
             let p = Io.print(\"x\\n\")\n\
             x + 1\n\
         }\n\
         fn main() -> Unit uses { Io } {\n\
             let b = Box { run: io_inc }\n\
             ()\n\
         }\n",
    );
}

/// The same for a generic record, where the field type goes through the type
/// parameter substitution first (spec 0028).
#[test]
fn effectful_handler_rejected_in_generic_record_field() {
    assert_row_mismatch(
        "import std.io\n\
         record Box<T> { run: (T) -> T uses {} }\n\
         fn io_inc(x: Int) -> Int uses { Io } {\n\
             let p = Io.print(\"x\\n\")\n\
             x + 1\n\
         }\n\
         fn main() -> Unit uses { Io } {\n\
             let b = Box { run: io_inc }\n\
             ()\n\
         }\n",
    );
}

/// An enum payload is an inference site too: `Wrap(io_inc)` may not smuggle an
/// effectful function into a pure-declared field.
#[test]
fn effectful_handler_rejected_in_enum_payload() {
    assert_row_mismatch(
        "import std.io\n\
         enum Handler {\n\
             Wrap((Int) -> Int uses {})\n\
         }\n\
         fn io_inc(x: Int) -> Int uses { Io } {\n\
             let p = Io.print(\"x\\n\")\n\
             x + 1\n\
         }\n\
         fn main() -> Unit uses { Io } {\n\
             let h = Handler::Wrap(io_inc)\n\
             ()\n\
         }\n",
    );
}

/// And trait-method dispatch: the method's declared parameter row is checked
/// once `Self` is inferred, like a generic call's.
#[test]
fn effectful_handler_rejected_by_trait_method_param() {
    assert_row_mismatch(
        "import std.io\n\
         trait Runner {\n\
             fn run(subject: Self, f: (Int) -> Int uses {}) -> Int\n\
         }\n\
         enum Box {\n\
             One\n\
         }\n\
         impl Runner for Box {\n\
             fn run(subject: Self, f: (Int) -> Int uses {}) -> Int {\n\
                 f(1)\n\
             }\n\
         }\n\
         fn io_inc(x: Int) -> Int uses { Io } {\n\
             let p = Io.print(\"x\\n\")\n\
             x + 1\n\
         }\n\
         fn main() -> Unit uses { Io } {\n\
             let r = Runner.run(Box::One, io_inc)\n\
             ()\n\
         }\n",
    );
}

/// The `throws` channel is closed at the same sites: a throwing function is not
/// acceptable where a non-throwing field is declared (spec 0011/0023).
#[test]
fn throwing_handler_rejected_in_record_field() {
    let output = check_single(
        "record Box { run: (Int) -> Int uses {} }\n\
         fn may_fail(x: Int) -> Int throws String uses {} { throw \"bad\" }\n\
         fn main() -> Unit uses {} {\n\
             let b = Box { run: may_fail }\n\
             ()\n\
         }\n",
    );
    assert!(!output.status.success(), "expected check to fail");
    let err = stderr(&output);
    assert!(
        err.contains("throws String"),
        "unexpected diagnostic:\n{err}"
    );
    assert!(
        err.contains("`try`/`catch`"),
        "unexpected diagnostic:\n{err}"
    );
}

/// Subsumption still runs in the accepting direction at every one of those
/// sites: a pure handler goes into a `uses { Io }` enum payload, and into a
/// trait method's `uses { Io }` parameter.
#[test]
fn pure_handler_accepted_at_every_inference_site() {
    let output = check_single(
        "import std.io\n\
         enum Handler {\n\
             Wrap((Int) -> Int uses { Io })\n\
         }\n\
         trait Runner {\n\
             fn run(subject: Self, f: (Int) -> Int uses { Io }) -> Int uses { Io }\n\
         }\n\
         enum Box {\n\
             One\n\
         }\n\
         impl Runner for Box {\n\
             fn run(subject: Self, f: (Int) -> Int uses { Io }) -> Int uses { Io } {\n\
                 f(1)\n\
             }\n\
         }\n\
         fn pure_inc(x: Int) -> Int uses {} { x + 1 }\n\
         fn main() -> Unit uses { Io } {\n\
             let h = Handler::Wrap(pure_inc)\n\
             let r = Runner.run(Box::One, pure_inc)\n\
             ()\n\
         }\n",
    );
    assert!(
        output.status.success(),
        "expected check to pass:\n{}",
        stderr(&output)
    );
}
