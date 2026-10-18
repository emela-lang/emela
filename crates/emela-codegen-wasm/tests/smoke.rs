//! The first smoke tests: the module is valid, and simple programs run under `wasmtime`.

mod common;

use common::*;
use emela_codegen_wasm::WasmErrorKind;

#[test]
fn empty_main_validates() {
    compile_checked("fn main() { () }\n");
}

#[test]
fn integer_main_validates() {
    compile_checked("fn main() -> Int { 1 + 2 }\n");
}

#[test]
fn an_external_returning_a_reference_is_rejected() {
    // A parameter that cannot cross the host boundary is reported by `host_type_error`, and the
    // result must be checked the same way: without it the import would be declared without a
    // result and the caller would read whatever the stack happened to hold.
    let errors = to_wasm(
        "@external(wasm, \"env\", \"name\")\n\
         fn host_name() -> String use {}\n\
         fn main() { dbg(host_name()) }\n",
    )
    .expect_err("a `String` result cannot cross to WASM");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].kind, WasmErrorKind::Unsupported, "{errors:?}");
    assert!(
        errors[0].message.contains("cannot cross to WASM"),
        "{errors:?}"
    );
}

#[test]
fn an_external_taking_a_reference_is_rejected() {
    // The parameter side (the pre-existing half of the same asymmetry), for symmetry.
    let errors = to_wasm(
        "@external(wasm, \"env\", \"log\")\n\
         fn host_log(s: String) -> () use {}\n\
         fn main() { host_log(\"hi\") }\n",
    )
    .expect_err("a `String` parameter cannot cross to WASM");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].kind, WasmErrorKind::Unsupported, "{errors:?}");
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

#[test]
fn a_release_defect_becomes_a_note_of_the_first_defect() {
    // The note is assembled in `defect_resume` by chaining `str_append`; the intermediate strings
    // are given up there (`note_append`). This checks the wording and that the chain still builds.
    let Some(run) = run(
        "release-note",
        "effect Pool {\n  fn get() -> Int\n}\n\
         handler Outer(n: Int) implements Pool {\n\
           init { Outer(n: 0) }\n\
           release { panic(\"outer release failed\") }\n\
           fn get() { self.n }\n\
         }\n\
         fn work() -> Int {\n\
           zero = (use Pool).get()\n\
           1 / zero\n\
         }\n\
         fn main() {\n\
           with Outer { dbg(work()) }\n\
         }\n",
    ) else {
        return;
    };
    assert_eq!(run.code, 101, "stderr: {}", run.stderr);
    assert_eq!(
        run.stderr,
        "defect: division by zero\n\
         note: the release of handler Outer also failed: outer release failed\n"
    );
}
