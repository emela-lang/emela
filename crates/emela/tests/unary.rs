//! End-to-end tests for the unary negation operator `-e`. Unlike the binary
//! operator traits (spec 0020), which dispatch generically to any
//! user impl (`impl Add for Money`, see `traits.rs`), `-e` desugars to
//! `Sub.sub(zero, e)` with a compiler-synthesized zero literal — so it only
//! applies to `Int`/`Float`, the two types with a literal zero. A user type
//! that implements `Sub` but is not `Int`/`Float` must be rejected with a
//! diagnostic at type-check time rather than reach lowering, which has no
//! generic zero to emit.

use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> std::path::PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-unary-test-{}-{id}", std::process::id()));
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

/// Compiles `expr` (an `Int`) into a `main` that prints it and executes the
/// module under `emela run` (the wasm-wasi backend via wasmi), returning stdout.
fn run_int(expr: &str) -> String {
    let source =
        format!("import std.io\n\nfn main() -> Unit uses {{ Io }} {{\n    Io.print({expr})\n}}\n");
    let output = run(&["run"], &source);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// The same `expr` compiled to the js-node backend and executed with `node`,
/// so the two backends can be checked to agree (spec 0052 parity).
fn node_int(expr: &str) -> String {
    let dir = temp_dir();
    let input = dir.join("main.emel");
    let js_path = dir.join("out.js");
    let source =
        format!("import std.io\n\nfn main() -> Unit uses {{ Io }} {{\n    Io.print({expr})\n}}\n");
    fs::write(&input, source).unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_emela"))
        .args(["build", "--backend", "js-node", "-o"])
        .arg(&js_path)
        .arg(&input)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let node = Command::new("node").arg(&js_path).output().unwrap();
    let _ = fs::remove_dir_all(&dir);
    assert!(
        node.status.success(),
        "{}",
        String::from_utf8_lossy(&node.stderr)
    );
    String::from_utf8(node.stdout).unwrap()
}

#[test]
fn desugars_to_sub_with_zero_literal() {
    let dump = ir("fn f(a: Int) -> Int uses {} { - a }\nfn main() -> Int uses {} { 0 }\n");
    assert!(
        dump.contains("Sub__Int__sub(0, %a)"),
        "`-a` should desugar to `Sub.sub(0, a)`:\n{dump}"
    );
}

#[test]
fn desugars_to_sub_with_zero_literal_for_float() {
    let dump = ir("fn f(a: Float) -> Float uses {} { - a }\nfn main() -> Int uses {} { 0 }\n");
    assert!(
        dump.contains("Sub__Float__sub(0, %a)"),
        "`-a` on a `Float` should desugar to `Sub.sub(0, a)`:\n{dump}"
    );
}

#[test]
fn binds_tighter_than_binary_addition() {
    // `-a + b` parses as `(-a) + b`, not `-(a + b)`: `parse_unary` sits inside
    // `parse_product`, tighter than the `+`/`-` level.
    let dump =
        ir("fn f(a: Int, b: Int) -> Int uses {} { - a + b }\nfn main() -> Int uses {} { 0 }\n");
    assert!(
        dump.contains("Add__Int__add(call @Sub__Int__sub(0, %a), %b)"),
        "`-a + b` should be `(-a) + b`:\n{dump}"
    );
}

#[test]
fn double_negation_parses_recursively() {
    assert_eq!(run_int("- -5").trim(), "5");
    assert_eq!(node_int("- -5").trim(), "5");
}

#[test]
fn runtime_values_match_on_both_backends() {
    let expr = "(- 5) + (- (0 - 3)) + (if - 4 == 0 - 4 { 100 } else { 0 })";
    assert_eq!(run_int(expr).trim(), "98");
    assert_eq!(node_int(expr).trim(), "98");
}

#[test]
fn rejects_types_without_a_literal_zero_even_with_a_sub_impl() {
    // `Money` implements `Sub` (so binary `a - b` on `Money` works, same as
    // `traits.rs`'s `impl Add for Money`), but the compiler has no generic
    // zero to synthesize for `-money`, so it must be rejected at type-check
    // time with a diagnostic — not reach lowering and panic.
    let source = "\
enum Money {
    Cents(Int)
}
impl Sub for Money {
    fn sub(a: Money, b: Money) -> Money uses {} {
        match a {
            Cents(x) -> match b {
                Cents(y) -> Money::Cents(x - y)
            }
        }
    }
}
fn neg_money(m: Money) -> Money { - m }
fn main() -> Int uses {} { 0 }
";
    let err = check_err(source);
    assert!(
        err.contains("Unsupported type for unary"),
        "expected a diagnostic rejecting `-` on `Money`:\n{err}"
    );
}

#[test]
fn rejects_string() {
    let err =
        check_err("fn f(s: String) -> String uses {} { - s }\nfn main() -> Int uses {} { 0 }\n");
    assert!(
        err.contains("Unsupported type for unary"),
        "expected a diagnostic rejecting `-` on `String`:\n{err}"
    );
}
