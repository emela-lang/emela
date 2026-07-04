//! End-to-end tests for the logical operators `&& || !` (spec 0027). These are
//! Bool-only, short-circuiting language built-ins — not trait methods — so the
//! frontend desugars them to `if` (spec 0015): `a && b` == `if a { b } else
//! { false }`, `a || b` == `if a { true } else { b }`, `!e` == `if e { false }
//! else { true }`. The tests pin that desugaring, the precedence relative to the
//! comparisons, and the Bool-only typing.

use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> std::path::PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-logic-test-{}-{id}", std::process::id()));
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

fn check_ok(source: &str) {
    let output = run(&["check"], source);
    assert!(
        output.status.success(),
        "expected check to pass, but it failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
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

fn bool_fn(expr: &str) -> String {
    format!("fn f(a: Bool, b: Bool) -> Bool {{ {expr} }}\nfn main() -> Int uses {{}} {{ 0 }}\n")
}

#[test]
fn and_desugars_to_short_circuit_if() {
    // `a && b` == `if a { b } else { false }`: `b` is only evaluated when `a`.
    let dump = ir(&bool_fn("a && b"));
    assert!(
        dump.contains("if %a {") && dump.contains("%b") && dump.contains("else { false }"),
        "`&&` should desugar to `if a {{ b }} else {{ false }}`:\n{dump}"
    );
}

#[test]
fn or_desugars_to_short_circuit_if() {
    // `a || b` == `if a { true } else { b }`: `b` is only evaluated when `!a`.
    let dump = ir(&bool_fn("a || b"));
    assert!(
        dump.contains("if %a { true } else {") && dump.contains("%b"),
        "`||` should desugar to `if a {{ true }} else {{ b }}`:\n{dump}"
    );
}

#[test]
fn not_desugars_to_if() {
    // `!a` == `if a { false } else { true }`; there is no operator trait for it.
    let dump = ir("fn f(a: Bool) -> Bool { !a }\nfn main() -> Int uses {} { 0 }\n");
    assert!(
        dump.contains("if %a { false } else { true }"),
        "`!a` should desugar to `if a {{ false }} else {{ true }}`:\n{dump}"
    );
}

#[test]
fn double_negation_parses() {
    check_ok("fn f(a: Bool) -> Bool { !!a }\nfn main() -> Int uses {} { 0 }\n");
}

#[test]
fn comparison_binds_tighter_than_logical() {
    // `n < 0 || n > 10` must parse as `(n < 0) || (n > 10)`. If `||` bound tighter
    // than `<`, it would try `||` on Int and fail to type-check.
    check_ok("fn f(n: Int) -> Bool { n < 0 || n > 10 }\nfn main() -> Int uses {} { 0 }\n");
}

#[test]
fn and_binds_tighter_than_or() {
    // `a || b && c` == `a || (b && c)`, so `||` is the outermost node and its
    // condition is `a` directly (not a nested `||`).
    let dump = ir("fn f(a: Bool, b: Bool, c: Bool) -> Bool { a || b && c }\n\
            fn main() -> Int uses {} { 0 }\n");
    assert!(
        dump.contains("if %a { true } else {"),
        "`||` should be outermost with `a` as its condition:\n{dump}"
    );
}

#[test]
fn not_composes_with_comparison() {
    // `!` applies to its parenthesized operand: `!(a < b)` negates the `Ord`
    // comparison. (`Bool` has no `Eq` instance yet, so `!a == b` is a separate,
    // unsupported case — spec 0027 open question.)
    check_ok("fn f(a: Int, b: Int) -> Bool { !(a < b) }\nfn main() -> Int uses {} { 0 }\n");
}

#[test]
fn logical_ops_build_to_js() {
    // Short-circuit survives to JS as a ternary (only one branch is evaluated).
    let code = js(&bool_fn("a && b || !a"));
    assert!(
        code.contains('?') && code.contains(':'),
        "logical operators should lower to branching JS:\n{code}"
    );
}

#[test]
fn logical_ops_build_to_wasm() {
    build_wasm_ok(&bool_fn("a && b || !a"));
}

#[test]
fn rejects_non_bool_and() {
    let err = check_err("fn main() -> Bool uses {} {\n  1 && true\n}\n");
    assert!(
        err.contains("Bool"),
        "`&&` on a non-Bool operand should require Bool:\n{err}"
    );
}

#[test]
fn rejects_not_on_int() {
    let err = check_err("fn main() -> Bool uses {} {\n  !5\n}\n");
    assert!(
        err.contains("Bool"),
        "`!` on Int should require Bool:\n{err}"
    );
}
