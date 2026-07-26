//! Type-checking tests for effect-row *polymorphism* (spec 0022): a function
//! that declares row parameters in `<...>` and threads them through parameter
//! `uses` rows unifies each row variable against the argument's actual row and
//! propagates the residual to the caller. This is also the soundness fix — a
//! generic higher-order function no longer silently drops a function argument's
//! effects or `throws`. The parser-level accept/reject rules live in
//! `effect_row_syntax.rs`; ordinary function-value subsumption in
//! `effect_subsumption.rs`.

use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

/// Runs `emela check` against a single self-contained file (no package).
fn check_single(source: &str) -> std::process::Output {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-rowpoly-{}-{id}", std::process::id()));
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

fn assert_ok(source: &str) {
    let output = check_single(source);
    assert!(
        output.status.success(),
        "expected check to pass:\n{}",
        stderr(&output)
    );
}

/// Asserts the check fails with `diagnostic` in it, returning the whole stderr
/// so a caller can also pin the hint text.
fn assert_rejected(source: &str, diagnostic: &str) -> String {
    let output = check_single(source);
    assert!(!output.status.success(), "expected check to fail");
    let err = stderr(&output);
    assert!(err.contains(diagnostic), "unexpected diagnostic:\n{err}");
    err
}

/// An effectful closure passed to a row-polymorphic HOF propagates its effect to
/// the caller: `apply<T, e>` instantiated with `e = { Io }` makes the call site
/// `uses { Io }`, which `main` declares.
#[test]
fn effectful_closure_propagates_to_caller() {
    assert_ok(
        "import std.io\n\
         fn apply<T, e>(x: T, f: (T) -> T uses e) -> T uses e { f(x) }\n\
         fn io_inc(x: Int) -> Int uses { Io } {\n\
             let p = Io.print(\"x\\n\")\n\
             x + 1\n\
         }\n\
         fn main() -> Unit uses { Io } {\n\
             let r = apply(5, io_inc)\n\
             ()\n\
         }\n",
    );
}

/// The same instantiation makes a pure `main` unsound, so it is rejected: the
/// propagated `Io` is unhandled where `uses {}` is declared.
#[test]
fn propagated_effect_unhandled_in_pure_caller() {
    assert_rejected(
        "import std.io\n\
         fn apply<T, e>(x: T, f: (T) -> T uses e) -> T uses e { f(x) }\n\
         fn io_inc(x: Int) -> Int uses { Io } {\n\
             let p = Io.print(\"x\\n\")\n\
             x + 1\n\
         }\n\
         fn main() -> Unit uses {} {\n\
             let r = apply(5, io_inc)\n\
             ()\n\
         }\n",
        "Unhandled effects",
    );
}

/// A pure closure instantiates `e = {}`, so the whole call stays pure and a
/// `uses {}` caller is accepted.
#[test]
fn pure_closure_keeps_call_pure() {
    assert_ok(
        "fn apply<T, e>(x: T, f: (T) -> T uses e) -> T uses e { f(x) }\n\
         fn pure_inc(x: Int) -> Int uses {} { x + 1 }\n\
         fn main() -> Unit uses {} {\n\
             let r = apply(5, pure_inc)\n\
             ()\n\
         }\n",
    );
}

/// The soundness fix (breaking change): a generic HOF that declares a *pure*
/// callback (`f: (T) -> T`, i.e. `uses {}`, no row variable) rejects an
/// effectful closure. Before the row-unification pass this compiled and dropped
/// the effect.
#[test]
fn effectful_closure_rejected_by_pure_generic_hof() {
    let stderr = assert_rejected(
        "import std.io\n\
         fn apply_pure<T>(x: T, f: (T) -> T) -> T { f(x) }\n\
         fn io_inc(x: Int) -> Int uses { Io } {\n\
             let p = Io.print(\"x\\n\")\n\
             x + 1\n\
         }\n\
         fn main() -> Unit uses { Io } {\n\
             let r = apply_pure(5, io_inc)\n\
             ()\n\
         }\n",
        "Type mismatch",
    );
    // The rows are what differ, so they must be legible in the message and the
    // hint must name the row-polymorphic escape hatch.
    assert!(
        stderr.contains("expected `(Int) -> Int uses {}`, but found `(Int) -> Int uses { Io }`"),
        "{stderr}"
    );
    assert!(stderr.contains("row-polymorphic"), "{stderr}");
}

/// The soundness fix for the `throws` channel (breaking change): a non-throwing
/// callback rejects a throwing closure, closing the same hole for errors.
#[test]
fn throwing_closure_rejected_by_non_throwing_param() {
    let stderr = assert_rejected(
        "fn apply<T, e>(x: T, f: (T) -> T uses e) -> T uses e { f(x) }\n\
         fn may_fail(x: Int) -> Int throws String uses {} { throw \"bad\" }\n\
         fn main() -> Unit uses {} {\n\
             let r = apply(5, may_fail)\n\
             ()\n\
         }\n",
        "Type mismatch",
    );
    assert!(stderr.contains("throws String"), "{stderr}");
    assert!(stderr.contains("`try`/`catch`"), "{stderr}");
}

/// The extended row `uses { Io, ..e }` always contributes `Io`, even for a pure
/// closure — so a pure caller is still rejected.
#[test]
fn row_extension_always_adds_concrete_effect() {
    assert_rejected(
        "import std.io\n\
         fn traced<T, e>(x: T, f: (T) -> T uses e) -> T uses { Io, ..e } {\n\
             let p = Io.print(\"call\\n\")\n\
             f(x)\n\
         }\n\
         fn pure_inc(x: Int) -> Int uses {} { x + 1 }\n\
         fn main() -> Unit uses {} {\n\
             let r = traced(5, pure_inc)\n\
             ()\n\
         }\n",
        "Unhandled effects",
    );
}

/// The extended row's own `Io` plus the closure's `Io` collapse to `{ Io }`,
/// which the caller declares.
#[test]
fn row_extension_accepted_when_caller_declares_concrete() {
    assert_ok(
        "import std.io\n\
         fn traced<T, e>(x: T, f: (T) -> T uses e) -> T uses { Io, ..e } {\n\
             let p = Io.print(\"call\\n\")\n\
             f(x)\n\
         }\n\
         fn io_inc(x: Int) -> Int uses { Io } {\n\
             let p = Io.print(\"x\\n\")\n\
             x + 1\n\
         }\n\
         fn main() -> Unit uses { Io } {\n\
             let r = traced(5, io_inc)\n\
             ()\n\
         }\n",
    );
}

/// One row variable shared by two callbacks accumulates the *sum* of their
/// residuals: an `Io` closure and a `Clock` closure make the call `uses { Clock,
/// Io }`, which the caller must declare in full.
#[test]
fn shared_row_variable_sums_residuals() {
    assert_ok(
        "import std.io\n\
         import std.clock\n\
         fn seq<T, e>(x: T, f: (T) -> T uses e, g: (T) -> T uses e) -> T uses e {\n\
             g(f(x))\n\
         }\n\
         fn io_step(x: Int) -> Int uses { Io } {\n\
             let p = Io.print(\"x\\n\")\n\
             x\n\
         }\n\
         fn clock_step(x: Int) -> Int uses { Clock } {\n\
             let t = Clock.now()\n\
             x + t\n\
         }\n\
         fn main() -> Unit uses { Io, Clock } {\n\
             let r = seq(5, io_step, clock_step)\n\
             ()\n\
         }\n",
    );
}

/// The residual sum is unhandled when the caller declares only one of the two
/// effects.
#[test]
fn shared_row_variable_sum_unhandled() {
    assert_rejected(
        "import std.io\n\
         import std.clock\n\
         fn seq<T, e>(x: T, f: (T) -> T uses e, g: (T) -> T uses e) -> T uses e {\n\
             g(f(x))\n\
         }\n\
         fn io_step(x: Int) -> Int uses { Io } {\n\
             let p = Io.print(\"x\\n\")\n\
             x\n\
         }\n\
         fn clock_step(x: Int) -> Int uses { Clock } {\n\
             let t = Clock.now()\n\
             x + t\n\
         }\n\
         fn main() -> Unit uses { Io } {\n\
             let r = seq(5, io_step, clock_step)\n\
             ()\n\
         }\n",
        "Unhandled effects",
    );
}

/// A row-only polymorphic function (`<e>`, no type parameters) still flows
/// through the generic call path so its row is unified and propagated.
#[test]
fn row_only_polymorphic_function() {
    assert_ok(
        "import std.io\n\
         fn run<e>(f: () -> Unit uses e) -> Unit uses e { f() }\n\
         fn shout() -> Unit uses { Io } { Io.print(\"hi\\n\") }\n\
         fn main() -> Unit uses { Io } {\n\
             run(shout)\n\
         }\n",
    );
}

/// A row-polymorphic function's row variable is fixed only at a direct call
/// (spec 0022), so — like a type-generic function — it cannot be taken as a
/// first-class value.
#[test]
fn rejects_row_polymorphic_function_as_value() {
    assert_rejected(
        "fn run<e>(f: () -> Unit uses e) -> Unit uses e { f() }\n\
         fn main() -> Unit uses {} {\n\
             let g = run\n\
             ()\n\
         }\n",
        "Generic function used as a value",
    );
}

/// A row variable that appears in no parameter's `uses` row cannot be inferred
/// from a call (spec 0022).
#[test]
fn rejects_uninferable_row_parameter() {
    assert_rejected(
        "fn f<e>(x: Int) -> Unit uses e {\n\
             ()\n\
         }\n",
        "Uninferable row parameter",
    );
}

/// A row variable on a nested function type (here inside a parameter's return
/// type) is not inferable in v1 and is rejected.
#[test]
fn rejects_row_variable_in_nested_position() {
    assert_rejected(
        "fn f<e>(g: (Int) -> ((Int) -> Int uses e)) -> Unit uses e {\n\
             ()\n\
         }\n",
        "Row variable in a nested position",
    );
}

/// A function literal (spec 0008) is unified like any other argument: its
/// declared row instantiates `e`, so the effect reaches the caller.
#[test]
fn closure_literal_propagates_row() {
    assert_ok(
        "import std.io\n\
         fn apply<T, e>(x: T, f: (T) -> T uses e) -> T uses e { f(x) }\n\
         fn main() -> Unit uses { Io } {\n\
             let r = apply(5, fn (x: Int) -> Int uses { Io } {\n\
                 let p = Io.print(\"hi\\n\")\n\
                 x + 1\n\
             })\n\
             ()\n\
         }\n",
    );
}

/// The same literal in a pure caller is rejected — the row it instantiates is
/// unhandled there.
#[test]
fn closure_literal_row_unhandled_in_pure_caller() {
    assert_rejected(
        "import std.io\n\
         fn apply<T, e>(x: T, f: (T) -> T uses e) -> T uses e { f(x) }\n\
         fn main() -> Unit uses {} {\n\
             let r = apply(5, fn (x: Int) -> Int uses { Io } {\n\
                 let p = Io.print(\"hi\\n\")\n\
                 x + 1\n\
             })\n\
             ()\n\
         }\n",
        "Unhandled effects",
    );
}

/// A row-polymorphic function calling another one keeps the row *open*: `outer`
/// passes its own still-abstract parameter row `r` to `apply`, whose result row
/// must come back as `r` (not as a concrete effect) so `outer`'s declaration
/// still covers its body.
#[test]
fn row_propagates_through_row_polymorphic_caller() {
    assert_ok(
        "import std.io\n\
         fn apply<T, e>(x: T, f: (T) -> T uses e) -> T uses e { f(x) }\n\
         fn outer<T, r>(x: T, g: (T) -> T uses r) -> T uses r { apply(x, g) }\n\
         fn io_inc(x: Int) -> Int uses { Io } {\n\
             let p = Io.print(\"x\\n\")\n\
             x + 1\n\
         }\n\
         fn main() -> Unit uses { Io } {\n\
             let v = outer(5, io_inc)\n\
             ()\n\
         }\n",
    );
}

/// Recursion closes on itself: the recursive call binds `e ↦ { ..e }`, so the
/// body's row stays exactly the declared one (the shape `list.map` needs).
#[test]
fn self_recursive_row_polymorphic_function() {
    assert_ok(
        "import std.io\n\
         fn repeat<T, e>(n: Int, x: T, f: (T) -> T uses e) -> T uses e {\n\
             if n <= 0 {\n\
                 x\n\
             } else {\n\
                 repeat(n - 1, f(x), f)\n\
             }\n\
         }\n\
         fn io_inc(x: Int) -> Int uses { Io } {\n\
             let p = Io.print(\"x\\n\")\n\
             x + 1\n\
         }\n\
         fn main() -> Unit uses { Io } {\n\
             let r = repeat(3, 0, io_inc)\n\
             ()\n\
         }\n",
    );
}

/// A parameter function's `uses` row may name at most one row variable (spec
/// 0022): the minimal solution for two at once is ambiguous.
#[test]
fn rejects_two_row_variables_on_one_parameter() {
    assert_rejected(
        "fn f<e1, e2>(g: () -> Unit uses { ..e1, ..e2 }) -> Unit uses { ..e1, ..e2 } {\n\
             g()\n\
         }\n",
        "Too many row variables in a parameter",
    );
}
