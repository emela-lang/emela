//! The values of `const`s (spec 5.6): the memoized value, the name in a defect raised while a
//! const evaluates, a self-reference, and the nesting of the prefixes. Run under `wasmtime`
//! (skipped without it).

mod common;

use common::*;

/// A program that ends normally: its stderr.
fn stderr(name: &str, src: &str) -> Option<String> {
    let run = run(name, src)?;
    assert_eq!(
        run.code, 0,
        "{name} exited with {}: {}",
        run.code, run.stderr
    );
    Some(run.stderr)
}

/// A program that ends in a defect: its stderr.
fn defect(name: &str, src: &str) -> Option<String> {
    let run = run(name, src)?;
    assert_eq!(
        run.code, 101,
        "{name} exited with {}: {}",
        run.code, run.stderr
    );
    Some(run.stderr)
}

#[test]
fn a_const_is_evaluated_and_remembered() {
    // The value is computed on the first reference; a later reference returns the remembered one.
    // `BROKEN` defects on the first reference and remembers the defect, so the second reference
    // raises the same message instead of evaluating again.
    let Some(out) = stderr(
        "const-value",
        "const GREETING = \"hello #{List.length(NAMES)}\"\n\
         const NAMES = [\"a\", \"b\", \"c\"]\n\
         const LIMIT = 3 * 7\n\
         fn main() {\n  \
         dbg(GREETING)\n  \
         dbg(\"#{NAMES} #{LIMIT}\")\n  \
         dbg(LIMIT + 1)\n}\n",
    ) else {
        return;
    };
    assert_eq!(
        out,
        "\"hello 3\"\n\"[\\\"a\\\", \\\"b\\\", \\\"c\\\"] 21\"\n22\n"
    );
}

#[test]
fn a_defect_names_the_const_it_evaluates() {
    let Some(out) = defect(
        "const-defect",
        "const NAMES = [\"a\", \"b\", \"c\"]\n\
         const LIMIT = 3 * 7\n\
         const BROKEN = List.length(NAMES) / (LIMIT - 21)\n\
         fn main() {\n  dbg(BROKEN)\n  dbg(BROKEN)\n}\n",
    ) else {
        return;
    };
    assert_eq!(
        out,
        "defect: while evaluating const BROKEN: division by zero\n"
    );
}

#[test]
fn the_prefixes_nest() {
    let Some(out) = defect(
        "const-nested",
        "const INNER = 1 / 0\n\
         const OUTER = INNER + 1\n\
         fn main() {\n  dbg(OUTER)\n}\n",
    ) else {
        return;
    };
    assert_eq!(
        out,
        "defect: while evaluating const OUTER: while evaluating const INNER: division by zero\n"
    );
}

#[test]
fn a_const_referring_to_itself_is_a_defect() {
    // The inner reference raises the self-reference defect and returns a placeholder, so the
    // function on the way out computes a value and it is printed before the pending defect ends the
    // run. This is the C target's behaviour (`em_const_enter` raises without unwinding); JS throws,
    // so it prints nothing there.
    let Some(out) = defect(
        "const-self",
        "const SELF = through()\n\
         fn through() -> Int { SELF + 1 }\n\
         fn main() {\n  dbg(SELF)\n}\n",
    ) else {
        return;
    };
    assert_eq!(
        out,
        "1\ndefect: const SELF refers to itself while being evaluated\n"
    );
}
