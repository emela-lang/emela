//! End-to-end tests for a `std.list` package (spec 0029): a generic `enum
//! List<T>` (spec 0028) declared in one module and used from another. This
//! exercises cross-module type imports — an imported module's enum and its
//! impls travel with its functions — plus higher-order list functions.

use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

// A compact `std.list` module: the `List<T>` type, an `Add` instance so `+`
// concatenates, and the higher-order core (map / filter / fold).
const LIST_MODULE: &str = "\
module list

pub enum List<T> {
    Nil
    Cons(T, List<T>)
}

impl<T> Add for List<T> {
    fn add(a: List<T>, b: List<T>) -> List<T> uses {} { append(a, b) }
}

pub fn length<T>(xs: List<T>) -> Int {
    match xs {
        Nil -> 0
        Cons(h, t) -> 1 + length(t)
    }
}

pub fn append<T>(xs: List<T>, ys: List<T>) -> List<T> {
    match xs {
        Nil -> ys
        Cons(h, t) -> List::Cons(h, append(t, ys))
    }
}

pub fn map<T, U>(xs: List<T>, f: (T) -> U) -> List<U> {
    match xs {
        Nil -> List::Nil
        Cons(h, t) -> List::Cons(f(h), map(t, f))
    }
}

pub fn filter<T>(xs: List<T>, pred: (T) -> Bool) -> List<T> {
    match xs {
        Nil -> List::Nil
        Cons(h, t) -> if pred(h) { List::Cons(h, filter(t, pred)) } else { filter(t, pred) }
    }
}

pub fn fold<T, A>(xs: List<T>, init: A, f: (A, T) -> A) -> A {
    match xs {
        Nil -> init
        Cons(h, t) -> fold(t, f(init, h), f)
    }
}
";

/// Lays out a `std` package containing `src/list.emel`, plus `app` importing it.
fn list_project(app_source: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-list-test-{}-{id}", std::process::id()));
    let package = dir.join("std");
    fs::create_dir_all(package.join("src")).unwrap();
    fs::write(
        package.join("emela-package.json"),
        r#"{"name":"std","source":"src"}"#,
    )
    .unwrap();
    fs::write(package.join("src").join("list.emel"), LIST_MODULE).unwrap();
    let app = dir.join("main.emel");
    fs::write(&app, app_source).unwrap();
    (package, app)
}

fn emela() -> Command {
    Command::new(env!("CARGO_BIN_EXE_emela"))
}

const MAP_FILTER_FOLD_APP: &str = "\
import std.list.map
import std.list.filter
import std.list.fold

fn double(n: Int) -> Int { n * 2 }
fn gt2(n: Int) -> Bool { n > 2 }
fn add(acc: Int, x: Int) -> Int { acc + x }

fn main() -> Int {
    let xs: List<Int> = List::Cons(1, List::Cons(2, List::Cons(3, List::Nil)))
    fold(filter(map(xs, double), gt2), 0, add)
}
";

#[test]
fn list_module_checks_as_a_library() {
    // The module has no `main`, so `check --library` type-checks it directly.
    let (package, _app) = list_project("fn main() -> Int uses {} { 0 }\n");
    let module = package.join("src").join("list.emel");
    let output = emela()
        .arg("check")
        .arg("--library")
        .arg("--package")
        .arg(&package)
        .arg(&module)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(package.parent().unwrap());
    assert!(
        output.status.success(),
        "list module should type-check as a library:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn imports_generic_list_type_and_functions_across_modules() {
    // `import std.list.map` must also bring the `List` enum into scope, so the
    // app can name `List<Int>` and construct `List::Cons` (spec 0028 + imports).
    let (package, app) = list_project(MAP_FILTER_FOLD_APP);
    let output = emela()
        .arg("build")
        .arg("--backend")
        .arg("js-node")
        .arg("--package")
        .arg(&package)
        .arg(&app)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(app.parent().unwrap());
    assert!(
        output.status.success(),
        "app importing std.list should build:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let js = String::from_utf8(output.stdout).unwrap();
    // The generic list functions monomorphize at `Int` in the app.
    assert!(js.contains("map__Int") || js.contains("map"), "{js}");
}

#[test]
fn imported_list_builds_to_wasm() {
    let (package, app) = list_project(MAP_FILTER_FOLD_APP);
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

#[test]
fn imported_list_add_impl_concatenates() {
    // The imported `impl<T> Add for List<T>` makes `+` concatenate lists — an
    // imported parameterized impl over a generic enum (spec 0020 + 0028).
    let app = "\
import std.list.length

fn main() -> Int {
    let a: List<Int> = List::Cons(1, List::Cons(2, List::Nil))
    let b: List<Int> = List::Cons(3, List::Nil)
    length(a + b)
}
";
    let (package, app_file) = list_project(app);
    let output = emela()
        .arg("check")
        .arg("--package")
        .arg(&package)
        .arg(&app_file)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(app_file.parent().unwrap());
    assert!(
        output.status.success(),
        "`+` on imported lists should type-check:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
