//! End-to-end tests for the derived comparison operators `!= > <= >=`
//! (spec 0027). These operators introduce no new trait: they desugar to the
//! Core Prelude's `Eq.eq` / `Ord.lt` (spec 0020/0021), so a single file needs
//! no import. The tests pin the desugaring shape in the IR (including the
//! left-to-right evaluation order the spec mandates for the swapped forms) and
//! confirm both backends accept the result.

use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> std::path::PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-cmp-test-{}-{id}", std::process::id()));
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

fn main_returning(expr: &str) -> String {
    format!("fn cmp(a: Int, b: Int) -> Bool {{ {expr} }}\nfn main() -> Int uses {{}} {{ 0 }}\n")
}

#[test]
fn not_equal_desugars_to_negated_eq() {
    // `a != b` == `!(a == b)`: an `Eq.eq` call wrapped in a branch to Bool
    // literals (there is no `!` node in the IR).
    let dump = ir(&main_returning("a != b"));
    assert!(
        dump.contains("Eq__Int__eq"),
        "`!=` should route through `Eq.eq`:\n{dump}"
    );
    assert!(
        dump.contains("false") && dump.contains("true"),
        "`!=` should negate via `if ... {{ false }} else {{ true }}`:\n{dump}"
    );
}

#[test]
fn greater_equal_desugars_to_negated_lt() {
    // `a >= b` == `!(a < b)`: no operand swap, so `lt` keeps `(a, b)` order.
    let dump = ir(&main_returning("a >= b"));
    assert!(
        dump.contains("Ord__Int__lt(%a, %b)"),
        "`>=` should negate `a < b` (no swap):\n{dump}"
    );
}

#[test]
fn greater_than_swaps_operands_preserving_evaluation_order() {
    // `a > b` == `b < a`, but the operands must still evaluate a-before-b
    // (spec 0027), so they are bound to temporaries in source order and then
    // compared swapped.
    let dump = ir(&main_returning("a > b"));
    // a is bound first, b second, then `lt(b_tmp, a_tmp)` — the swapped call.
    assert!(
        dump.contains("let __cmp0 = %a") && dump.contains("let __cmp1 = %b"),
        "operands should bind to temporaries in source order:\n{dump}"
    );
    assert!(
        dump.contains("Ord__Int__lt(%__cmp1, %__cmp0)"),
        "`>` should compare the temporaries swapped:\n{dump}"
    );
}

#[test]
fn less_equal_swaps_and_negates() {
    // `a <= b` == `!(b < a)`: swapped like `>`, then negated.
    let dump = ir(&main_returning("a <= b"));
    assert!(
        dump.contains("Ord__Int__lt(%__cmp1, %__cmp0)"),
        "`<=` should compare the temporaries swapped:\n{dump}"
    );
    assert!(
        dump.contains("false") && dump.contains("true"),
        "`<=` should negate the swapped comparison:\n{dump}"
    );
}

#[test]
fn all_comparisons_build_to_js() {
    let source = "fn cmp(a: Int, b: Int) -> Bool {\n  if a > b { a <= b } else { a >= b }\n}\n\
         fn ne(a: Int, b: Int) -> Bool { a != b }\n\
         fn main() -> Int uses {} { 0 }\n";
    let code = js(source);
    // The comparisons never reach the backend as `Binary` nodes; they arrive as
    // the prelude's mangled `lt`/`eq` calls.
    assert!(
        code.contains("Ord__Int__lt") && code.contains("Eq__Int__eq"),
        "comparisons should lower to prelude calls in JS:\n{code}"
    );
}

#[test]
fn all_comparisons_build_to_wasm() {
    build_wasm_ok(
        "fn cmp(a: Int, b: Int) -> Bool {\n  if a > b { a <= b } else { a >= b }\n}\n\
         fn ne(a: Float, b: Float) -> Bool { a != b }\n\
         fn main() -> Int uses {} { 0 }\n",
    );
}

