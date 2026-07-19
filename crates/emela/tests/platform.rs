use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

/// Writes an app that imports the embedded `std.io` (spec 0038) and calls
/// `io.print`; no package is needed. Returns the app file.
fn hello_app() -> std::path::PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-platform-test-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let app = dir.join("main.emel");
    fs::write(
        &app,
        "import std.io\nfn main() -> Unit uses { Io } { Io.print(\"Hello, Emela!\\n\") }\n",
    )
    .unwrap();
    app
}

fn emela() -> Command {
    Command::new(env!("CARGO_BIN_EXE_emela"))
}

#[test]
fn js_backend_resolves_platform_via_runtime() {
    let app = hello_app();
    let output = emela()
        .arg("build")
        .arg("--backend")
        .arg("js-node")
        .arg(&app)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(app.parent().unwrap());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let js = String::from_utf8(output.stdout).unwrap();
    // The runtime bundles only the used platform function: `eprint` is never
    // instantiated, so `write_stderr` appears nowhere in the output.
    assert!(
        js.contains("\"io.write_stdout\": (s) => process.stdout.write(s)"),
        "{js}"
    );
    assert!(!js.contains("write_stderr"), "{js}");
    // The wrapper calls the runtime; `main` passes the message to the wrapper
    // (its emitted name is a monomorphized mangling, so match the literal).
    assert!(js.contains("__rt[\"io.write_stdout\"]"), "{js}");
    assert!(js.contains("(\"Hello, Emela!\\n\")"), "{js}");
}

#[test]
fn wasm_backend_builds_a_valid_module() {
    let app = hello_app();
    let out = app.parent().unwrap().join("hello.wasm");
    let output = emela()
        .arg("build")
        .arg("--backend")
        .arg("wasm-wasi")
        .arg("-o")
        .arg(&out)
        .arg(&app)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = fs::read(&out).unwrap();
    let _ = fs::remove_dir_all(app.parent().unwrap());
    // A WASI module with the wasm magic; instantiation needs an `io`-capable
    // runtime (WAMR `iwasm`, `wasmtime`, ...).
    assert_eq!(&bytes[0..4], b"\0asm");
}

/// The `throws` clause is part of the registry signature (spec 0043):
/// declaring `throws` on an infallible platform function is a signature
/// mismatch, not a silently-accepted extension.
#[test]
fn throws_on_an_infallible_platform_fn_is_rejected() {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-throws-test-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let app = dir.join("main.emel");
    fs::write(
        &app,
        "module io\n\npub enum Bad {\n  Oops\n}\n\neffect Io {\n    extern fn write_stdout(s: String) -> Unit throws Bad\n}\n\nfn main() -> Unit { () }\n",
    )
    .unwrap();
    let output = emela().arg("check").arg(&app).output().unwrap();
    let _ = fs::remove_dir_all(&dir);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Platform signature mismatch"), "{stderr}");
}

/// Builds a program that uses a pure `std.int.to_text` (if + `/`/`%` +
/// `Char`/`++`) from a `std` package together with the embedded `std.io`
/// effect's `print` (spec 0038), end to end to a wasm module. The package
/// supplies only non-embedded modules; the embedded `io` resolves alongside
/// it. Imported functions no longer bind bare names (spec 0037), so the
/// import cannot capture the `Show` method call inside the embedded
/// `print<T: Show>` — the pre-0037 name-capture bug.
#[test]
fn pure_to_string_builds_on_wasm() {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-tostring-test-{}-{id}", std::process::id()));
    let package = dir.join("std");
    fs::create_dir_all(package.join("src")).unwrap();
    fs::write(
        package.join("emela-package.json"),
        r#"{"name":"std","source":"src"}"#,
    )
    .unwrap();
    fs::write(
        package.join("src").join("int.emel"),
        "module int\nfn digits(n: Int) -> String { if n == 0 { \"\" } else { digits(n / 10) ++ String::from_char(Char::from_code(48 + n % 10)) } }\npub fn to_text(n: Int) -> String { if n == 0 { \"0\" } else { if n < 0 { \"-\" ++ digits(0 - n) } else { digits(n) } } }\n",
    )
    .unwrap();
    let app = dir.join("main.emel");
    fs::write(
        &app,
        "import std.io\nimport std.int\nfn main() -> Unit uses { Io } { Io.print(int.to_text(42) ++ \"\\n\") }\n",
    )
    .unwrap();

    let out = dir.join("out.wasm");
    let output = emela()
        .arg("build")
        .arg("--backend")
        .arg("wasm-wasi")
        .arg("--package")
        .arg(&package)
        .arg("-o")
        .arg(&out)
        .arg(&app)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = fs::read(&out).unwrap();
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(&bytes[0..4], b"\0asm");
}
