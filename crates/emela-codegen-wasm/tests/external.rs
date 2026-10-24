//! `@external(wasm, ...)` is not available on the WASI 0.3 component target yet: a component
//! can only import interfaces declared in its WIT world, and custom host WIT is not implemented.

mod common;

use common::*;
use emela_codegen_wasm::WasmErrorKind;

#[test]
fn a_program_naming_a_wasi_host_is_diagnosed() {
    // Only the bundled `Std` externals are runtime wrappers. A program's own binding to the same
    // WASI function is not, whatever its shape.
    let errors = to_wasm(
        r#"
effect Randomish {
  fn next() -> Int
}

@external(wasm, "wasi:random/random@0.3.0", "get-random-u64")
fn host_random() -> Int

handler FromWasi: Randomish {
  fn next() { host_random() }
}

fn main() {
  with FromWasi {
    dbg((use Randomish).next())
  }
}
"#,
    )
    .expect_err("a program's own WASI binding is not supported");

    assert!(
        errors.iter().any(|error| {
            error.kind == WasmErrorKind::Unsupported
                && error.message.contains("`host_random`")
                && error.message.contains("`wasm` import")
        }),
        "expected an unsupported wasm import diagnostic, got {errors:#?}"
    );
}

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

#[test]
fn the_scheduler_hook_is_not_a_user_host() {
    // `emela:runtime/scheduler@0.1.0`/`when-stalled` is the bundled TestClock's idle hook, not a
    // WIT import. The runtime implements it only for the bundled declaration, so a program's own
    // binding to the same name is an unsupported `wasm` import, reported once.
    let Err(errors) = to_wasm(
        r#"
@external(wasm, "emela:runtime/scheduler@0.1.0", "when-stalled")
suspend fn poke(at: Int64) -> ()

effect Hook {
  suspend fn stall() -> ()
}

handler Hooked: Hook {
  suspend fn stall(resume) {
    poke(Int64.from_int(5))
    resume(())
  }
}

fn main() {
  with Hooked {
    (use Hook).stall()
  }
}
"#,
    ) else {
        panic!("a user binding to the scheduler hook must not compile");
    };
    let about: Vec<_> = errors
        .iter()
        .filter(|error| error.message.contains("`poke`"))
        .collect();
    assert_eq!(about.len(), 1, "{errors:#?}");
    assert_eq!(about[0].kind, WasmErrorKind::Unsupported);
}
