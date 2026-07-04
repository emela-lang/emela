//! End-to-end tests for the `std.ord` and `std.int` utility modules: generic
//! ordering helpers over any `Ord` type (spec 0020/0027) and pure integer
//! helpers (spec 0016). Both are pure Emela — no intrinsics or platform
//! functions — and are used here across a module boundary.

use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

const ORD_MODULE: &str = "\
module ord

pub fn min<T: Ord>(a: T, b: T) -> T {
    if a < b { a } else { b }
}

pub fn max<T: Ord>(a: T, b: T) -> T {
    if a > b { a } else { b }
}

pub fn clamp<T: Ord>(value: T, low: T, high: T) -> T {
    if value < low { low } else { if value > high { high } else { value } }
}
";

const INT_MODULE: &str = "\
module int

pub fn abs(n: Int) -> Int {
    if n < 0 { 0 - n } else { n }
}

pub fn signum(n: Int) -> Int {
    if n < 0 { 0 - 1 } else { if n > 0 { 1 } else { 0 } }
}

pub fn is_even(n: Int) -> Bool { n % 2 == 0 }
pub fn is_odd(n: Int) -> Bool { n % 2 != 0 }

pub fn pow(base: Int, exp: Int) -> Int {
    if exp <= 0 { 1 } else { base * pow(base, exp - 1) }
}

pub fn gcd(a: Int, b: Int) -> Int {
    if b == 0 { a } else { gcd(b, a % b) }
}
";

/// Lays out a `std` package with `ord.emel` and `int.emel`, plus `app`.
fn util_project(app_source: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-util-test-{}-{id}", std::process::id()));
    let package = dir.join("std");
    fs::create_dir_all(package.join("src")).unwrap();
    fs::write(
        package.join("emela-package.json"),
        r#"{"name":"std","source":"src"}"#,
    )
    .unwrap();
    fs::write(package.join("src").join("ord.emel"), ORD_MODULE).unwrap();
    fs::write(package.join("src").join("int.emel"), INT_MODULE).unwrap();
    let app = dir.join("main.emel");
    fs::write(&app, app_source).unwrap();
    (package, app)
}

fn emela() -> Command {
    Command::new(env!("CARGO_BIN_EXE_emela"))
}

fn check_app(app_source: &str) {
    let (package, app) = util_project(app_source);
    let output = emela()
        .arg("check")
        .arg("--package")
        .arg(&package)
        .arg(&app)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(app.parent().unwrap());
    assert!(
        output.status.success(),
        "app should type-check:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn uses_ord_and_int_helpers_across_modules() {
    check_app(
        "\
import std.ord.max
import std.ord.clamp
import std.int.abs
import std.int.pow
import std.int.gcd

fn main() -> Int {
    max(3, 7) + clamp(15, 0, 10) + abs(0 - 8) + pow(2, 5) + gcd(48, 36)
}
",
    );
}

#[test]
fn ord_is_generic_over_any_ord_type() {
    // `max` works on `Float` too, not just `Int` (bounded generic over `Ord`).
    check_app(
        "\
import std.ord.max

fn main() -> Float uses {} {
    max(1.5, 2.5)
}
",
    );
}

#[test]
fn int_predicates_return_bool() {
    check_app(
        "\
import std.int.is_even
import std.int.is_odd

fn main() -> Bool uses {} {
    if is_even(4) { is_odd(3) } else { false }
}
",
    );
}

#[test]
fn util_modules_build_to_wasm() {
    let (package, app) = util_project(
        "\
import std.ord.max
import std.int.abs

fn main() -> Int {
    max(abs(0 - 5), 3)
}
",
    );
    let output_path = app.parent().unwrap().join("out.wasm");
    let result = emela()
        .arg("build")
        .arg("--backend")
        .arg("wasm-wasi")
        .arg("-o")
        .arg(&output_path)
        .arg("--package")
        .arg(&package)
        .arg(&app)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let bytes = fs::read(&output_path).unwrap();
    let _ = fs::remove_dir_all(app.parent().unwrap());
    assert_eq!(&bytes[0..4], b"\0asm");
}
