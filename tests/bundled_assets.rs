use std::fs;
use std::process::Command;

#[test]
fn compiler_binary_uses_embedded_stdlib_from_other_cwd() {
    let temp = std::env::temp_dir().join(format!("emela-bundled-test-{}", std::process::id()));
    fs::create_dir_all(&temp).unwrap();
    let source = temp.join("main.emel");
    fs::write(
        &source,
        r##"
import std.io.write_stdout_utf8!

fn main!() -> Result<Unit, PlatformError> {
  write_stdout_utf8!("hello")
}
"##,
    )
    .unwrap();

    let status = Command::new(env!("CARGO_BIN_EXE_compiler"))
        .current_dir(&temp)
        .arg("--backend")
        .arg("js-node")
        .arg("--check")
        .arg(&source)
        .status()
        .unwrap();

    let _ = fs::remove_file(&source);
    let _ = fs::remove_dir(&temp);
    assert!(status.success());
}

#[test]
fn compiler_binary_imports_source_package() {
    let temp = std::env::temp_dir().join(format!("emela-package-test-{}", std::process::id()));
    let package = temp.join("math");
    fs::create_dir_all(package.join("src")).unwrap();
    fs::write(
        package.join("emela-package.json"),
        r#"{"name":"math","source":"src"}"#,
    )
    .unwrap();
    fs::write(
        package.join("src/ops.emel"),
        r#"
fn add_one(value: I32) -> I32 {
  value + 1
}
"#,
    )
    .unwrap();
    let source = temp.join("main.emel");
    fs::write(
        &source,
        r#"
import math.ops.add_one

fn main() -> I32 {
  add_one(41)
}
"#,
    )
    .unwrap();

    let status = Command::new(env!("CARGO_BIN_EXE_compiler"))
        .current_dir(&temp)
        .arg("--backend")
        .arg("js-node")
        .arg("--package")
        .arg(&package)
        .arg("--check")
        .arg(&source)
        .status()
        .unwrap();

    let _ = fs::remove_file(&source);
    let _ = fs::remove_dir_all(&temp);
    assert!(status.success());
}

#[test]
fn compiler_binary_can_use_std_as_external_package() {
    let temp = std::env::temp_dir().join(format!("emela-std-package-test-{}", std::process::id()));
    fs::create_dir_all(&temp).unwrap();
    let source = temp.join("main.emel");
    fs::write(
        &source,
        r##"
import std.io.write_stdout_utf8!

fn main!() -> Result<Unit, PlatformError> {
  write_stdout_utf8!("hello")
}
"##,
    )
    .unwrap();
    let stdlib = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../stdlib");

    let status = Command::new(env!("CARGO_BIN_EXE_compiler"))
        .current_dir(&temp)
        .arg("--backend")
        .arg("js-node")
        .arg("--package")
        .arg(stdlib)
        .arg("--check")
        .arg(&source)
        .status()
        .unwrap();

    let _ = fs::remove_file(&source);
    let _ = fs::remove_dir(&temp);
    assert!(status.success());
}

#[test]
fn compiler_binary_rejects_removed_stdlib_option() {
    let temp = std::env::temp_dir().join(format!(
        "emela-removed-stdlib-option-test-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp).unwrap();
    let source = temp.join("main.emel");
    fs::write(
        &source,
        r#"
fn main() -> Unit {
}
"#,
    )
    .unwrap();

    let status = Command::new(env!("CARGO_BIN_EXE_compiler"))
        .current_dir(&temp)
        .arg("--backend")
        .arg("js-node")
        .arg("--stdlib")
        .arg(".")
        .arg("--check")
        .arg(&source)
        .status()
        .unwrap();

    let _ = fs::remove_file(&source);
    let _ = fs::remove_dir(&temp);
    assert!(!status.success());
}
