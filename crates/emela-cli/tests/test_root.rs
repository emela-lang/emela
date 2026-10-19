//! The test root and `[dev-dependencies]` (spec 4.7, 13.1): `emela test` and `emela check` read
//! them, `emela build` does not.

use std::path::Path;
use std::process::Command;

fn write(dir: &Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

fn emela(dir: &Path, args: &[&str]) -> (Option<i32>, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_emela"))
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .env("EMELA_POMEPROXY", "off")
        .env("EMELA_POME_CACHE", dir.join(".cache"))
        .output()
        .unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into(),
        String::from_utf8_lossy(&out.stderr).into(),
    )
}

// The app tests `Sort` from `test/` with a helper module and a generator from a test-only Pome,
// which is a `[replace]`d directory so that nothing is fetched. A host file under `test/` is
// copied into the test output.
fn project(dir: &Path) {
    write(
        dir,
        &[
            (
                "app/Pome.toml",
                "[pome]\nname = \"github.com/me/app\"\nversion = \"0.1.0\"\nemela = \"0.22.0\"\n\n[dev-dependencies]\n\"github.com/acme/gen\" = \"v0.1.0\"\n\n[replace]\n\"github.com/acme/gen\" = { path = \"../gen\" }\n",
            ),
            ("app/Pome.lock", "version = 1\n"),
            (
                "app/src/main.emel",
                "import Sort\n\nfn main() -> Int { List.length(Sort.sort([2, 1])) }\n",
            ),
            (
                "app/src/sort.emel",
                "pub fn sort(xs: List[Int]) -> List[Int] { xs }\n",
            ),
            (
                "app/test/support.emel",
                "import Gen\n\n@external(js, \"./answer.mjs\", \"answer\")\nfn answer() -> Int use {}\n\npub fn sample() -> List[Int] { Gen.list(answer()) }\n",
            ),
            (
                "app/test/answer.mjs",
                "export function answer() { return 3; }\n",
            ),
            (
                "app/test/sort_test.emel",
                "import Sort\nimport Support\n\n@test\nfn keeps_the_length() {\n  assert List.length(Sort.sort(Support.sample())) == 3\n}\n",
            ),
            (
                "gen/Pome.toml",
                "[pome]\nname = \"github.com/acme/gen\"\nversion = \"0.1.0\"\nemela = \"0.22.0\"\n",
            ),
            (
                "gen/src/lib.emel",
                "pub fn list(n: Int) -> List[Int] { List.range(0, n) }\n",
            ),
        ],
    );
}

#[test]
fn test_runs_the_test_root_with_dev_dependencies() {
    let dir = tempfile::tempdir().unwrap();
    project(dir.path());
    let app = dir.path().join("app");
    let (code, out, err) = emela(&app, &["test"]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(
        out.contains("test SortTest.keeps_the_length ... ok"),
        "{out}"
    );
    assert!(
        app.join("target/emela/test/emela_tests/answer.mjs")
            .is_file()
    );

    let (code, _, err) = emela(&app, &["check"]);
    assert_eq!(code, Some(0), "{err}");
    // build reads neither the test root nor the test-only Pome.
    let (code, _, err) = emela(&app, &["build"]);
    assert_eq!(code, Some(0), "{err}");
    assert!(!app.join("target/emela/js/emela_tests").exists());
}

#[test]
fn the_source_root_cannot_import_a_dev_dependency() {
    let dir = tempfile::tempdir().unwrap();
    project(dir.path());
    let app = dir.path().join("app");
    write(
        &app,
        &[(
            "src/main.emel",
            "import Gen\n\nfn main() -> Int { List.length(Gen.list(1)) }\n",
        )],
    );
    let (code, _, err) = emela(&app, &["check"]);
    assert_eq!(code, Some(1), "{err}");
    assert!(
        err.contains("error[E0239]: `Gen` can only be imported under `test/`"),
        "{err}"
    );
}

#[test]
fn test_runs_without_main_or_lib() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &[
            ("Pome.toml", ""),
            (
                "src/sort.emel",
                "pub fn sort(xs: List[Int]) -> List[Int] { xs }\n",
            ),
            (
                "test/sort_test.emel",
                "import Sort\n\n@test\nfn keeps_the_list() {\n  assert Sort.sort([1]) == [1]\n}\n",
            ),
        ],
    );
    let (code, out, err) = emela(dir.path(), &["test"]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(out.contains("test SortTest.keeps_the_list ... ok"), "{out}");
}
