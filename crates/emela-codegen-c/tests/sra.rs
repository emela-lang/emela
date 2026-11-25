//! Behavioural tests for the scalar replacement of tuples in the C backend: the tuple form and its
//! handwritten scalar form print the same, with the LIR optimisation on and off. Every run has the
//! leak check on, so a replaced tuple must not leak or free a field.

mod common;

#[allow(clippy::duplicate_mod)] // `common` includes the same Core test support.
#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

use common::{PRELUDE, Run, build_and_run, cc, out_dir};
use emela_codegen_c::{Entry, compile_with};
use emela_core::Roots;
use support::try_lower;

/// Lowers `src` and builds it to C, with the LIR optimisation on or off.
fn run_lir(name: &str, src: &str, optimize: bool) -> Run {
    let core = try_lower(&[("main.emel", src)], Roots::Exported)
        .unwrap_or_else(|e| panic!("lowering failed: {e:#?}"));
    let out = compile_with(&core, &Entry::Main, optimize)
        .unwrap_or_else(|e| panic!("C compilation failed: {e:#?}"));
    build_and_run(&out_dir(name), &out, &["-O1"])
}

fn same(got: &Run, expected: &Run, what: &str) {
    assert_eq!(
        (&got.stdout, &got.stderr, got.code),
        (&expected.stdout, &expected.stderr, expected.code),
        "{what}: SRA changed the observable behaviour"
    );
}

#[test]
fn a_non_escaping_tuple_matches_its_scalar_form() {
    if cc().is_none() {
        return;
    }
    let tuple = format!(
        "{PRELUDE}
fn add(a: Int, b: Int) -> Int {{
  t = (a * 2, b + 1)
  (x, y) = t
  x + y
}}

fn main() {{
  print(Int.to_string(add(3, 4)))
}}
"
    );
    let scalar = format!(
        "{PRELUDE}
fn add(a: Int, b: Int) -> Int {{
  x = a * 2
  y = b + 1
  x + y
}}

fn main() {{
  print(Int.to_string(add(3, 4)))
}}
"
    );
    let on = run_lir("sra-tuple-on", &tuple, true);
    let off = run_lir("sra-tuple-off", &tuple, false);
    let manual = run_lir("sra-scalar", &scalar, true);
    assert_eq!((on.code, on.stderr.as_str()), (0, ""), "{on:?}");
    same(&on, &off, "the tuple with optimisation off");
    same(&on, &manual, "the handwritten scalar form");
}

#[test]
fn the_field_evaluation_order_is_kept() {
    if cc().is_none() {
        return;
    }
    let tuple = format!(
        "{PRELUDE}
fn f() -> Int {{
  print(\"f\")
  1
}}

fn g() -> Int {{
  print(\"g\")
  2
}}

fn main() {{
  t = (f(), g())
  (a, b) = t
  print(Int.to_string(a + b))
}}
"
    );
    let on = run_lir("sra-order-on", &tuple, true);
    let off = run_lir("sra-order-off", &tuple, false);
    assert_eq!((on.code, on.stderr.as_str()), (0, ""), "{on:?}");
    assert_eq!(on.stdout, "f\ng\n3\n");
    same(&on, &off, "the tuple with optimisation off");
}
