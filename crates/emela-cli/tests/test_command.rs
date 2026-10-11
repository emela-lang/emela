//! `emela test` をプロセスとして起動する結合テスト．
//!
//! ソースからテスト関数の IR を作る lowering はまだないので，ここでは引数の解釈と，
//! `@test` の検査（E0226），検査を通った後に JS の出力がないこと（E0905）で止まることを見る．
//! IR からテストを実行する経路は emela-driver の tests/test_runner.rs が見る．

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
fn valid_tests_stop_at_the_missing_backend() {
    let project = Project::new(
        "valid",
        &[
            ("Pome.toml", ""),
            (
                "src/main.emel",
                "@test\nfn adds() {\n  assert 1 + 2 == 3\n}\n",
            ),
        ],
    );
    for args in [
        &["test"][..],
        &["test", ".", "adds"],
        &["test", "src/main.emel"],
    ] {
        let (code, stdout, stderr) = project.emela(args);
        assert_eq!(code, Some(1), "{args:?}");
        assert_eq!(stdout, "");
        assert!(stderr.starts_with("error[E0905]: "), "{stderr}");
    }
}