#[test]
fn comparison_is_lower_precedence_than_arithmetic() {
    // `a + 1 > b` must parse as `(a + 1) > b`, not `a + (1 > b)` (which would be
    // an `Int + Bool` type error). If it type-checks, precedence is correct.
    let dump = ir(&main_returning("a + 1 > b"));
    assert!(
        dump.contains("Add__Int__add") && dump.contains("Ord__Int__lt"),
        "`a + 1 > b` should add first, then compare:\n{dump}"
    );
}

// A user type that implements only `Ord` (spec 0020) gains `> <= >=`.
const ORD_PRIO: &str = "\
enum Prio {
    At(Int)
}
impl Ord for Prio {
    fn lt(a: Prio, b: Prio) -> Bool uses {} {
        match a {
            At(x) -> match b {
                At(y) -> x < y
            }
        }
    }
}
";

#[test]
fn greater_than_works_on_user_ord_type() {
    let source = format!(
        "{ORD_PRIO}fn higher(a: Prio, b: Prio) -> Bool {{ a > b }}\n\
         fn main() -> Int uses {{}} {{ 0 }}\n"
    );
    let dump = ir(&source);
    assert!(
        dump.contains("Ord__Prio__lt"),
        "`>` on a user `Ord` type should dispatch to its impl:\n{dump}"
    );
}

#[test]
fn rejects_greater_than_on_non_ord() {
    // A user type with no `Ord` instance rejects `>` exactly as `<` is rejected.
    // (`String` has an `Ord` instance now, spec 0027, so it is no longer the
    // example — a bare user enum is.)
    let err = check_err(
        "enum Color { Red\n  Green }\nfn main() -> Bool uses {} {\n  Color::Red > Color::Green\n}\n",
    );
    assert!(
        err.contains("Ord"),
        "`>` on a non-Ord type should complain about `Ord`:\n{err}"
    );
}

#[test]
fn rejects_less_equal_on_bool() {
    // `Bool` has an `Eq` instance but no `Ord` one, so the ordering operators
    // are still rejected.
    let err = check_err("fn main() -> Bool uses {} {\n  true <= false\n}\n");
    assert!(
        err.contains("Ord"),
        "`<=` on Bool should complain about `Ord`:\n{err}"
    );
}

#[test]
fn bool_equality_dispatches_to_prelude_eq() {
    // `Bool` has a Core Prelude `Eq` instance (spec 0027), so `==` works on it.
    let dump = ir("fn f(a: Bool, b: Bool) -> Bool { a == b }\nfn main() -> Int uses {} { 0 }\n");
    assert!(
        dump.contains("Eq__Bool__eq"),
        "`==` on Bool should dispatch to the prelude `Eq for Bool`:\n{dump}"
    );
}

#[test]
fn bool_inequality_builds() {
    // `a != b` on Bool desugars to `!(a == b)` and reaches both backends.
    build_wasm_ok("fn f(a: Bool, b: Bool) -> Bool { a != b }\nfn main() -> Int uses {} { 0 }\n");
}

#[test]
fn string_has_eq_and_ord() {
    // `String` has Core Prelude `Eq` and `Ord` instances (spec 0027), so all the
    // comparisons work and bottom out in the string-comparison intrinsics.
    let dump = ir("fn f(a: String, b: String) -> Bool { a == b }\n\
                   fn g(a: String, b: String) -> Bool { a < b }\n\
                   fn main() -> Int uses {} { 0 }\n");
    assert!(
        dump.contains("Eq__String__eq"),
        "`==` on String should dispatch to the prelude `Eq for String`:\n{dump}"
    );
    assert!(
        dump.contains("Ord__String__lt"),
        "`<` on String should dispatch to the prelude `Ord for String`:\n{dump}"
    );
    assert!(
        dump.contains("string_eq") && dump.contains("string_lt"),
        "String comparison should bottom out in the string intrinsics:\n{dump}"
    );
}

#[test]
fn string_comparison_builds_to_wasm() {
    // Exercises the wasm string-comparison runtime helper: every ordering
    // operator reduces to `string_eq` / `string_lt`.
    build_wasm_ok(
        "fn f(a: String, b: String) -> Bool { a != b }\n\
                   fn g(a: String, b: String) -> Bool { a >= b }\n\
                   fn h(a: String, b: String) -> Bool { a > b }\n\
                   fn main() -> Int uses {} { 0 }\n",
    );
}
