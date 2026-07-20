//! Tests for the capability manifest (spec 0025).
//!
//! These tests verify that the WASM binary contains the `emela:capabilities`
//! custom section, that its content matches the program's actual requirements,
//! and that the encoding is deterministic.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-manifest-{label}-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn emela() -> Command {
    Command::new(env!("CARGO_BIN_EXE_emela"))
}

/// Builds source to a wasm binary, returning the bytes and cleaning up.
fn build_wasm(source: &str) -> Vec<u8> {
    let dir = temp_dir("build");
    let input = dir.join("main.emel");
    let output = dir.join("out.wasm");
    fs::write(&input, source).unwrap();
    let result = emela()
        .arg("build")
        .arg("--backend")
        .arg("wasm-wasi")
        .arg("-o")
        .arg(&output)
        .arg(&input)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let bytes = fs::read(&output).unwrap();
    let _ = fs::remove_dir_all(&dir);
    bytes
}

/// A minimal program that uses no platform functions and no intrinsics
/// beyond what the codegen emits for the unit value.
#[test]
fn manifest_custom_section_is_present() {
    let bytes = build_wasm("fn main() -> Int { 42 }\n");
    // The custom section name must appear in the binary.
    assert!(
        bytes.windows(18).any(|w| w == b"emela:capabilities"),
        "expected emela:capabilities custom section"
    );
}

/// A program that prints to stdout uses the `io.write_stdout` platform
/// function.
#[test]
fn manifest_reflects_io_program() {
    let bytes = build_wasm(
        "import std.io\nfn main() -> Unit uses { Io } { Io.print(\"Hello\\n\") }\n",
    );
    // Parse the custom section payload.
    let manifest = extract_manifest(&bytes).expect("manifest should be parseable");
    assert_eq!(manifest["format"], serde_json::Value::Number(1.into()));
    assert!(manifest["entry"].as_bool().unwrap_or(false));
    let requires: Vec<&str> = manifest["requires"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        requires.contains(&"io.write_stdout"),
        "expected io.write_stdout in requires, got {requires:?}"
    );
    let capabilities: Vec<&str> = manifest["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        capabilities.contains(&"io"),
        "expected io capability, got {capabilities:?}"
    );
}

/// The manifest must be deterministic: building the same program twice
/// produces identical wasm bytes.
#[test]
fn manifest_is_deterministic() {
    let source = "fn main() -> Int { 42 }\n";
    let a = build_wasm(source);
    let b = build_wasm(source);
    assert_eq!(a, b, "manifest must be deterministic");
}

/// A program with no platform function usage produces an empty requires list.
#[test]
fn pure_program_has_empty_requires() {
    let bytes = build_wasm("fn main() -> Int { 1 + 2 }\n");
    let manifest = extract_manifest(&bytes).expect("manifest should be parseable");
    let requires = manifest["requires"].as_array().unwrap();
    assert!(requires.is_empty(), "expected empty requires, got {requires:?}");
    let intrinsics = manifest["intrinsics"].as_array().unwrap();
    // i32_add is used by the addition.
    let intrinsic_names: Vec<&str> = intrinsics
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        intrinsic_names.contains(&"i32_add"),
        "expected i32_add intrinsic, got {intrinsic_names:?}"
    );
}

/// A program that uses multiple platform functions should report all
/// capabilities they belong to.
#[test]
fn multiple_platform_functions() {
    let bytes = build_wasm(
        r#"import std.io
fn main() -> Unit uses { Io } {
  Io.print("hello\n")
  Io.eprint("oops\n")
}
"#,
    );
    let manifest = extract_manifest(&bytes).expect("manifest should be parseable");
    let requires: Vec<&str> = manifest["requires"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(requires.contains(&"io.write_stdout"), "missing write_stdout");
    assert!(requires.contains(&"io.write_stderr"), "missing write_stderr");
    let capabilities: Vec<&str> = manifest["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(capabilities, vec!["io"], "unexpected capabilities");
}

/// An unknown `--host-interface` name is rejected (spec 0026).
#[test]
fn host_interface_flag_rejects_unknown_name() {
    let dir = temp_dir("host-flag");
    let input = dir.join("main.emel");
    fs::write(&input, "fn main() -> Int { 42 }\n").unwrap();
    let output = emela()
        .arg("build")
        .arg("--host-interface")
        .arg("nonexistent")
        .arg(&input)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(&dir);
    assert!(
        !output.status.success(),
        "expected failure for unknown host interface"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("host interface `nonexistent` not found"),
        "stderr: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extracts and parses the `emela:capabilities` custom section from a wasm
/// binary.
fn extract_manifest(bytes: &[u8]) -> Option<serde_json::Value> {
    let payload = find_custom_section(bytes, "emela:capabilities")?;
    serde_json::from_slice(&payload).ok()
}

/// Finds the payload of a named custom section in a wasm binary.
fn find_custom_section(wasm: &[u8], name: &str) -> Option<Vec<u8>> {
    let mut offset = 8; // skip magic + version
    while offset < wasm.len() {
        let section_id = wasm[offset];
        offset += 1;
        let (size, advance) = read_leb128_u32(&wasm[offset..]);
        offset += advance;
        if section_id == 0 {
            // Custom section: read the name
            let name_len_start = offset;
            let (name_len, advance) = read_leb128_u32(&wasm[offset..]);
            offset += advance;
            let section_name = std::str::from_utf8(&wasm[offset..offset + name_len as usize]).ok()?;
            let data_start = name_len_start + advance + name_len as usize;
            let data_len = size as usize - (data_start - name_len_start);
            if section_name == name {
                return Some(wasm[data_start..data_start + data_len].to_vec());
            }
            offset = data_start + data_len;
        } else {
            offset += size as usize;
        }
    }
    None
}

/// Reads an unsigned LEB128 value from the beginning of `bytes`.
fn read_leb128_u32(bytes: &[u8]) -> (u32, usize) {
    let mut result: u32 = 0;
    let mut shift = 0u32;
    for (i, &byte) in bytes.iter().enumerate() {
        result |= ((byte & 0x7f) as u32) << shift;
        if byte & 0x80 == 0 {
            return (result, i + 1);
        }
        shift += 7;
    }
    (result, bytes.len())
}
