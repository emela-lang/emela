//! The integration tests of `emela-codegen-wasm`, built as one test binary: every file in this
//! directory is a module of it, so the crate and its dependencies are linked once rather than
//! once per file. A file added here is declared below;
//! `every_file_is_a_module` fails when one is missing.

mod common;
mod grapheme_break_test;

mod array_reads;
mod arrays;
mod bytes_stable;
mod clock;
mod component;
mod external;
mod filesystem;
mod hosts;
mod layout;
mod memory;
mod notes;
mod protocol;
mod reach;
mod run;
mod smoke;
mod suspend;
mod task;
mod unicode;
mod values;
mod values_abi;
mod values_boxes;
mod values_indirect;

/// Every `.rs` file in this directory that is not a test binary of its own is declared
/// above. Without the declaration Cargo would not build it, and its tests would not run.
#[test]
fn every_file_is_a_module() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let main = std::fs::read_to_string(dir.join("main.rs")).unwrap();
    let own: &[&str] = &[];
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
