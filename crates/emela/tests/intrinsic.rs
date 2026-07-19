//! End-to-end tests for `intrinsic fn` (spec 0021) under the embedded-std
//! boundary (spec 0038): the embedded std declares every intrinsic and wraps
//! it, backends inline calls to native instructions, and user sources may not
//! declare intrinsics of their own.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("emela-intrinsic-test-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn emela() -> Command {
    Command::new(env!("CARGO_BIN_EXE_emela"))
}

/// Writes `source` to a temp `main.emel` and builds it with `backend` (no
/// package). Returns the process output.
fn build_single(source: &str, backend: &str, out: Option<&PathBuf>) -> std::process::Output {
    let dir = temp_dir();
    let input = dir.join("main.emel");
    fs::write(&input, source).unwrap();
    let mut cmd = emela();
    cmd.arg("build").arg("--backend").arg(backend);
    if let Some(out) = out {
        cmd.arg("-o").arg(out);
    }
    let output = cmd.arg(&input).output().unwrap();
    let _ = fs::remove_dir_all(&dir);
    output
}

/// An operator bottoms out in a Core Prelude intrinsic (spec 0021), which the
/// JS backend inlines to a native `+`: the intrinsic's name does not survive
/// into the artifact. No package is involved anywhere.
#[test]
fn js_backend_inlines_operator_intrinsic() {
    let output = build_single("fn main() -> Int uses {} { 2 + 3 }\n", "js-node", None);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let js = String::from_utf8(output.stdout).unwrap();
    assert!(!js.contains("i32_add"), "intrinsic was not inlined:\n{js}");
    assert!(js.contains(" + "), "expected an inlined `+`:\n{js}");
}

