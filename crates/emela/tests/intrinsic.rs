//! End-to-end tests for `intrinsic fn` (spec 0021): stdlib declares a pure
//! primitive, wraps it, and the backend inlines it to a native instruction.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

/// Lays out a `std` package whose `core` module declares the given intrinsic
/// source, plus an app. Returns (temp dir, package dir, app file).
fn project(core_src: &str, app_src: &str) -> (PathBuf, PathBuf, PathBuf) {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("emela-intrinsic-test-{}-{id}", std::process::id()));
    let package = dir.join("std");
    fs::create_dir_all(package.join("src")).unwrap();
    fs::write(
        package.join("emela-package.json"),
        r#"{"name":"std","source":"src"}"#,
    )
    .unwrap();
    fs::write(package.join("src").join("core.emel"), core_src).unwrap();
    let app = dir.join("main.emel");
    fs::write(&app, app_src).unwrap();
    (dir.clone(), package, app)
}

fn emela() -> Command {
    Command::new(env!("CARGO_BIN_EXE_emela"))
}

/// A `core` module wrapping the `i32_add` intrinsic in an `add` function.
const CORE_ADD: &str = "module core\nintrinsic fn i32_add(a: Int, b: Int) -> Int uses {}\npub fn add(a: Int, b: Int) -> Int uses {} { i32_add(a, b) }\n";

#[test]
fn js_backend_inlines_intrinsic() {
    let (dir, package, app) = project(
        CORE_ADD,
        "import std.core.add\nfn main() -> Int uses {} { add(2, 3) }\n",
    );
    let output = emela()
        .arg("build")
        .arg("--backend")
        .arg("js-node")
        .arg("--package")
        .arg(&package)
        .arg(&app)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(&dir);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let js = String::from_utf8(output.stdout).unwrap();
    // The intrinsic is inlined to a native `+`, not emitted as a call: its name
    // does not survive into the artifact.
    assert!(!js.contains("i32_add"), "intrinsic was not inlined:\n{js}");
    assert!(js.contains(" + "), "expected an inlined `+`:\n{js}");
}

#[test]
fn wasm_backend_builds_with_intrinsic() {
    let (dir, package, app) = project(
        CORE_ADD,
        "import std.core.add\nfn main() -> Int uses {} { add(2, 3) }\n",
    );
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

#[test]
fn unknown_intrinsic_is_rejected() {
    // `bogus_op` is not in the intrinsic interface (spec 0021), so declaring it
    // as an `intrinsic fn` is a compile error.
    let (dir, package, app) = project(
        "module core\nintrinsic fn bogus_op(a: Int, b: Int) -> Int uses {}\npub fn go(a: Int, b: Int) -> Int uses {} { bogus_op(a, b) }\n",
        "import std.core.go\nfn main() -> Int uses {} { go(2, 3) }\n",
    );
    let output = emela()
        .arg("build")
        .arg("--backend")
        .arg("js-node")
        .arg("--package")
        .arg(&package)
        .arg(&app)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(&dir);
    assert!(!output.status.success(), "expected a compile error");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("intrinsic"),
        "expected an intrinsic error, got:\n{stderr}"
    );
}

#[test]
fn impure_intrinsic_is_rejected() {
    // Intrinsics must be pure (`uses {}`); an effectful declaration is rejected.
    let (dir, package, app) = project(
        "module core\nintrinsic fn i32_add(a: Int, b: Int) -> Int uses { io }\npub fn add(a: Int, b: Int) -> Int uses { io } { i32_add(a, b) }\n",
        "import std.core.add\nfn main() -> Int uses { io } { add(2, 3) }\n",
    );
    let output = emela()
        .arg("build")
        .arg("--backend")
        .arg("js-node")
        .arg("--package")
        .arg(&package)
        .arg(&app)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(&dir);
    assert!(!output.status.success(), "expected a compile error");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("pure") || stderr.contains("Intrinsic"),
        "expected a purity error, got:\n{stderr}"
    );
}
