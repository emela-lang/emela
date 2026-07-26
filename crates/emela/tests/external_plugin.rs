#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> std::path::PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-plugin-test-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn external_backend_round_trip() {
    let dir = temp_dir();

    // A dummy plugin: consume the IR request, emit a fixed artifact ("PLUG").
    let script = dir.join("plugin.sh");
    fs::write(
        &script,
        "#!/bin/sh\ncat >/dev/null\nprintf '%s' '{\"status\":\"ok\",\"kind\":\"JsSource\",\"bytes\":[80,76,85,71]}'\n",
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

    let descriptor = dir.join("backend.json");
    fs::write(
        &descriptor,
        format!(
            "{{\"name\":\"dummy\",\"backend\":\"custom\",\"abi_version\":1,\"command\":[\"sh\",\"{}\"]}}",
            script.display()
        ),
    )
    .unwrap();

    let source = dir.join("main.emel");
    fs::write(&source, "fn main() -> Int { 1 }\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("build")
        .arg("--backend")
        .arg(&descriptor)
        .arg(&source)
        .output()
        .unwrap();

    let _ = fs::remove_dir_all(&dir);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "PLUG");
}

/// Lowering erases effect-row variables (spec 0022), so the IR a backend or an
/// external plugin receives never mentions one — not in a function's own row,
/// and not in a `uses` row nested in a type (a parameter's function type, a
/// `FunctionRef`'s signature). `tails` is skipped when empty, so its absence
/// from the payload is exactly the erasure guarantee.
#[test]
fn lowered_ir_carries_no_row_variables() {
    let dir = temp_dir();
    let captured = dir.join("ir.json");

    // A plugin that keeps the IR request it was handed, then emits an artifact.
    let script = dir.join("plugin.sh");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\ncat >'{}'\nprintf '%s' '{{\"status\":\"ok\",\"kind\":\"JsSource\",\"bytes\":[80,76,85,71]}}'\n",
            captured.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

    let descriptor = dir.join("backend.json");
    fs::write(
        &descriptor,
        format!(
            "{{\"name\":\"dummy\",\"backend\":\"custom\",\"abi_version\":1,\"command\":[\"sh\",\"{}\"]}}",
            script.display()
        ),
    )
    .unwrap();

    // `run_it` is row-polymorphic and instantiated twice; `apply` is generic in
    // both a type and a row, so it also reaches the IR through monomorphization.
    let source = dir.join("main.emel");
    fs::write(
        &source,
        "import std.io\n\
         fn run_it<e>(f: () -> Int uses e) -> Int uses e { f() }\n\
         fn apply<T, e>(x: T, f: (T) -> T uses e) -> T uses e { f(x) }\n\
         fn io_one() -> Int uses { Io } {\n\
           let p = Io.print(\"one\\n\")\n\
           1\n\
         }\n\
         fn inc(x: Int) -> Int uses { Io } {\n\
           let p = Io.print(\"inc\\n\")\n\
           x + 1\n\
         }\n\
         fn main() -> Int uses { Io } {\n\
           let a = run_it(io_one)\n\
           apply(a, inc)\n\
         }\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("build")
        .arg("--backend")
        .arg(&descriptor)
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let ir = fs::read_to_string(&captured).unwrap();
    let _ = fs::remove_dir_all(&dir);

    assert!(
        ir.contains("run_it"),
        "the row-polymorphic function should be in the IR"
    );
    assert!(
        !ir.contains("tails"),
        "lowered IR still carries a row variable:\n{ir}"
    );
}