/// The `Char`/`String` conversions are bare Core Prelude intrinsics (spec 0021,
/// formerly the `Char::from_code` / `String::from_char` builtins): usable with
/// no import, and inlined so their names never reach the artifact.
#[test]
fn js_backend_inlines_char_string_conversions() {
    let output = build_single(
        "fn main() -> String uses {} { string_from_char(char_from_code(65)) }\n",
        "js-node",
        None,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let js = String::from_utf8(output.stdout).unwrap();
    assert!(
        !js.contains("char_from_code") && !js.contains("string_from_char"),
        "conversion intrinsics were not inlined:\n{js}"
    );
    assert!(
        js.contains("String.fromCodePoint"),
        "expected inlined `fromCodePoint`:\n{js}"
    );
}

/// The generic `Array` intrinsics (spec 0021) monomorphize and inline: the bare
/// `array_length` / `array_push` and the raw `array_get_unchecked` leave no
/// intrinsic name in the artifact. `array_get` is a safe `pub fn` wrapper that
/// returns `Option<T>`, so its name survives (it is not an intrinsic).
#[test]
fn js_backend_inlines_generic_array_intrinsics() {
    let output = build_single(
        "fn main() -> Int uses {} {\n    let xs: Array<Int> = [1, 2, 3]\n    let ys = array_push(xs, 4)\n    array_length(ys)\n}\n",
        "js-node",
        None,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let js = String::from_utf8(output.stdout).unwrap();
    assert!(
        !js.contains("array_length") && !js.contains("array_push"),
        "array intrinsics were not inlined:\n{js}"
    );
    assert!(js.contains(".length"), "expected inlined `.length`:\n{js}");
}

/// The safe `array_get` wrapper (spec 0011) returns `Option<T>`, resolvable by
/// bare name with no import (it is a Core Prelude `pub fn`). It bottoms out in
/// the raw `array_get_unchecked` intrinsic, which inlines away, and builds an
/// `Option` value at the call site.
#[test]
fn js_backend_safe_array_get_wraps_raw_intrinsic() {
    let source = "fn at(xs: Array<Int>, i: Int) -> Int uses {} {\n    match array_get(xs, i) {\n        Some(v) -> v\n        None -> 0 - 1\n    }\n}\nfn main() -> Int uses {} {\n    let xs: Array<Int> = [10, 20, 30]\n    at(xs, 1) + at(xs, 9)\n}\n";
    let output = build_single(source, "js-node", None);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let js = String::from_utf8(output.stdout).unwrap();
    assert!(
        !js.contains("array_get_unchecked"),
        "raw accessor was not inlined:\n{js}"
    );
    // The safe wrapper is a real (monomorphized) function, not an intrinsic.
    assert!(
        js.contains("function array_get"),
        "expected the `array_get` wrapper to survive as a function:\n{js}"
    );
}

/// The embedded `std.string` / `std.float` wrappers (spec 0038) resolve with
/// no `--package` and their intrinsics inline: `f64_sqrt` becomes `Math.sqrt`
/// on the JS backend.
#[test]
fn embedded_std_intrinsics_build_on_js() {
    let output = build_single(
        "import std.string\nimport std.float\n\nfn main() -> Int uses {} {\n    if float.sqrt(4.0) < 3.0 {\n        string.length(\"hello\")\n    } else {\n        0\n    }\n}\n",
        "js-node",
        None,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let js = String::from_utf8(output.stdout).unwrap();
    assert!(js.contains("Math.sqrt"), "expected inlined sqrt:\n{js}");
    assert!(!js.contains("f64_sqrt"), "intrinsic was not inlined:\n{js}");
}

/// The same program builds to a well-formed wasm module: the wasm backend
/// supplies `f64_sqrt` and the structural string intrinsics.
#[test]
fn embedded_std_intrinsics_build_on_wasm() {
    let dir = temp_dir();
    let out = dir.join("out.wasm");
    let output = build_single(
        "import std.string\nimport std.float\n\nfn main() -> Int uses {} {\n    if float.sqrt(4.0) < 3.0 {\n        string.length(\"hello\")\n    } else {\n        0\n    }\n}\n",
        "wasm-wasi",
        Some(&out),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = fs::read(&out).unwrap();
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(&bytes[0..4], b"\0asm");
}

/// An `intrinsic fn` in the compilation root is rejected (spec 0038): only
/// the embedded std declares intrinsics.
#[test]
fn intrinsic_in_root_source_is_rejected() {
    let dir = temp_dir();
    let input = dir.join("main.emel");
    fs::write(
        &input,
        "intrinsic fn i32_add(a: Int, b: Int) -> Int uses {}\n\nfn main() -> Int uses {} {\n    0\n}\n",
    )
    .unwrap();
    let output = emela().arg("check").arg(&input).output().unwrap();
    let _ = fs::remove_dir_all(&dir);
    assert!(!output.status.success(), "expected a compile error");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Intrinsic outside the embedded std"),
        "unexpected diagnostic:\n{stderr}"
    );
}

/// An `intrinsic fn` in a package module is rejected the same way — even when
/// it names a real intrinsic and the package is not called `std`.
#[test]
fn intrinsic_in_package_module_is_rejected() {
    let dir = temp_dir();
    let package = dir.join("mathx");
    fs::create_dir_all(package.join("src")).unwrap();
    fs::write(
        package.join("emela-package.json"),
        r#"{"name":"mathx","source":"src"}"#,
    )
    .unwrap();
    fs::write(
        package.join("src").join("num.emel"),
        "module num\n\nintrinsic fn f64_sqrt(x: Float) -> Float uses {}\n\npub fn root(x: Float) -> Float uses {} {\n    f64_sqrt(x)\n}\n",
    )
    .unwrap();
    let app = dir.join("main.emel");
    fs::write(
        &app,
        "import mathx.num.root\n\nfn main() -> Int uses {} {\n    if root(4.0) < 3.0 {\n        1\n    } else {\n        0\n    }\n}\n",
    )
    .unwrap();
    let output = emela()
        .arg("check")
        .arg("--package")
        .arg(&package)
        .arg(&app)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(&dir);
    assert!(!output.status.success(), "expected a compile error");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Intrinsic outside the embedded std"),
        "unexpected diagnostic:\n{stderr}"
    );
}
