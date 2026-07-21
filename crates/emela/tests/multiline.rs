//! End-to-end tests for spec 0034: newlines are insignificant inside `(...)`
//! and `[...]` (a `{` frame restores significance), and every comma-separated
//! list accepts a trailing comma. These are the grammar prerequisites for the
//! canonical formatter's forced wrapping (spec 0035).

use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> std::path::PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("emela-multiline-test-{}-{id}", std::process::id()));
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

/// `main`'s `Int` result is the process exit code, so an expression's runtime
/// value can be asserted directly.
fn run_exit_code(source: &str) -> i32 {
    let output = run(&["run"], source);
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.status.code().unwrap()
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

#[test]
fn multiline_call_arguments() {
    check_ok(
        "fn add(x: Int, y: Int) -> Int {\n  x + y\n}\n\nfn main() -> Unit uses {} {\n  let s = add(\n    1,\n    2\n  )\n  ()\n}\n",
    );
}

#[test]
fn multiline_params_with_trailing_comma() {
    check_ok(
        "fn add(\n  x: Int,\n  y: Int,\n) -> Int {\n  x + y\n}\n\nfn main() -> Unit uses {} {\n  let s = add(1, 2,)\n  ()\n}\n",
    );
}

#[test]
fn multiline_array_with_trailing_comma() {
    check_ok("fn main() -> Unit uses {} {\n  let xs = [\n    1,\n    2,\n    3,\n  ]\n  ()\n}\n");
}

#[test]
fn trailing_comma_in_effect_row_and_type_args() {
    check_ok(
        "effect Stdout {\n  pub fn emit() -> Unit {\n    ()\n  }\n}\n\nenum Pair<A, B,> {\n  Both(A, B,)\n}\n\nfn quiet() -> Unit uses { Stdout, } {\n  ()\n}\n\nfn main() -> Unit uses {} {\n  let p = Pair::Both(1, 2)\n  let q: Pair<Int, Int,> = p\n  ()\n}\n",
    );
}

#[test]
fn trailing_comma_in_match_pattern_fields() {
    check_ok(
        "enum Pair {\n  Both(Int, Int)\n}\n\nfn first(p: Pair) -> Int {\n  match p {\n    Both(a, b,) -> a\n  }\n}\n\nfn main() -> Unit uses {} {\n  let x = first(Pair::Both(1, 2))\n  ()\n}\n",
    );
}

// Inside `foo(match x { ... })` the `{` frame restores newline significance:
// the arms are still newline-separated even though the enclosing parens
// suppress newlines.
#[test]
fn braces_inside_parens_keep_newline_significance() {
    check_ok(
        "enum Color {\n  Red\n  Blue\n}\n\nfn pick(n: Int) -> Color {\n  if n > 0 { Color::Red } else { Color::Blue }\n}\n\nfn name(c: Color) -> String {\n  match c {\n    Red -> \"red\"\n    Blue -> \"blue\"\n  }\n}\n\nfn main() -> Unit uses {} {\n  let s = name(\n    match pick(1) {\n      Red -> Color::Red\n      Blue -> Color::Blue\n    }\n  )\n  ()\n}\n",
    );
}

#[test]
fn multiline_lambda_argument() {
    check_ok(
        "fn apply(f: (Int) -> Int, x: Int) -> Int {\n  f(x)\n}\n\nfn main() -> Unit uses {} {\n  let r = apply(\n    fn (x: Int) -> Int {\n      let doubled = x * 2\n      doubled + 1\n    },\n    5,\n  )\n  ()\n}\n",
    );
}

// A comma with no element before the closer is still an error: the list
// grammar requires an element before each comma (spec 0034 T2).
#[test]
fn lone_comma_is_rejected() {
    check_err("fn f() -> Int {\n  g(,)\n}\n");
}

// A statement boundary still cannot be replaced by a comma, and newlines at
// the top level still separate items: a signature split outside any bracket
// remains an error.
#[test]
fn newline_outside_brackets_still_significant() {
    check_err("fn f()\n -> Int {\n  1\n}\n\nfn main() -> Unit uses {} {\n  ()\n}\n");
}

// A binary operator at the start of the next line continues the current
// expression (issue #62): no operator can begin a statement (the language has
// no prefix `+`/`-`), so a leading operator is unambiguously a continuation.
// `1 + 2 + 3 * 4` == `1 + 2 + (3 * 4)` == 15 confirms precedence is preserved
// across the newlines.
#[test]
fn leading_binary_operator_continues_expression() {
    assert_eq!(
        run_exit_code("fn main() -> Int {\n  1 + 2\n  + 3\n  * 4\n}\n"),
        15,
    );
}

// Comparison and short-circuiting operators continue across newlines too:
// `(1 < 2) && (3 > 2) || false` is `true`.
#[test]
fn leading_comparison_and_logical_operators_continue() {
    let source = "fn main() -> Int {\n  let ok = 1 < 2\n    && 3 > 2\n    || false\n  if ok { 7 } else { 0 }\n}\n";
    assert_eq!(run_exit_code(source), 7);
}

// The continuation must not swallow a genuine statement boundary: two
// expression statements on separate lines stay separate, so the block's value
// is the *last* one. If the newline were wrongly absorbed, `id(1)` would be
// applied to `id(2)` and fail to type-check.
#[test]
fn adjacent_statements_are_not_merged() {
    assert_eq!(
        run_exit_code("fn id(x: Int) -> Int {\n  x\n}\n\nfn main() -> Int {\n  id(1)\n  id(2)\n}\n"),
        2,
    );
}
