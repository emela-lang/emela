//! End-to-end tests for user-defined generic records (spec 0028): declaring
//! `record Pair<A, B>` / `record Box<T>`, constructing values with type
//! arguments inferred from the field values, accessing fields whose type is a
//! substituted parameter, and using them through generic functions that
//! monomorphize per instantiation. Complements `generic_enums.rs`, which covers
//! the enum half of the same spec.

use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> std::path::PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-grec-test-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(args: &[&str], source: &str) -> std::process::Output {
    let dir = temp_dir();
    let input = dir.join("main.emel");
    fs::write(&input, source).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_emela"));
    for arg in args {
        command.arg(arg);
    }
    let output = command.arg(&input).output().unwrap();
    let _ = fs::remove_dir_all(&dir);
    output
}

fn stdout_of(source: &str) -> String {
    let output = run(&["run"], source);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn check_err(source: &str) -> String {
    let output = run(&["check"], source);
    assert!(
        !output.status.success(),
        "expected check to fail, but it passed"
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn ir(source: &str) -> String {
    let output = run(&["ir"], source);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn js(source: &str) -> String {
    let output = run(&["build", "--backend", "js-node"], source);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn build_wasm_ok(source: &str) {
    let dir = temp_dir();
    let input = dir.join("main.emel");
    let output_path = dir.join("out.wasm");
    fs::write(&input, source).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("build")
        .arg("--backend")
        .arg("wasm-wasi")
        .arg("-o")
        .arg(&output_path)
        .arg(&input)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let bytes = fs::read(&output_path).unwrap();
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(&bytes[0..4], b"\0asm");
}

const PAIR: &str = "\
record Pair<A, B> {
    first: A
    second: B
}
";

#[test]
fn infers_type_arguments_from_fields_without_annotation() {
    // `Pair { first: 1, second: \"one\" }` infers `Pair<Int, String>` from the
    // field values (spec 0028); no annotation is needed for the binding, and
    // `q.second` is the substituted `Int`.
    let out = stdout_of(&format!(
        "import std.io\n{PAIR}\
         fn main() -> Unit uses {{ Io }} {{\n  \
         let p = Pair {{ first: 1, second: \"one\" }}\n  \
         Io.print(p.second)\n  Io.print(\"\\n\")\n}}\n"
    ));
    assert_eq!(out, "one\n");
}

#[test]
fn swap_reverses_type_parameters() {
    // The spec 0028 example: `swap<A, B>(Pair<A, B>) -> Pair<B, A>`. The field
    // access `p.second` yields the concrete `B`, and the returned record's type
    // is `Pair<String, Int>`.
    let source = format!(
        "import std.io\n{PAIR}\
         fn swap<A, B>(p: Pair<A, B>) -> Pair<B, A> {{\n  \
         Pair {{ first: p.second, second: p.first }}\n}}\n\
         fn main() -> Unit uses {{ Io }} {{\n  \
         let p = Pair {{ first: 1, second: \"one\" }}\n  \
         let q = swap(p)\n  Io.print(q.first)\n  Io.print(\"\\n\")\n}}\n"
    );
    assert_eq!(stdout_of(&source), "one\n");
}

#[test]
fn monomorphizes_generic_record_function() {
    // A generic function over a generic record specializes per instantiation
    // and the typed IR holds no type variable (spec 0012/0028).
    let dump = ir(&format!(
        "{PAIR}\
         fn first<A, B>(p: Pair<A, B>) -> A {{ p.first }}\n\
         fn main() -> Int uses {{}} {{\n  \
         let p = Pair {{ first: 7, second: true }}\n  first(p)\n}}\n"
    ));
    assert!(
        dump.contains("first__Int__Bool"),
        "missing the `Pair<Int, Bool>` specialization of `first`:\n{dump}"
    );
    assert!(
        !dump.contains("Var("),
        "a type variable leaked into the IR:\n{dump}"
    );
}

#[test]
fn two_instantiations_specialize_separately() {
    // The same generic function used at `Box<Int>` and `Box<Bool>` produces two
    // specializations (spec 0028/0014).
    let dump = ir("record Box<T> { value: T }\n\
         fn unwrap<T>(b: Box<T>) -> T { b.value }\n\
         fn main() -> Int uses {} {\n  \
         let a = Box { value: 1 }\n  \
         let b = Box { value: true }\n  \
         let _ = unwrap(b)\n  unwrap(a)\n}\n");
    assert!(
        dump.contains("unwrap__Int") && dump.contains("unwrap__Bool"),
        "expected both `unwrap__Int` and `unwrap__Bool`:\n{dump}"
    );
}

#[test]
fn recursive_generic_record_via_option() {
    // A generic record referring to itself through `Option<Node<T>>` (spec 0028):
    // finite because the field is a reference. Builds and runs.
    let out = stdout_of(
        "import std.io\n\
         record Node<T> {\n    value: T\n    next: Option<Node<T>>\n}\n\
         fn head<T>(n: Node<T>) -> T { n.value }\n\
         fn main() -> Unit uses { Io } {\n  \
         let leaf = Node { value: 2, next: None }\n  \
         let root = Node { value: 1, next: Some(leaf) }\n  \
         Io.print(head(root))\n  Io.print(\"\\n\")\n}\n",
    );
    assert_eq!(out, "1\n");
}

#[test]
fn reordered_generic_literal_infers_and_runs() {
    // Writing the fields out of declaration order still infers the type
    // arguments and stores in declaration order (spec 0003/0028).
    let out = stdout_of(&format!(
        "import std.io\n{PAIR}\
         fn main() -> Unit uses {{ Io }} {{\n  \
         let p = Pair {{ second: \"two\", first: 1 }}\n  \
         Io.print(p.second)\n  Io.print(\"\\n\")\n}}\n"
    ));
    assert_eq!(out, "two\n");
}

#[test]
fn generic_record_builds_to_wasm() {
    build_wasm_ok(&format!(
        "{PAIR}\
         fn first<A, B>(p: Pair<A, B>) -> A {{ p.first }}\n\
         fn main() -> Int {{\n  \
         let p = Pair {{ first: 5, second: true }}\n  first(p)\n}}\n"
    ));
}

#[test]
fn generic_record_builds_on_js() {
    let code = js(&format!(
        "{PAIR}\
         fn first<A, B>(p: Pair<A, B>) -> A {{ p.first }}\n\
         fn main() -> Int {{\n  \
         let p = Pair {{ first: 5, second: true }}\n  first(p)\n}}\n"
    ));
    assert!(code.contains("function first"), "{code}");
}

#[test]
fn rejects_wrong_number_of_type_arguments() {
    let err = check_err(
        "record Pair<A, B> { first: A, second: B }\n\
         fn f(p: Pair<Int>) -> Int { 0 }\n\
         fn main() -> Int uses {} { 0 }\n",
    );
    assert!(
        err.contains("type argument"),
        "wrong arity should be reported:\n{err}"
    );
}

#[test]
fn rejects_bound_on_record_type_parameter() {
    // Data types cannot carry bounds (spec 0028): the requirement belongs on the
    // functions/impls that use the type.
    let err = check_err("record Bad<T: Show> { value: T }\nfn main() -> Int uses {} { 0 }\n");
    assert!(
        err.to_lowercase().contains("bound"),
        "a bound on a record type parameter should be rejected:\n{err}"
    );
}
