//! End-to-end tests for the pipeline operator `|>` (spec 0019). `|>` is a pure
//! syntactic desugaring performed in the parser: `lhs |> f(a, b)` becomes the
//! ordinary call `f(lhs, a, b)` (first-argument insertion), a bare right side
//! `lhs |> f` becomes `f(lhs)`, and a trailing `?` applies after insertion so
//! `lhs |> g?` is `(lhs |> g)?`. Because it lowers to a plain `Call`, no later
//! stage (typed IR, backends) sees a pipe node — these tests pin the desugaring,
//! its lowest-precedence left-associativity, and that the compiled program runs.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-pipe-test-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(args: &[&str], source: &str) -> Output {
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

/// `main`'s `Int` result becomes the process exit code, so a pipeline's runtime
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

/// Formats `source` and returns the output. The formatter works on tokens, so it
/// is the component that must know `|>` renders with a space on each side.
fn fmt(source: &str) -> String {
    let dir = temp_dir();
    let input = dir.join("main.emel");
    fs::write(&input, source).unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("fmt")
        .arg(&input)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let formatted = fs::read_to_string(&input).unwrap();
    let _ = fs::remove_dir_all(&dir);
    formatted
}

const HELPERS: &str = "\
fn add1(x: Int) -> Int { x + 1 }\n\
fn double(x: Int) -> Int { x * 2 }\n\
fn scale(x: Int, factor: Int) -> Int { x * factor }\n";

/// P2/P3: a chain of bare and one-arg pipes lowers to nested calls and, at run
/// time, equals the spec's example `scale(double(add1(21)), 1) == 44`.
#[test]
fn chain_runs_to_spec_example() {
    let source = format!("{HELPERS}fn main() -> Int {{ 21 |> add1 |> double |> scale(1) }}\n");
    assert_eq!(run_exit_code(&source), 44);
}

/// A pipe desugars to exactly the call a reader would write by hand: the two
/// functions produce identical IR.
#[test]
fn desugars_to_the_same_ir_as_the_explicit_call() {
    let source = format!(
        "{HELPERS}\
        fn piped(n: Int) -> Int {{ n |> add1 |> scale(2) }}\n\
        fn explicit(n: Int) -> Int {{ scale(add1(n), 2) }}\n\
        fn main() -> Int uses {{}} {{ 0 }}\n"
    );
    let dump = ir(&source);
    let body_of = |name: &str| -> String {
        dump.split(&format!("fn {name}("))
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .unwrap()
            .to_string()
    };
    assert_eq!(
        body_of("piped"),
        body_of("explicit"),
        "`|>` must produce the same IR as the hand-written call:\n{dump}"
    );
}

/// P1: `|>` is the weakest operator, so `a + b |> f` is `f(a + b)` — if `|>`
/// bound tighter than `+`, `b |> f` would be grouped instead.
#[test]
fn is_lower_precedence_than_arithmetic() {
    let source = format!(
        "{HELPERS}fn go(a: Int, b: Int) -> Int {{ a + b |> add1 }}\n\
        fn main() -> Int uses {{}} {{ go(1, 2) }}\n"
    );
    let dump = ir(&source);
    assert!(
        dump.contains("call @add1(call @Add__Int__add("),
        "`a + b |> f` must parse as `f(a + b)`:\n{dump}"
    );
    // (1 + 2) |> add1 == add1(3) == 4.
    assert_eq!(run_exit_code(&source), 4);
}

/// P1: left-associative, so `a |> f |> g` is `g(f(a))`, with `g` outermost.
#[test]
fn is_left_associative() {
    let source = format!(
        "{HELPERS}fn go(n: Int) -> Int {{ n |> add1 |> double }}\n\
        fn main() -> Int uses {{}} {{ go(10) }}\n"
    );
    let dump = ir(&source);
    assert!(
        dump.contains("call @double(call @add1(%n))"),
        "`a |> f |> g` must nest as `g(f(a))`:\n{dump}"
    );
    // double(add1(10)) == 22.
    assert_eq!(run_exit_code(&source), 22);
}

/// P2: the left side is inserted as the *first* argument, before the written
/// ones. `x |> scale(3)` is `scale(x, 3)`, not `scale(3, x)`.
#[test]
fn inserts_left_side_as_first_argument() {
    let source = format!(
        "{HELPERS}fn go(x: Int) -> Int {{ x |> scale(3) }}\n\
        fn main() -> Int uses {{}} {{ go(7) }}\n"
    );
    let dump = ir(&source);
    assert!(
        dump.contains("call @scale(%x, 3)"),
        "`x |> scale(3)` must insert `x` first: `scale(x, 3)`:\n{dump}"
    );
    // scale(7, 3) == 21.
    assert_eq!(run_exit_code(&source), 21);
}

