//! The integration tests of `emela-codegen-wasm`, built as one test binary: every file in this
//! directory is a module of it, so the crate and its dependencies are linked once rather than
//! once per file. A file added here is declared in the `mod.rs` of its directory, or below for a
//! directory; `every_file_is_a_module` fails when one is missing.

mod common;

mod module;
mod programs;
mod values;
mod wasi;

/// Every `.rs` file under this directory is declared by the `main.rs` or `mod.rs` beside it,
/// and every directory by the `main.rs` above it. Without the declaration Cargo would not build
/// it, and its tests would not run.
#[test]
fn every_file_is_a_module() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut dirs = vec![(root.clone(), "main.rs")];
    while let Some((dir, parent)) = dirs.pop() {
        let declared = std::fs::read_to_string(dir.join(parent)).unwrap();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let module = if path.is_dir() {
                dirs.push((path.clone(), "mod.rs"));
                name
            } else if let Some(module) = name.strip_suffix(".rs") {
                if module == "main" || module == "mod" {
                    continue;
                }
                module
            } else {
                continue;
            };
            let shown = path.strip_prefix(&root).unwrap().display();
            assert!(
                declared
                    .lines()
                    .any(|line| line == format!("mod {module};")
                        || line == format!("pub mod {module};")),
                "tests/{shown} is not declared in tests/{}",
                dir.join(parent).strip_prefix(&root).unwrap().display()
            );
        }
    }
}
