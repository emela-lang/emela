//! The first smoke tests: the module is valid, and simple programs run under `wasmtime`.

mod common;

use common::*;

#[test]
fn empty_main_validates() {
    compile_checked("fn main() { () }\n");
}

#[test]
fn integer_main_validates() {
    compile_checked("fn main() -> Int { 1 + 2 }\n");
}

#[test]
fn dbg_prints_integers() {
    let Some(run) = run("dbg-int", "fn main() {\n  dbg(1 + 2)\n  dbg(-5)\n}\n") else {
        return;
    };
    assert_eq!(run.stdout, "");
    assert_eq!(run.stderr, "3\n-5\n");
    assert_eq!(run.code, 0);
}
