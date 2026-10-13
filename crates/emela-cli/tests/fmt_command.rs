//! Integration tests that run `emela fmt` as a process.
//!
//! Covers target selection, rewriting, the `--check` exit code, files with syntax errors, and
//! stdin/stdout. The formatting itself is tested in emela-fmt.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const MESSY: &str = "fn main( ) {1}\n";
const CLEAN: &str = "fn main() { 1 }\n";
const BROKEN: &str = "fn broken( {\n";

struct Project {
    dir: PathBuf,
}

impl Project {
    fn new(name: &str, files: &[(&str, &str)]) -> Self {
        let dir = std::env::temp_dir().join(format!("emela-cli-fmt-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, text) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        std::fs::create_dir_all(&dir).unwrap();
        Project { dir }
    }

    fn emela(&self, args: &[&str]) -> Run {
        self.emela_in(&self.dir, args, None)
    }

    fn emela_in(&self, cwd: &Path, args: &[&str], stdin: Option<&str>) -> Run {
        let mut child = Command::new(env!("CARGO_BIN_EXE_emela"))
            .args(args)
            .current_dir(cwd)
            .env("NO_COLOR", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut pipe = child.stdin.take().unwrap();
        if let Some(input) = stdin {
            pipe.write_all(input.as_bytes()).unwrap();
        }
        drop(pipe);
        let output = child.wait_with_output().unwrap();
        Run {
            code: output.status.code(),
            stdout: String::from_utf8(output.stdout).unwrap(),
            stderr: String::from_utf8(output.stderr).unwrap(),
        }
    }

    fn read(&self, path: &str) -> String {
        std::fs::read_to_string(self.dir.join(path)).unwrap()
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[derive(Debug)]
struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

#[test]
fn formats_project_sources_in_place() {
    let project = Project::new(
        "project",
        &[
            ("Pome.toml", ""),
            ("src/main.emel", MESSY),
            ("src/http/client.emel", MESSY),
            // Outside the source root, so not formatted.
            ("scripts/x.emel", MESSY),
        ],
    );
    let run = project.emela(&["fmt"]);
    assert_eq!(
        (run.code, run.stdout.as_str(), run.stderr.as_str()),
        (Some(0), "", "")
    );
    assert_eq!(project.read("src/main.emel"), CLEAN);
    assert_eq!(project.read("src/http/client.emel"), CLEAN);
    assert_eq!(project.read("scripts/x.emel"), MESSY);
}

#[test]
fn formats_given_paths_from_inside_project() {
    let project = Project::new(
        "paths",
        &[
            ("Pome.toml", ""),
            ("src/main.emel", MESSY),
            ("src/http/client.emel", MESSY),
            ("src/http/server.emel", MESSY),
        ],
    );
    let run = project.emela_in(
        &project.dir.join("src"),
        &["fmt", "main.emel", "http/client.emel"],
        None,
    );
    assert_eq!((run.code, run.stderr.as_str()), (Some(0), ""));
    assert_eq!(project.read("src/main.emel"), CLEAN);
    assert_eq!(project.read("src/http/client.emel"), CLEAN);
    assert_eq!(project.read("src/http/server.emel"), MESSY);
    // A directory means everything under it.
    let run = project.emela(&["fmt", "src/http"]);
    assert_eq!(run.code, Some(0));
    assert_eq!(project.read("src/http/server.emel"), CLEAN);
}

#[test]
fn check_lists_unformatted_files() {
    let project = Project::new(
        "check",
        &[
            ("Pome.toml", ""),
            ("src/main.emel", CLEAN),
            ("src/a.emel", MESSY),
            ("src/b/c.emel", MESSY),
        ],
    );
    let run = project.emela(&["fmt", "--check"]);
    assert_eq!(
        (run.code, run.stdout.as_str(), run.stderr.as_str()),
        (Some(1), "src/a.emel\nsrc/b/c.emel\n", "")
    );
    assert_eq!(project.read("src/a.emel"), MESSY);

    assert_eq!(project.emela(&["fmt"]).code, Some(0));
    let run = project.emela(&["fmt", "--check"]);
    assert_eq!(
        (run.code, run.stdout.as_str(), run.stderr.as_str()),
        (Some(0), "", "")
    );
}

#[test]
fn syntax_errors_are_reported_and_other_files_are_formatted() {
    let project = Project::new(
        "syntax",
        &[
            ("Pome.toml", ""),
            ("src/main.emel", MESSY),
            ("src/broken.emel", BROKEN),
            ("src/z.emel", "const X = $\n"),
        ],
    );
    let run = project.emela(&["fmt"]);
    assert_eq!(run.code, Some(1));
    assert_eq!(run.stdout, "");
    insta::assert_snapshot!(run.stderr);
    assert_eq!(project.read("src/main.emel"), CLEAN);
    assert_eq!(project.read("src/broken.emel"), BROKEN);

    // `--check` reports the same diagnostics and lists the files that need formatting.
    let run = project.emela(&["fmt", "--check", "src/broken.emel", "src/z.emel"]);
    assert_eq!((run.code, run.stdout.as_str()), (Some(1), ""));
    assert!(run.stderr.starts_with("error[E0110]: "), "{}", run.stderr);
    assert!(run.stderr.ends_with("\n5 errors\n"), "{}", run.stderr);
}

#[test]
fn unchanged_files_are_not_written() {
    let project = Project::new("unchanged", &[("Pome.toml", ""), ("src/main.emel", CLEAN)]);
    let path = project.dir.join("src/main.emel");
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();
    // Read-only, so a write fails with E0903.
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&path, permissions.clone()).unwrap();

    let run = project.emela(&["fmt"]);
    assert_eq!((run.code, run.stderr.as_str()), (Some(0), ""));
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        before
    );

    // A write failure is a diagnostic only when the file needs formatting.
    std::fs::write(project.dir.join("src/a.emel"), MESSY).unwrap();
    let a = project.dir.join("src/a.emel");
    std::fs::set_permissions(&a, permissions.clone()).unwrap();
    let run = project.emela(&["fmt"]);
    assert_eq!(run.code, Some(1));
    assert!(
        run.stderr.starts_with("error[E0903]: cannot write file: "),
        "{}",
        run.stderr
    );
    assert!(run.stderr.contains("─[ src/a.emel ]"), "{}", run.stderr);
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    std::fs::set_permissions(&path, permissions.clone()).unwrap();
    std::fs::set_permissions(&a, permissions).unwrap();
}

#[test]
fn single_file_without_project() {
    let project = Project::new("single", &[("main.emel", MESSY), ("util.emel", MESSY)]);
    let run = project.emela(&["fmt", "main.emel"]);
    assert_eq!((run.code, run.stderr.as_str()), (Some(0), ""));
    assert_eq!(project.read("main.emel"), CLEAN);
    // A single file formats only the files passed.
    assert_eq!(project.read("util.emel"), MESSY);
    // A directory needs Pome.toml, as with `check`.
    let run = project.emela(&["fmt"]);
    assert_eq!(run.code, Some(1));
    assert!(run.stderr.starts_with("error[E0207]: "), "{}", run.stderr);
}

#[test]
fn file_outside_source_root_is_an_error() {
    let project = Project::new(
        "outside",
        &[
            ("Pome.toml", ""),
            ("src/main.emel", MESSY),
            ("scripts/x.emel", MESSY),
        ],
    );
    let run = project.emela(&["fmt", "scripts/x.emel", "src/main.emel"]);
    assert_eq!(run.code, Some(1));
    assert!(
        run.stderr
            .starts_with("error[E0208]: file `scripts/x.emel` is outside the source root `src`"),
        "{}",
        run.stderr
    );
    // The remaining paths are still formatted.
    assert_eq!(project.read("src/main.emel"), CLEAN);
    assert_eq!(project.read("scripts/x.emel"), MESSY);
}

#[test]
fn stdin_to_stdout() {
    let project = Project::new("stdin", &[]);
    let run = project.emela_in(&project.dir, &["fmt", "-"], Some(MESSY));
    assert_eq!(
        (run.code, run.stdout.as_str(), run.stderr.as_str()),
        (Some(0), CLEAN, "")
    );
    // Already formatted input is returned as is.
    let run = project.emela_in(&project.dir, &["fmt", "-"], Some(CLEAN));
    assert_eq!((run.code, run.stdout.as_str()), (Some(0), CLEAN));

    // On a syntax error, writes nothing and reports a `<stdin>` diagnostic.
    let run = project.emela_in(&project.dir, &["fmt", "-"], Some("const X = $\n"));
    assert_eq!((run.code, run.stdout.as_str()), (Some(1), ""));
    assert!(
        run.stderr
            .starts_with("error[E0105]: unrecognized character `$`\n   ╭─[ <stdin>:1:11 ]\n"),
        "{}",
        run.stderr
    );
    assert!(run.stderr.ends_with("\n2 errors\n"), "{}", run.stderr);

    let run = project.emela_in(&project.dir, &["fmt", "--check", "-"], Some(MESSY));
    assert_eq!((run.code, run.stdout.as_str()), (Some(1), "<stdin>\n"));
    let run = project.emela_in(&project.dir, &["fmt", "--check", "-"], Some(CLEAN));
    assert_eq!((run.code, run.stdout.as_str()), (Some(0), ""));

    // `-` cannot be mixed with other paths.
    let run = project.emela_in(&project.dir, &["fmt", "-", "a.emel"], Some(""));
    assert_eq!(run.code, Some(2));
    assert!(run.stderr.contains("cannot be combined"), "{}", run.stderr);
}
