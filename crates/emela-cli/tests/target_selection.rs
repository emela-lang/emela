//! Target selection rejects retired embedded commands and preserves ordinary project builds.

use std::path::Path;
use std::process::{Command, Output};

fn write(root: &Path, name: &str, text: &str) {
    let path = root.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn emela(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_emela"))
        .args(args)
        .current_dir(root)
        .env("NO_COLOR", "1")
        .env("EMELA_POMEPROXY", "off")
        .env("EMELA_POME_CACHE", root.join(".cache"))
        .output()
        .unwrap()
}

#[test]
fn retired_embedded_cli_is_rejected_before_building() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "Pome.toml", "");
    write(root, "src/main.emel", "fn main() {}\n");
    for args in [
        &["build", "--target", "c-embedded"][..],
        &["run", "--target", "c-embedded"],
        &["test", "--target", "c-embedded"],
        &["check", "--board", "stm32f446re"],
        &["build", "--board", "stm32f446re"],
        &["build", "--embedded-config", "runtime.toml"],
        &["flash", "--dry-run"],
    ] {
        let output = emela(root, args);
        assert_eq!(output.status.code(), Some(2), "{args:?}: {output:?}");
        assert!(!root.join("target").exists(), "{args:?}");
    }
}

#[test]
fn retired_project_target_is_rejected_without_output() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "Pome.toml", "[build]\ntarget = \"c-embedded\"\n");
    write(root, "src/main.emel", "fn main() {}\n");
    for command in ["build", "run", "test"] {
        let output = emela(root, &[command]);
        let error = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{command}: {output:?}");
        assert!(error.contains("E0232"), "{command}: {error}");
        assert!(error.contains("unknown target `c-embedded`"), "{error}");
        assert!(!root.join("target").exists(), "{command}");
    }
}

#[test]
fn embedded_table_no_longer_selects_a_board_or_replaces_modules() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Pome.toml",
        "[embedded]\nboard = \"retired-board\"\n[embedded.modules]\nHardware = \"missing/{board}.emel\"\n",
    );
    write(
        root,
        "src/main.emel",
        "import Hardware\nfn main() { Hardware.answer() }\n",
    );
    write(root, "src/hardware.emel", "pub fn answer() {}\n");
    for command in ["check", "build"] {
        let output = emela(root, &[command]);
        assert!(output.status.success(), "{command}: {output:?}");
    }
    assert!(root.join("target/emela/js/main.mjs").is_file());
    assert!(!root.join("target/emela/c-embedded").exists());
}

#[test]
fn embedded_std_modules_are_no_longer_bundled() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "Pome.toml", "");
    write(
        root,
        "src/main.emel",
        "import Std.Embedded.Gpio\nfn main() {}\n",
    );
    let output = emela(root, &["check"]);
    let error = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(error.contains("Std.Embedded.Gpio"), "{error}");
    assert!(!root.join("target").exists());
}

#[test]
fn dependency_targets_are_checked_for_library_only_builds() {
    for target in ["js", "native", "wasm"] {
        for supported in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            let app = root.join("app");
            write(
                root,
                "app/Pome.toml",
                "[pome]\nname = \"github.com/me/app\"\nversion = \"0.1.0\"\nemela = \"0.27.0\"\n\n[dependencies]\n\"github.com/acme/value\" = \"v0.1.0\"\n\n[replace]\n\"github.com/acme/value\" = { path = \"../value\" }\n",
            );
            write(root, "app/Pome.lock", "version = 1\n");
            write(
                root,
                "app/src/lib.emel",
                "import Value\npub fn answer() -> Int { Value.answer() }\n",
            );
            let declared = if supported {
                target
            } else if target == "js" {
                "native"
            } else {
                "js"
            };
            write(
                root,
                "value/Pome.toml",
                &format!(
                    "[pome]\nname = \"github.com/acme/value\"\nversion = \"0.1.0\"\nemela = \"0.27.0\"\ntargets = [\"{declared}\"]\n"
                ),
            );
            write(root, "value/src/lib.emel", "pub fn answer() -> Int { 7 }\n");
            let output = emela(&app, &["build", "--target", target]);
            let error = String::from_utf8_lossy(&output.stderr);
            assert_eq!(
                output.status.success(),
                supported,
                "{target}/{supported}: {error}"
            );
            if !supported {
                assert!(error.contains("E0238"), "{error}");
            }
            assert!(!app.join("target").exists(), "{target}/{supported}");
        }
    }
}

/// `[target.native]`, `[cmd.build]`: `--target native` and the deprecated `--target c` build the
/// executable `main` and the header `emela.h` into `target/emela/native`; `c` reports W0903 first.
#[test]
fn the_native_target_and_its_deprecated_name() {
    let have_cc = Command::new("cc")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !have_cc {
        eprintln!("skipping: cannot start the C compiler `cc`");
        return;
    }
    for (name, warns) in [("native", false), ("c", true)] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "Pome.toml", "");
        write(root, "src/main.emel", "fn main() {}\n");
        let output = emela(root, &["build", "--target", name]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{name}: {stderr}");
        assert_eq!(
            stderr.starts_with(
                "warning[W0903]: target `c` is deprecated; use `native`\n   = help: `c` is removed in 0.29\n"
            ),
            warns,
            "{name}: {stderr}"
        );
        assert!(root.join("target/emela/native/main").is_file(), "{name}");
        assert!(root.join("target/emela/native/emela.h").is_file(), "{name}");
        assert!(!root.join("target/emela/c").exists(), "{name}");
    }
}

/// `[cmd.test.out-dir]`: the test output of the native target is `target/emela/test-native`.
#[test]
fn native_tests_build_into_test_native() {
    let have_cc = Command::new("cc")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !have_cc {
        eprintln!("skipping: cannot start the C compiler `cc`");
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "Pome.toml", "");
    write(
        root,
        "src/main.emel",
        "@test\nfn works() { assert 1 == 1 }\n",
    );
    let output = emela(root, &["test", "--target", "native"]);
    assert!(output.status.success(), "{output:?}");
    assert!(root.join("target/emela/test-native/main").is_file());
}
