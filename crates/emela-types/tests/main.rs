//! The integration tests of `emela-types`, built as one test binary: every file in this
//! directory is a module of it, so the crate and its dependencies are linked once rather than
//! once per file. A file added here is declared below;
//! `every_file_is_a_module` fails when one is missing.

mod common;

mod body;
mod capabilities;
mod effects;
mod errors;
mod export;
mod handlers;
mod lower;
mod methods;
mod signature;
mod spec_examples;
mod tables;
mod task;

/// Every `.rs` file in this directory that is not a test binary of its own is declared
/// above, and every `.rs` file in a directory of modules (one with a `mod.rs`) is declared in its
/// `mod.rs`. Without the declaration Cargo would not build it, and its tests would not run.
#[test]
fn every_file_is_a_module() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let own: &[&str] = &[];
    assert_declared(&dir, "main.rs", own);
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.join("mod.rs").is_file() {
            assert_declared(&path, "mod.rs", &[]);
        }
    }
}

/// Each `.rs` file in `dir` other than `root` and `own` is declared in `root`.
fn assert_declared(dir: &std::path::Path, root: &str, own: &[&str]) {
    let text = std::fs::read_to_string(dir.join(root)).unwrap();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(module) = name.strip_suffix(".rs") else {
            continue;
        };
        if name == root || own.contains(&module) {
            continue;
        }
        assert!(
            text.lines().any(|line| line == format!("mod {module};")),
            "{} is not declared in {root}",
            path.display()
        );
    }
}
