//! The bundled `Std.FileSystem` and `Std.Path` (spec 15.2) through the `emela test` path. The test
//! sources are the `.emel` files in `file_system/`, and every test function must pass, on JS and
//! on C (when cc starts; the C runtime with its leak check).

use std::path::{Path, PathBuf};
use std::process::Command;

use emela_driver::{
    CoreC, CoreJs, Input, LowerTests, MemoryFiles, Output, Target, TestOptions, TestOutcome,
    cc_command, test,
};

/// The C backend with the runtime's leak check, or `None` (with a message) without cc.
fn c_backend() -> Option<CoreC> {
    let (cc, mut flags) = cc_command();
    let ok = Command::new(&cc)
        .args(&flags)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!("skipping C: cannot start the C compiler `{}`", cc.display());
        return None;
    }
    flags.push("-DEMELA_LEAK_CHECK".to_owned());
    let flags: Vec<&str> = flags.iter().map(String::as_str).collect();
    Some(CoreC::with_cc(cc, &flags))
}

/// The failing tests on JS and then on C (when cc starts), as (target, name, outcome).
fn failures(name: &str, main: &str) -> Vec<(Target, String, TestOutcome)> {
    let mut found = failures_on(name, main, Target::Js);
    if c_backend().is_some() {
        found.extend(failures_on(name, main, Target::C));
    }
    found
}

/// A per-test temporary directory. Leftovers from earlier runs are removed.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("emela-driver-fs-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Runs the tests of a project with a single `main.emel` on `target`, returning the failing tests.
fn failures_on(name: &str, main: &str, target: Target) -> Vec<(Target, String, TestOutcome)> {
    let fs = MemoryFiles::new([("app/Pome.toml", ""), ("app/src/main.emel", main)]);
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let options = TestOptions {
        out_dir: temp_dir(&format!("{name}-out-{target}")),
        node: PathBuf::from("node"),
        filter: None,
        output: Output::Capture,
    };
    let mut out = Vec::new();
    let mut frontend = LowerTests::default().with_target(target);
    let (analysis, report) = match target {
        Target::Js => test(
            &fs,
            &input,
            &mut frontend,
            &mut CoreJs,
            &options,
            |_| {},
            &mut out,
        ),
        Target::C => test(
            &fs,
            &input,
            &mut frontend,
            &mut c_backend().expect("checked by the caller"),
            &options,
            |_| {},
            &mut out,
        ),
        Target::Wasm | Target::CEmbedded => unreachable!("the file system tests run js and c"),
    };
    let _ = std::fs::remove_dir_all(&options.out_dir);
    assert_eq!(analysis.diagnostics, []);
    let report = report.unwrap();
    assert!(!report.results.is_empty());
    report
        .results
        .into_iter()
        .filter(|r| !matches!(r.outcome, TestOutcome::Passed))
        .map(|r| (target, r.name, r.outcome))
        .collect()
}

#[test]
fn test_file_system_follows_the_rules() {
    let main = include_str!("file_system/test_file_system.emel");
    assert_eq!(failures("test-fs", main), []);
}

#[test]
fn system_file_system_on_a_temporary_directory() {
    let mut targets = vec![Target::Js];
    if c_backend().is_some() {
        targets.push(Target::C);
    }
    for target in targets {
        // A fresh directory per target: the tests write into it.
        let root = temp_dir(&format!("system-fs-{target}"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("invalid.bin"), [0x66, 0xff, 0x6f]).unwrap();
        std::fs::write(root.join("bom.txt"), "\u{feff}x").unwrap();
        // Use `/` separators (Std.Path only handles `/`).
        let root_str = root.to_string_lossy().replace('\\', "/");
        let main =
            include_str!("file_system/system_file_system.emel").replace("__ROOT__", &root_str);
        let result = failures_on("system-fs", &main, target);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(result, []);
    }
}

#[test]
fn path_functions() {
    let main = include_str!("file_system/path.emel");
    assert_eq!(failures("path", main), []);
}
