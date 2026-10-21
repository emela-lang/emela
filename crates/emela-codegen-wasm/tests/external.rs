//! `@external(wasm, ...)` is not available on the WASI 0.3 component target yet: a component
//! can only import interfaces declared in its WIT world, and custom host WIT is not implemented.

mod common;

use common::*;
use emela_codegen_wasm::WasmErrorKind;

#[test]
fn wasm_externals_are_diagnosed() {
    let errors = to_wasm(
        r#"
effect Clock {
  fn now() -> Int
}

@external(wasm, "env", "now")
fn host_now() -> Int

handler WasmClock: Clock {
  fn now() { host_now() }
}

fn main() {
  with WasmClock { dbg((use Clock).now()) }
}
"#,
    )
    .expect_err("a reachable wasm import must not compile to a component");

    assert!(
        errors.iter().any(|error| {
            error.kind == WasmErrorKind::Unsupported
                && error.message.contains("`host_now`")
                && error.message.contains("`wasm` import")
        }),
        "expected an unsupported wasm import diagnostic, got {errors:#?}"
    );
}
