//! Embedded target selection preserves Pome restrictions and library-only builds.

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
fn dependency_targets_are_checked_with_and_without_a_board() {
    for board in [false, true] {
        for targets in [Some("c"), Some("c-embedded"), None] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            let app = root.join("app");
            write(
                root,
                "app/Pome.toml",
                "[pome]\nname = \"github.com/me/app\"\nversion = \"0.1.0\"\nemela = \"0.24.0\"\n\n[dependencies]\n\"github.com/acme/value\" = \"v0.1.0\"\n\n[replace]\n\"github.com/acme/value\" = { path = \"../value\" }\n",
            );
            write(root, "app/Pome.lock", "version = 1\n");
            // A board-selected library build must check without invoking its C compiler.
            let source = if board {
                "app/src/lib.emel"
            } else {
                "app/src/main.emel"
            };
            write(
                root,
                source,
                "import Value\npub fn main() -> Int use Async { Value.answer() }\n",
            );
            let mut manifest = "[pome]\nname = \"github.com/acme/value\"\nversion = \"0.1.0\"\nemela = \"0.24.0\"\n".to_string();
            if let Some(target) = targets {
                manifest.push_str(&format!("targets = [\"{target}\"]\n"));
            }
            write(root, "value/Pome.toml", &manifest);
            write(root, "value/src/lib.emel", "pub fn answer() -> Int { 7 }\n");
            let mut args = vec!["build", "--target", "c-embedded"];
            if board {
                args.extend(["--board", "stm32f446"]);
            }
            let output = emela(&app, &args);
            let error = String::from_utf8_lossy(&output.stderr);
            assert_eq!(
                output.status.success(),
                targets != Some("c"),
                "{board}/{targets:?}: {error}"
            );
            if targets == Some("c") {
                assert!(error.contains("E0238"), "{error}");
            } else if !board {
                assert!(
                    app.join("target/emela/c-embedded/emela_embedded.c")
                        .is_file()
                );
            } else {
                assert!(
                    !app.join("target").exists(),
                    "library-only builds must not produce firmware"
                );
            }
        }
    }
}
