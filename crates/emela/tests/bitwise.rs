//! End-to-end tests for the bitwise operators `& | ^ ~ << >> >>>` (spec 0053).
//! The binary operators are operator traits (spec 0020) instanced for `Int` in
//! the Core Prelude, so a single file needs no import; `~e` desugars to `e ^ -1`
//! (no dedicated trait). The tests pin the desugaring shape in the IR, the
//! precedence relative to comparison, the coexistence of `>>` with nested
//! generics, and the runtime values on both backends.

use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> std::path::PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-bit-test-{}-{id}", std::process::id()));
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
fn binary_operators_route_through_their_trait_impl() {
    // Each binary bitwise operator desugars to its own operator trait's `Int`
    // impl (spec 0053), whose body bottoms out in the matching intrinsic.
    let dump = ir("fn f(a: Int, b: Int) -> Int uses {} {\n\
         a & b | a ^ b\n\
        }\nfn main() -> Int uses {} { 0 }\n");
    for method in [
        "BitAnd__Int__bitand",
        "BitOr__Int__bitor",
        "BitXor__Int__bitxor",
    ] {
        assert!(dump.contains(method), "missing `{method}` in:\n{dump}");
    }
    for intr in ["i32_and", "i32_or", "i32_xor"] {
        assert!(
            dump.contains(intr),
            "missing intrinsic `{intr}` in:\n{dump}"
        );
    }
}

#[test]
fn shifts_distinguish_arithmetic_from_logical() {
    // `>>` is the arithmetic shift (`i32_shr_s`); `>>>` the logical shift
    // (`i32_shr_u`); `<<` is `i32_shl` (spec 0053).
    let dump = ir("fn f(a: Int) -> Int uses {} {\n\
         (a << 1) + (a >> 1) + (a >>> 1)\n\
        }\nfn main() -> Int uses {} { 0 }\n");
    assert!(dump.contains("i32_shl"), "missing `i32_shl`:\n{dump}");
    assert!(dump.contains("i32_shr_s"), "missing `i32_shr_s`:\n{dump}");
    assert!(dump.contains("i32_shr_u"), "missing `i32_shr_u`:\n{dump}");
}

#[test]
fn tilde_desugars_to_xor_with_all_ones() {
    // `~e ≡ e ^ -1` (spec 0053 U1): no dedicated trait, it reuses `BitXor` with
    // an all-ones literal as the right operand.
    let dump = ir("fn f(a: Int) -> Int uses {} { ~a }\nfn main() -> Int uses {} { 0 }\n");
    assert!(
        dump.contains("BitXor__Int__bitxor(%a, -1)"),
        "`~a` should desugar to `a ^ -1`:\n{dump}"
    );
}

#[test]
fn bitwise_binds_tighter_than_comparison() {
    // Spec 0053 precedence: `x & 1 == 0` parses as `(x & 1) == 0`, so the `Eq.eq`
    // call takes the `BitAnd` result — not `x` and `(1 == 0)`.
    let dump = ir("fn f(x: Int) -> Bool uses {} { x & 1 == 0 }\nfn main() -> Int uses {} { 0 }\n");
    assert!(
        dump.contains("BitAnd__Int__bitand(%x, 1)"),
        "`&` should bind tighter than `==`:\n{dump}"
    );
}

#[test]
fn shift_coexists_with_nested_generics() {
    // `>>` lexes as one token but must not break the nested-generic close of
    // `Array<Array<Int>>` (spec 0053 字句), which the type parser splits back
    // into two `>`.
    let output = run(
        &["check"],
        "fn flatten(xss: Array<Array<Int>>) -> Int uses {} {\n\
             array_length(xss) >> 0\n\
            }\nfn main() -> Int uses {} { 0 }\n",
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn bitwise_is_int_only() {
    // Spec 0053 B1: no `Float` instance, so `Float & Float` fails the bound just
    // like `Float % Float`.
    let err = check_err("fn f(a: Float, b: Float) -> Float uses {} { a & b }\n");
    assert!(
        err.contains("BitAnd"),
        "expected an unsatisfied `BitAnd` bound:\n{err}"
    );
}

#[test]
fn runtime_values_match_on_both_backends() {
    // A single expression exercising AND/OR/XOR, both shifts, `~`, and the
    // precedence of `&` over `==`. `0 - 16` stands in for the missing negative
    // literal. wasm and js must agree (spec 0052 parity).
    // 8 + 14 + 6 + 16 + (-4) + 15 + (-1) + 100 = 154.
    let expr = "(12 & 10) + (12 | 10) + (12 ^ 10) + (1 << 4) \
        + ((0 - 16) >> 2) + ((0 - 16) >>> 28) + (~0) \
        + (if 5 & 1 == 1 { 100 } else { 0 })";
    assert_eq!(run_int(expr).trim(), "154");
    assert_eq!(node_int(expr).trim(), "154");
}
