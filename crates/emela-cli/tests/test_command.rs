//! `emela test` をプロセスとして起動する結合テスト．
//!
//! 引数の解釈，`@test` の検査（E0226），ソースのテスト関数を lowering して node で
//! 実行するところまでを見る．結果の表示の細部は emela-driver の tests/test_runner.rs が見る．

use std::path::PathBuf;
use std::process::Command;

struct Project {
    dir: PathBuf,
}

impl Project {
    fn new(name: &str, files: &[(&str, &str)]) -> Self {
        let dir =
            std::env::temp_dir().join(format!("emela-cli-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, text) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        Project { dir }
    }

    fn emela(&self, args: &[&str]) -> (Option<i32>, String, String) {
        let output = Command::new(env!("CARGO_BIN_EXE_emela"))
            .args(args)
            .current_dir(&self.dir)
            .env("NO_COLOR", "1")
            .output()
            .unwrap();
        (
            output.status.code(),
            String::from_utf8(output.stdout).unwrap(),
            String::from_utf8(output.stderr).unwrap(),
        )
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn invalid_test_functions_are_errors() {
    let project = Project::new(
        "invalid",
        &[
            ("Pome.toml", ""),
            (
                "src/main.emel",
                "@test\nfn takes(x: Int) {\n  assert x == 1\n}\n",
            ),
        ],
    );
    let (code, stdout, stderr) = project.emela(&["test"]);
    assert_eq!(code, Some(1));
    assert_eq!(stdout, "");
    insta::assert_snapshot!(stderr);
}

#[test]
fn valid_tests_run() {
    let project = Project::new(
        "valid",
        &[
            ("Pome.toml", ""),
            (
                "src/main.emel",
                "@test\nfn adds() {\n  assert 1 + 2 == 3\n}\n\n@test\nfn subtracts() {\n  assert 3 - 1 == 2\n}\n",
            ),
        ],
    );
    for (args, ran) in [
        (&["test"][..], 2),
        (&["test", ".", "adds"], 1),
        (&["test", "src/main.emel"], 2),
    ] {
        let (code, stdout, stderr) = project.emela(args);
        assert_eq!((code, stderr.as_str()), (Some(0), ""), "{args:?}");
        assert!(
            stdout.starts_with(&format!("running {ran} test")),
            "{stdout}"
        );
        assert!(stdout.contains("test adds ... ok"), "{stdout}");
    }
}
