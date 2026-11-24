//! The integration tests of `emela-driver`, built as one test binary: every file in this
//! directory is a module of it, so the crate and its dependencies are linked once rather than
//! once per file. A file added here is declared below;
//! `every_file_is_a_module` fails when one is missing.

mod suspend;
mod wasm_common;

mod actions;
mod algebra;
mod backends;
mod borrow_opt;
mod c;
mod dependencies;
mod diagnostics;
mod doc;
mod file_system;
mod http_service;
mod ide_docs;
mod incomplete_impl;
mod library_project;
mod navigation;
mod opt_diff;
mod run;
mod suspend_parity;
mod task;
mod tcp;
mod test_runner;
mod trait_calls;
mod traits;
mod wasm;
mod wasm_examples;

/// Every `.rs` file in this directory that is not a test binary of its own is declared
/// above. Without the declaration Cargo would not build it, and its tests would not run.
#[test]
fn every_file_is_a_module() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let main = std::fs::read_to_string(dir.join("main.rs")).unwrap();
    let own: &[&str] = &["parity", "wasm_corpus", "wasm_parity"];
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(module) = name.strip_suffix(".rs") else {
            continue;
        };
        if module == "main" || own.contains(&module) {
            continue;
        }
        assert!(
            main.lines().any(|line| line == format!("mod {module};")),
            "tests/{name} is not declared in tests/main.rs"
        );
    }
}
