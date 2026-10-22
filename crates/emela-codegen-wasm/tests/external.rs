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

#[test]
fn a_wasm_external_is_reported_once() {
    let errors = to_wasm(
        r#"
effect Host {
  fn each(f: fn(Int) -> ()) -> ()
  suspend fn wait() -> ()
}

@external(wasm, "env", "each")
fn host_each(f: fn(Int) -> ()) -> ()

@external(wasm, "env", "wait")
suspend fn host_wait() -> ()

handler WasmHost: Host {
  fn each(f) { host_each(f) }
  suspend fn wait(resume) { resume(host_wait()) }
}

fn main() {
  with WasmHost {
    (use Host).each(fn(_x) { () })
    (use Host).wait()
  }
}
"#,
    )
    .expect_err("a reachable wasm import must not compile to a component");

    for name in ["host_each", "host_wait"] {
        let about: Vec<_> = errors
            .iter()
            .filter(|error| error.message.contains(&format!("`{name}`")))
            .collect();
        assert_eq!(about.len(), 1, "`{name}`: {errors:#?}");
        assert_eq!(about[0].kind, WasmErrorKind::Unsupported);
    }
}
