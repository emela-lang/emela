//! The integration tests of `emela-driver`, built as one test binary: every file under this
//! directory is a module of it, so the crate and its dependencies are linked once rather than
//! once per file. A file added here is declared below, and one added in a directory in that
//! directory's `mod.rs`; `every_file_is_a_module` fails when one is missing.

mod wasm_common;

mod check;
mod ide;
mod opt;
mod project;
mod runtime;
mod stdlib;
mod targets;

/// Every `.rs` file under this directory that is not a test binary of its own is declared: one
/// in this directory in this file, one in a directory in that directory's `mod.rs`, or else in
/// the file of the same name next to the directory (`diagnostics.rs` for `diagnostics/`).
/// Without the declaration Cargo would not build it, and its tests would not run.
#[test]
fn every_file_is_a_module() {
    use std::path::{Path, PathBuf};

    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    // The file that declares the modules of the files in `dir`.
    let declaring = |dir: &Path| {
        if dir == tests {
            return tests.join("main.rs");
        }
        let own = dir.join("mod.rs");
        if own.exists() {
            own
        } else {
            dir.with_extension("rs")
        }
    };
    let own: &[&str] = &["parity", "wasm_corpus", "wasm_parity"];
    let mut files = Vec::new();
    rust_files(&tests, &mut files);
    for path in files {
        let module = path.file_stem().unwrap().to_str().unwrap();
        let dir = path.parent().unwrap();
        if path == tests.join("main.rs") || (dir == tests && own.contains(&module)) {
            continue;
        }
        // `dir/mod.rs` is the module `dir`, declared where `dir`'s files are.
        let (module, dir) = if module == "mod" {
            (
                dir.file_name().unwrap().to_str().unwrap(),
                dir.parent().unwrap(),
            )
        } else {
            (module, dir)
        };
        let declared = std::fs::read_to_string(declaring(dir)).unwrap();
        assert!(
            declared
                .lines()
                .any(|line| line == format!("mod {module};")),
            "{} is not declared in {}",
            path.strip_prefix(&tests).unwrap().display(),
            declaring(dir).strip_prefix(&tests).unwrap().display()
        );
    }
}
