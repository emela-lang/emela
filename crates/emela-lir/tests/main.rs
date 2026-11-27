//! The integration tests of `emela-lir`, built as one test binary: every file in this
//! directory and its subdirectories is a module of it, so the crate and its dependencies are
//! linked once rather than once per file. A file added here is declared below, and a file added
//! to a subdirectory in its `mod.rs`; `every_file_is_a_module` fails when one is missing.

mod common;
#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

mod analysis;
mod dump;
mod from_core;
mod heap;
mod opt;
mod verify;

/// Every `.rs` file in this directory that is not a test binary of its own is declared
/// above, and every `.rs` file in a subdirectory with a `mod.rs` is declared in that `mod.rs`.
/// Without the declaration Cargo would not build it, and its tests would not run.
#[test]
fn every_file_is_a_module() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let own: &[&str] = &[];
    check_declared(&dir, "main.rs", own);
}

/// Checks that every `.rs` file in `dir` other than `decl` and `own` is declared in `decl`, and
/// does the same for each subdirectory that has a `mod.rs`.
fn check_declared(dir: &std::path::Path, decl: &str, own: &[&str]) {
    let declared = std::fs::read_to_string(dir.join(decl)).unwrap();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.join("mod.rs").is_file() {
            check_declared(&path, "mod.rs", &[]);
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(module) = name.strip_suffix(".rs") else {
            continue;
        };
        if name == decl || own.contains(&module) {
            continue;
        }
        assert!(
            declared
                .lines()
                .any(|line| line == format!("mod {module};")),
            "{} is not declared in {}",
            path.display(),
            dir.join(decl).display()
        );
    }
}
