//! Tests for embedder-defined capabilities (spec 0026).
//!
//! These tests verify that `--host-interface` activates host-provided extern
//! functions, translates them into proper WASM imports, records them in the
//! capability manifest, and rejects invalid usage.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "emela-host-iface-{label}-{}-{id}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn emela() -> Command {
    Command::new(env!("CARGO_BIN_EXE_emela"))
}

/// Writes a host interface source file (`host/<name>.emel`) into `dir`.
fn write_host_interface(dir: &Path, name: &str, body: &str) {
    let host_dir = dir.join("host");
    fs::create_dir_all(&host_dir).unwrap();
    let path = host_dir.join(format!("{name}.emel"));
    let full = format!("module host.{name}\n\n{body}\n");
    fs::write(&path, full).unwrap();
}

/// Builds a single-file wasm binary (text mode) with optional host interfaces.
// ---------------------------------------------------------------------------
// 1. Basic registration and compilation
// ---------------------------------------------------------------------------

#[test]
fn host_interface_registers_externs_and_compiles() {
    let dir = temp_dir("reg");
    write_host_interface(
        &dir,
        "gpio",
        "pub extern fn write(pin: Int, value: Bool) -> Unit uses { host.gpio }\npub extern fn read(pin: Int) -> Bool uses { host.gpio }\n",
    );
    let main = dir.join("main.emel");
    fs::write(
        &main,
        "import host.gpio\n\nfn main() -> Unit uses { host.gpio } {\n    write(13, true)\n}\n",
    )
    .unwrap();

    let output = dir.join("out.wasm");
    let result = emela()
        .arg("build")
        .arg("--backend")
        .arg("wasm-wasi")
        .arg("--host-interface")
        .arg("gpio")
        .arg("-o")
        .arg(&output)
        .arg(&main)
        .output()
        .unwrap();

    let _ = fs::remove_dir_all(&dir);
    assert!(
        result.status.success(),
        "compile failed:\n{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

// ---------------------------------------------------------------------------
// 2. Without --host-interface the host module is unknown
// ---------------------------------------------------------------------------

#[test]
fn without_flag_is_unknown_platform_error() {
    let dir = temp_dir("noflag");
    write_host_interface(
        &dir,
        "gpio",
        "pub extern fn write(pin: Int, value: Bool) -> Unit uses { host.gpio }\n",
    );
    let main = dir.join("main.emel");
    fs::write(
        &main,
        "import host.gpio\n\nfn main() -> Unit uses { host.gpio } {\n    write(13, true)\n}\n",
    )
    .unwrap();

    let output = dir.join("out.wasm");
    let result = emela()
        .arg("build")
        .arg("--backend")
        .arg("wasm-wasi")
        .arg("-o")
        .arg(&output)
        .arg(&main)
        .output()
        .unwrap();

    let _ = fs::remove_dir_all(&dir);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success(), "expected failure");
    assert!(
        stderr.contains("not a platform function"),
        "expected 'not a platform function' error:\n{stderr}"
    );
}

// ---------------------------------------------------------------------------
// 3. Effect row propagation
// ---------------------------------------------------------------------------

#[test]
fn host_interface_adds_effect_to_row() {
    let dir = temp_dir("effect");
    write_host_interface(
        &dir,
        "gpio",
        "pub extern fn write(pin: Int, value: Bool) -> Unit uses { host.gpio }\n",
    );
    let main = dir.join("main.emel");
    // The function does NOT declare `uses { host.gpio }` but calls a host
    // external — this should produce an unhandled-effects error.
    fs::write(
        &main,
        "import host.gpio\n\nfn main() -> Unit uses {} {\n    write(13, true)\n}\n",
    )
    .unwrap();

    let output = dir.join("out.wasm");
    let result = emela()
        .arg("build")
        .arg("--backend")
        .arg("wasm-wasi")
        .arg("--host-interface")
        .arg("gpio")
        .arg("-o")
        .arg(&output)
        .arg(&main)
        .output()
        .unwrap();

    let _ = fs::remove_dir_all(&dir);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success(), "expected failure");
    assert!(
        stderr.contains("Unhandled effects") || stderr.contains("host.gpio"),
        "expected unhandled-effects error mentioning host.gpio:\n{stderr}"
    );
}