/// P3: a right side that is a function *value* (not a call) applies to the left
/// side: `x |> h` is `h(x)`.
#[test]
fn bare_function_value_is_applied() {
    let source = format!("{HELPERS}fn main() -> Int {{\n  let h = double\n  10 |> h\n}}\n");
    // double(10) == 20.
    assert_eq!(run_exit_code(&source), 20);
}

/// P4: a trailing `?` applies *after* insertion, so `x |> validate? |> transform?`
/// is `transform(validate(x)?)?` — identical IR to writing it out.
#[test]
fn trailing_question_applies_after_insertion() {
    let prelude = "\
enum E {\n  Bad\n}\n\
fn validate(x: Int) -> Int throws E { if x < 0 { throw E::Bad } else { x } }\n\
fn transform(x: Int) -> Int throws E { x + 100 }\n";
    let source = format!(
        "{prelude}\
        fn piped(x: Int) -> Int throws E {{ x |> validate? |> transform? }}\n\
        fn explicit(x: Int) -> Int throws E {{ transform(validate(x)?)? }}\n\
        fn main() -> Int {{\n  try {{\n    piped(5)\n  }} catch {{\n    Bad -> 7\n  }}\n}}\n"
    );
    let dump = ir(&source);
    let body_of = |name: &str| -> String {
        dump.split(&format!("fn {name}("))
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .unwrap()
            .to_string()
    };
    assert_eq!(
        body_of("piped"),
        body_of("explicit"),
        "`|> g?` must desugar to `(|> g)?`:\n{dump}"
    );
    // validate(5) == 5, transform(5) == 105, no error thrown.
    assert_eq!(run_exit_code(&source), 105);
    // The failing path propagates through both `?`s to the `catch`. (`0 - 1`
    // rather than a `-1` literal: the language has no prefix minus.)
    let failing = source.replace("piped(5)", "piped(0 - 1)");
    assert_eq!(run_exit_code(&failing), 7);
}

/// A pipeline may place `|>` at the start of each continuation line (spec 0034,
/// issue #62): a newline before a binary operator continues the expression. The
/// multi-line form desugars to the very same IR as the single-line chain.
#[test]
fn leading_operator_spans_newlines() {
    let multiline = format!(
        "{HELPERS}\
        fn piped(n: Int) -> Int {{\n  n\n  |> add1\n  |> double\n  |> scale(1)\n}}\n\
        fn inline(n: Int) -> Int {{ n |> add1 |> double |> scale(1) }}\n\
        fn main() -> Int uses {{}} {{ 0 }}\n"
    );
    let dump = ir(&multiline);
    let body_of = |name: &str| -> String {
        dump.split(&format!("fn {name}("))
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .unwrap()
            .to_string()
    };
    assert_eq!(
        body_of("piped"),
        body_of("inline"),
        "a `|>` on the next line must parse as one pipeline:\n{dump}"
    );
    // double(add1(21)) |> scale(1) == scale(44, 1) == 44.
    let program =
        format!("{HELPERS}fn main() -> Int {{\n  21\n  |> add1\n  |> double\n  |> scale(1)\n}}\n");
    assert_eq!(run_exit_code(&program), 44);
}

/// A pipe on a generic function infers its type argument exactly as the call
/// would (spec 0019: type/effect/throws match the desugared call).
#[test]
fn pipes_into_generic_function() {
    let source = "\
fn identity<T>(x: T) -> T { x }\n\
fn main() -> Int { 42 |> identity }\n";
    check_ok(source);
    assert_eq!(run_exit_code(source), 42);
}

/// The desugared call survives to the JS backend as an ordinary call.
#[test]
fn builds_to_js() {
    let source = format!("{HELPERS}fn main() -> Int {{ 21 |> add1 |> double |> scale(1) }}\n");
    let code = js(&source);
    assert!(
        code.contains("scale(") && code.contains("add1("),
        "pipeline should lower to plain calls in JS:\n{code}"
    );
}

/// The formatter renders `|>` with a single space on each side and is
/// idempotent on already-formatted pipes.
#[test]
fn formatter_spaces_the_operator() {
    let source = format!("{HELPERS}fn main() -> Int {{\n  21|>add1|>scale(1)\n}}\n");
    let formatted = fmt(&source);
    assert!(
        formatted.contains("21 |> add1 |> scale(1)"),
        "`|>` should render spaced:\n{formatted}"
    );
    assert_eq!(fmt(&formatted), formatted, "formatting must be idempotent");
}
