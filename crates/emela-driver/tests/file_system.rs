//! The bundled `Std.FileSystem` and `Std.Path` (spec 15.2) through the `emela test` path. The test
//! sources are the `.emel` files in `file_system/`, and every test function must pass.

use std::path::{Path, PathBuf};

use emela_driver::{
    CoreJs, Input, LowerTests, MemoryFiles, Output, TestOptions, TestOutcome, test,
};

/// A per-test temporary directory. Leftovers from earlier runs are removed.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("emela-driver-fs-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Runs the tests of a project with a single `main.emel`, returning the failing tests as
/// (name, outcome).
fn failures(name: &str, main: &str) -> Vec<(String, TestOutcome)> {
    let fs = MemoryFiles::new([("app/Pome.toml", ""), ("app/src/main.emel", main)]);
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let options = TestOptions {
        out_dir: temp_dir(&format!("{name}-out")),
        node: PathBuf::from("node"),
        filter: None,
        output: Output::Capture,
    };
    let mut out = Vec::new();
    let (analysis, report) = test(
        &fs,
        &input,
        &mut LowerTests::default(),
        &mut CoreJs,
        &options,
        |_| {},
        &mut out,
    );
    let _ = std::fs::remove_dir_all(&options.out_dir);
    assert_eq!(analysis.diagnostics, []);
    let report = report.unwrap();
    assert!(!report.results.is_empty());
    report
        .results
        .into_iter()
        .filter(|r| !matches!(r.outcome, TestOutcome::Passed))
        .map(|r| (r.name, r.outcome))
        .collect()
}

#[test]
fn test_file_system_follows_the_rules() {
    let main = include_str!("file_system/test_file_system.emel");
    assert_eq!(failures("test-fs", main), []);
}

#[test]
fn system_file_system_on_a_temporary_directory() {
    let root = temp_dir("system-fs");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("invalid.bin"), [0x66, 0xff, 0x6f]).unwrap();
    std::fs::write(root.join("bom.txt"), "\u{feff}x").unwrap();
    // Use `/` separators (Std.Path only handles `/`).
    let root_str = root.to_string_lossy().replace('\\', "/");
    let main = include_str!("file_system/system_file_system.emel").replace("__ROOT__", &root_str);
    let result = failures("system-fs", &main);
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(result, []);
}

#[test]
fn path_functions() {
    let main = include_str!("file_system/path.emel");
    assert_eq!(failures("path", main), []);
}