// ---------------------------------------------------------------------------
// 4. Manifest includes host entries
// ---------------------------------------------------------------------------

#[test]
fn host_interface_appears_in_manifest() {
    let dir = temp_dir("manifest");
    write_host_interface(
        &dir,
        "gpio",
        "pub extern fn write(pin: Int, value: Bool) -> Unit uses { host.gpio }\n",
    );
    let main = dir.join("main.emel");
    fs::write(
        &main,
        "import host.gpio\n\nfn main() -> Unit uses { host.gpio } {\n    write(13, true)\n}\n",
    )
    .unwrap();

    let output = dir.join("out.wat");
    let result = emela()
        .arg("build")
        .arg("--backend")
        .arg("wasm-wasi")
        .arg("--emit")
        .arg("text")
        .arg("--host-interface")
        .arg("gpio")
        .arg("-o")
        .arg(&output)
        .arg(&main)
        .output()
        .unwrap();

    assert!(
        result.status.success(),
        "compile failed:\n{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let wat = fs::read_to_string(&output).unwrap();
    assert!(
        wat.contains("emela:capabilities"),
        "missing manifest section"
    );
    // The manifest is hex-encoded in the custom section (WAT format).
    // "\2e\67\70\69\6f" decodes to ".gpio", "\2e\77\72\69\74\65" to ".write".
    assert!(
        wat.contains("\\2e\\67\\70\\69\\6f\\2e\\77\\72\\69\\74\\65"),
        "manifest missing requires entry:\n...{wat}..."
    );
    assert!(
        wat.contains("\\2e\\67\\70\\69\\6f"),
        "manifest missing capability:\n...{wat}..."
    );
    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 5. WASM import is emitted
// ---------------------------------------------------------------------------

#[test]
fn host_interface_import_emitted_in_wat() {
    let dir = temp_dir("import");
    write_host_interface(
        &dir,
        "gpio",
        "pub extern fn write(pin: Int, value: Bool) -> Unit uses { host.gpio }\n",
    );
    let main = dir.join("main.emel");
    fs::write(
        &main,
        "import host.gpio\n\nfn main() -> Unit uses { host.gpio } {\n    write(13, true)\n}\n",
    )
    .unwrap();

    let output = dir.join("out.wat");
    let result = emela()
        .arg("build")
        .arg("--backend")
        .arg("wasm-wasi")
        .arg("--emit")
        .arg("text")
        .arg("--host-interface")
        .arg("gpio")
        .arg("-o")
        .arg(&output)
        .arg(&main)
        .output()
        .unwrap();

    assert!(
        result.status.success(),
        "compile failed:\n{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let wat = fs::read_to_string(&output).unwrap();
    assert!(
        wat.contains("(import \"host_gpio\" \"write\""),
        "missing host import:\n...{wat}..."
    );
    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 6. Bare `host` is rejected (MUST NOT — spec 0026)
// ---------------------------------------------------------------------------

#[test]
fn rejects_bare_host_effect() {
    let dir = temp_dir("barehost");
    let main = dir.join("main.emel");
    fs::write(&main, "fn main() -> Unit uses { host } {\n    ()\n}\n").unwrap();

    let result = emela().arg("check").arg(&main).output().unwrap();

    let _ = fs::remove_dir_all(&dir);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success(), "expected failure");
    assert!(
        stderr.contains("not a standalone capability"),
        "expected 'not a standalone capability' error:\n{stderr}"
    );
}

// ---------------------------------------------------------------------------
// 7. Invalid host interface (wrong module) is rejected
// ---------------------------------------------------------------------------

#[test]
fn rejects_host_extern_in_wrong_module() {
    let dir = temp_dir("wrongmod");
    // The extern declares module host.other, not host.gpio.
    let host_file = dir.join("host").join("gpio.emel");
    fs::create_dir_all(host_file.parent().unwrap()).unwrap();
    fs::write(
        &host_file,
        "module host.other\n\npub extern fn write(pin: Int, value: Bool) -> Unit uses { host.gpio }\n",
    )
    .unwrap();
    let main = dir.join("main.emel");
    fs::write(&main, "fn main() -> Unit uses {} {\n    ()\n}\n").unwrap();

    let output = dir.join("out.wasm");
    let result = emela()
        .arg("build")
        .arg("--backend")
        .arg("wasm-wasi")
        .arg("--host-interface")
        .arg("gpio")
        .arg("-o")
        .arg(&output)
        .arg(&main)
        .output()
        .unwrap();

    let _ = fs::remove_dir_all(&dir);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success(), "expected failure");
    assert!(
        stderr.contains("host.other"),
        "expected error about wrong module:\n{stderr}"
    );
}

// ---------------------------------------------------------------------------
// 8. Missing `uses { host.<name> }` in host extern is rejected
// ---------------------------------------------------------------------------

#[test]
fn rejects_host_extern_without_correct_uses() {
    let dir = temp_dir("baduses");
    write_host_interface(
        &dir,
        "gpio",
        "pub extern fn write(pin: Int, value: Bool) -> Unit uses { io }\n",
    );
    let main = dir.join("main.emel");
    fs::write(&main, "fn main() -> Unit uses {} {\n    ()\n}\n").unwrap();

    let output = dir.join("out.wasm");
    let result = emela()
        .arg("build")
        .arg("--backend")
        .arg("wasm-wasi")
        .arg("--host-interface")
        .arg("gpio")
        .arg("-o")
        .arg(&output)
        .arg(&main)
        .output()
        .unwrap();

    let _ = fs::remove_dir_all(&dir);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success(), "expected failure");
    assert!(
        stderr.contains("must declare `uses { host.gpio }`"),
        "expected uses-mismatch error:\n{stderr}"
    );
}

// ---------------------------------------------------------------------------
// 9. Multiple host interfaces work
// ---------------------------------------------------------------------------

#[test]
fn multiple_host_interfaces_work() {
    let dir = temp_dir("multi");
    write_host_interface(
        &dir,
        "gpio",
        "pub extern fn write(pin: Int, value: Bool) -> Unit uses { host.gpio }\n",
    );
    write_host_interface(
        &dir,
        "db",
        "pub extern fn query(sql: String) -> Int uses { host.db }\n",
    );
    let main = dir.join("main.emel");
    fs::write(
        &main,
        "import host.gpio\nimport host.db\n\nfn main() -> Unit uses { host.gpio, host.db } {\n    write(13, true)\n    let _ = query(\"SELECT 1\")\n}\n",
    )
    .unwrap();

    let output = dir.join("out.wat");
    let result = emela()
        .arg("build")
        .arg("--backend")
        .arg("wasm-wasi")
        .arg("--emit")
        .arg("text")
        .arg("--host-interface")
        .arg("gpio")
        .arg("--host-interface")
        .arg("db")
        .arg("-o")
        .arg(&output)
        .arg(&main)
        .output()
        .unwrap();

    assert!(
        result.status.success(),
        "compile failed:\n{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let wat = fs::read_to_string(&output).unwrap();
    assert!(
        wat.contains("(import \"host_gpio\" \"write\""),
        "missing gpio import"
    );
    assert!(
        wat.contains("(import \"host_db\" \"query\""),
        "missing db import"
    );
    // Manifest entries are hex-encoded in the WAT custom section.
    assert!(
        wat.contains("\\2e\\67\\70\\69\\6f\\2e\\77\\72\\69\\74\\65"),
        "manifest missing gpio"
    );
    assert!(
        wat.contains("\\2e\\64\\62\\2e\\71\\75\\65\\72\\79"),
        "manifest missing db"
    );
    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 10. Host interface with `throws` is accepted
// ---------------------------------------------------------------------------

#[test]
fn host_interface_with_throws_is_accepted() {
    let dir = temp_dir("throws");
    write_host_interface(
        &dir,
        "dev",
        "pub extern fn probe() -> Int throws HttpError uses { host.dev }\n",
    );
    let main = dir.join("main.emel");
    fs::write(
        &main,
        "import host.dev\n\nfn main() -> Unit uses { host.dev } {\n    let _ = try { probe() } catch { _ -> 0 }\n}\n",
    )
    .unwrap();

    let output = dir.join("out.wasm");
    let result = emela()
        .arg("build")
        .arg("--backend")
        .arg("wasm-wasi")
        .arg("--host-interface")
        .arg("dev")
        .arg("-o")
        .arg(&output)
        .arg(&main)
        .output()
        .unwrap();

    let _ = fs::remove_dir_all(&dir);
    assert!(
        result.status.success(),
        "compile failed:\n{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
