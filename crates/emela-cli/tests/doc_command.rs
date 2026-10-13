//! Integration tests that run `emela doc` as a process.
//!
//! Covers the arguments, where the files go, and refusing to write when there are errors. What
//! the pages contain is tested in emela-driver's tests/doc.rs.

use std::path::{Path, PathBuf};
use std::process::Command;

struct Project {
    dir: PathBuf,
}

impl Project {
    fn new(name: &str, files: &[(&str, &str)]) -> Self {
        let dir = std::env::temp_dir().join(format!("emela-cli-doc-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, text) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        std::fs::create_dir_all(&dir).unwrap();
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

    fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const GREET: &[(&str, &str)] = &[
    ("Pome.toml", ""),
    (
        "src/main.emel",
        "## Greets.\npub fn greet(name: String) -> String { \"hi #{name}\" }\n\nfn main() {}\n",
    ),
];

#[test]
fn doc_writes_html_under_target() {
    let project = Project::new("html", GREET);
    let (code, stdout, stderr) = project.emela(&["doc"]);
    assert_eq!((code, stderr.as_str()), (Some(0), ""));
    let dir = project.path().join("target/emela/doc");
    assert!(stdout.trim_end().ends_with("index.html"), "{stdout}");
    for file in ["index.html", "style.css", "Main.html"] {
        assert!(dir.join(file).is_file(), "{file} missing");
    }
    let page = std::fs::read_to_string(dir.join("Main.html")).unwrap();
    assert!(page.contains("<p>Greets.</p>"), "{page}");
}

#[test]
fn doc_writes_markdown_to_out_dir() {
    let project = Project::new("md", GREET);
    let (code, _, stderr) = project.emela(&["doc", "--format", "md", "--out-dir", "docs"]);
    assert_eq!((code, stderr.as_str()), (Some(0), ""));
    let page = std::fs::read_to_string(project.path().join("docs/Main.md")).unwrap();
    assert!(
        page.contains("```emela\nfn greet(name: String) -> String\n```"),
        "{page}"
    );
}

#[test]
fn doc_writes_nothing_on_errors() {
    let project = Project::new(
        "errors",
        &[
            ("Pome.toml", ""),
            ("src/main.emel", "pub fn bad() -> Int { \"x\" }\n"),
        ],
    );
    let (code, _, stderr) = project.emela(&["doc"]);
    assert_eq!(code, Some(1));
    assert!(stderr.contains("E0301"), "{stderr}");
    assert!(!project.path().join("target/emela/doc").exists());
}

#[test]
fn doc_std_needs_no_project() {
    let project = Project::new("std", &[]);
    let (code, _, stderr) = project.emela(&["doc", "--std"]);
    assert_eq!((code, stderr.as_str()), (Some(0), ""));
    let dir = project.path().join("target/emela/doc");
    for file in ["index.html", "List.html", "Std.Clock.html"] {
        assert!(dir.join(file).is_file(), "{file} missing");
    }
}

#[test]
fn doc_std_takes_no_path() {
    let project = Project::new("std-path", GREET);
    let (code, _, stderr) = project.emela(&["doc", "--std", "."]);
    assert_eq!(code, Some(2));
    assert!(stderr.contains("cannot be used with"), "{stderr}");
}
