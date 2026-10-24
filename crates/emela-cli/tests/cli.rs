//! Integration tests that run `emela` as a process.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// A per-test project. Leftovers are removed on creation, and the directory on drop.
struct Project {
    dir: PathBuf,
}

impl Project {
    fn new(name: &str, files: &[(&str, &str)]) -> Self {
        // A unique directory per project: two tests may pick the same readable `name`, and the
        // `remove_dir_all` here (and on drop) must not delete another test's files while its
        // `node` is still running in them (`ENOENT: process.cwd`).
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("emela-cli-{}-{name}-{n}", std::process::id()));
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
        // `emela test` prints in the `plain` format outside a coding agent ([cmd.test.reporter]).
        let mut command = Command::new(env!("CARGO_BIN_EXE_emela"));
        for var in emela_driver::AGENT_ENV_VARS {
            command.env_remove(var);
        }
        let output = command
            .args(args)
            .current_dir(&self.dir)
            .env("NO_COLOR", "1")
            .output()
            .unwrap();
        Run {
            code: output.status.code(),
            stdout: String::from_utf8(output.stdout).unwrap(),
            stderr: String::from_utf8(output.stderr).unwrap(),
        }
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

#[derive(Debug)]
struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

const BROKEN: &[(&str, &str)] = &[
    ("Pome.toml", ""),
    ("src/main.emel", "fn main() {\n  \"abc\n}\nconst Y = $\n"),
    ("src/http/client.emel", "const N = 3px\n"),
    ("src/Util.emel", ""),
];

/// `--help` is written in English, using the same words as the Commands table in the README.
#[test]
fn help_is_english() {
    let project = Project::new("help", &[]);
    for command in ["", "new", "check", "build", "run", "test", "fmt", "lsp"] {
        let args: Vec<&str> = [command, "--help"]
            .into_iter()
            .filter(|a| !a.is_empty())
            .collect();
        let run = project.emela(&args);
        assert_eq!(run.code, Some(0), "{command}: {run:?}");
        assert!(run.stdout.is_ascii(), "{command}: {}", run.stdout);
    }
}

/// A new application passes check, run, test and fmt as created.
#[test]
fn new_application_runs() {
    let project = Project::new("new-app", &[]);
    let run = project.emela(&["new", "hello"]);
    assert_eq!(run.code, Some(0), "{run:?}");
    assert_eq!(run.stderr, "created application `hello`\n");
    for args in [
        &["check", "hello"][..],
        &["test", "hello"],
        &["fmt", "--check", "hello"],
    ] {
        let run = project.emela(args);
        assert_eq!(run.code, Some(0), "{args:?}: {run:?}");
    }
    let run = project.emela(&["run", "hello"]);
    assert_eq!(
        (run.code, run.stdout.as_str()),
        (Some(0), "Hello, world!\n")
    );
}

/// A new library has one passing test.
#[test]
fn new_library_tests() {
    let project = Project::new("new-lib", &[]);
    let run = project.emela(&["new", "github:alice/json-codec", "--lib"]);
    assert_eq!(run.code, Some(0), "{run:?}");
    assert_eq!(run.stderr, "created library `json-codec`\n");
    for args in [
        &["check", "json-codec"][..],
        &["fmt", "--check", "json-codec"],
    ] {
        let run = project.emela(args);
        assert_eq!(run.code, Some(0), "{args:?}: {run:?}");
    }
    let run = project.emela(&["test", "json-codec"]);
    assert_eq!(run.code, Some(0), "{run:?}");
    assert!(run.stdout.contains("1 passed"), "{}", run.stdout);
}

/// An existing directory, even an empty one, is left as it is.
#[test]
fn new_keeps_existing_directory() {
    let project = Project::new("new-exists", &[("hello/keep.txt", "kept\n")]);
    let run = project.emela(&["new", "hello"]);
    assert_eq!(run.code, Some(1), "{run:?}");
    let entries: Vec<_> = std::fs::read_dir(project.path().join("hello"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(entries, ["keep.txt"]);
    assert_eq!(
        std::fs::read_to_string(project.path().join("hello/keep.txt")).unwrap(),
        "kept\n"
    );

    std::fs::create_dir(project.path().join("empty")).unwrap();
    let run = project.emela(&["new", "empty"]);
    assert_eq!(run.code, Some(1), "{run:?}");
    assert_eq!(
        std::fs::read_dir(project.path().join("empty"))
            .unwrap()
            .count(),
        0
    );
}

/// A rejected name creates nothing.
#[test]
fn new_rejects_name() {
    let project = Project::new("new-bad", &[]);
    let run = project.emela(&["new", "emela-json"]);
    assert_eq!(run.code, Some(1), "{run:?}");
    assert_eq!(std::fs::read_dir(project.path()).unwrap().count(), 0);
}

#[test]
fn check_reports_all_errors() {
    let project = Project::new("broken", BROKEN);
    let run = project.emela(&["check"]);
    assert_eq!(run.code, Some(1));
    assert_eq!(run.stdout, "");
    insta::assert_snapshot!(run.stderr);
}

#[test]
fn incomplete_impl_is_rejected_by_every_command_and_target() {
    let project = Project::new(
        "incomplete-impl",
        &[
            ("Pome.toml", ""),
            (
                "src/main.emel",
                "type User(id: Int)\ntrait Label {\n  fn label(self) -> String\n  fn merge(self, other: Self) -> Self\n  fn default(self) -> Int { 1 }\n}\nimpl User: Label {}\nfn main() {}\n@test\nfn works() {}\n",
            ),
        ],
    );
    let check = project.emela(&["check"]);
    assert_eq!(check.code, Some(1), "{check:?}");
    assert!(
        check
            .stderr
            .contains("error[E0325]: incomplete impl: missing methods `label`, `merge`")
    );
    assert!(!check.stderr.contains("`default`"));
    for command in ["build", "run", "test"] {
        for target in ["js", "c", "wasm"] {
            let run = project.emela(&[command, "--target", target]);
            assert_eq!(run.code, Some(1), "{command} {target}: {run:?}");
            assert_eq!(run.stderr, check.stderr, "{command} {target}");
            assert!(run.stdout.is_empty(), "{command} {target}: {run:?}");
        }
    }
    assert!(!project.path().join("target").exists());
}

#[test]
fn check_file_and_directory_agree() {
    let project = Project::new("agree", BROKEN);
    let by_dir = project.emela(&["check", "."]);
    let by_file = project.emela(&["check", "src/main.emel"]);
    assert_eq!(by_dir.stderr, by_file.stderr);
    assert_eq!(by_file.code, Some(1));
}

#[test]
fn check_clean_project() {
    let project = Project::new(
        "clean",
        &[
            ("Pome.toml", ""),
            ("src/main.emel", "import Json\n\nfn main() { 1 }\n"),
            ("src/json.emel", ""),
        ],
    );
    let run = project.emela(&["check"]);
    assert_eq!(
        (run.code, run.stdout.as_str(), run.stderr.as_str()),
        (Some(0), "", "")
    );
    // Same when the directory is passed from outside the project.
    let parent = project.path().parent().unwrap();
    let run = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("check")
        .arg(project.path())
        .current_dir(parent)
        .output()
        .unwrap();
    assert_eq!(run.status.code(), Some(0));
}

#[test]
fn check_missing_path() {
    let project = Project::new("missing", &[]);
    let run = project.emela(&["check", "nowhere.emel"]);
    assert_eq!(run.code, Some(1));
    insta::assert_snapshot!(run.stderr, @"
    error[E0901]: `nowhere.emel` not found

    1 error
    ");
}

#[test]
fn build_writes_js() {
    let project = Project::new(
        "build-js",
        &[
            ("Pome.toml", ""),
            ("src/main.emel", "fn main() {\n  dbg(1 + 2)\n}\n"),
        ],
    );
    let run = project.emela(&["build"]);
    assert_eq!((run.code, run.stderr.as_str()), (Some(0), ""));
    let js = std::fs::read_to_string(project.path().join("target/emela/js/main.mjs")).unwrap();
    assert!(js.contains("function main()"), "{js}");
    assert!(js.contains("export { main };"), "{js}");
}

#[test]
fn build_reports_unsupported_expressions() {
    let project = Project::new(
        "unsupported",
        &[
            ("Pome.toml", ""),
            // Capability values cannot be shown.
            (
                "src/main.emel",
                "import Std.Console.{Console, SystemConsole}\n\nfn main() {\n  with SystemConsole {\n    c = use Console\n    dbg(\"#{c}\")\n  }\n}\n",
            ),
        ],
    );
    for command in ["build", "run"] {
        let run = project.emela(&[command]);
        assert_eq!(run.code, Some(1), "{command}");
        assert!(
            run.stderr
                .starts_with("error: not supported yet: showing a capability value\n"),
            "{command}: {}",
            run.stderr
        );
    }
}

#[test]
fn library_only_project_uses_lib_as_entry_for_check_and_test() {
    for source in [
        "pub fn f() -> Int { 1 }\n",
        "pub fn f() -> Int { 1 }\n\n@test\nfn works() { assert f() == 1 }\n",
        "pub fn f() -> Int { 1 }\n\n@test\nfn works() { assert f() == 1 }\n\nfn main() { panic(\"must not run\") }\n",
    ] {
        let project = Project::new(
            "library-only",
            &[("Pome.toml", ""), ("src/lib.emel", source)],
        );
        let check = project.emela(&["check"]);
        assert_eq!((check.code, check.stderr.as_str()), (Some(0), ""));

        // Build before running tests so their output cannot mask an unwanted build artifact.
        for target in ["js", "c", "wasm"] {
            for path in [".", "src"] {
                let build = project.emela(&["build", path, "--target", target]);
                assert_eq!((build.code, build.stderr.as_str()), (Some(0), ""));
                assert!(!project.path().join("target").exists());
                let build =
                    project.emela(&["build", path, "--target", target, "--out-dir", "custom"]);
                assert_eq!((build.code, build.stderr.as_str()), (Some(0), ""));
                assert!(!project.path().join("custom").exists());

                let run = project.emela(&["run", path, "--target", target]);
                assert_eq!((run.code, run.stdout.as_str()), (Some(1), ""));
                assert!(
                    run.stderr
                        .starts_with("error[E0230]: entry `src/lib.emel` has no `main` function"),
                    "{}",
                    run.stderr
                );
                assert!(!project.path().join("target").exists());
            }
        }
        let test = project.emela(&["test"]);
        assert_eq!((test.code, test.stderr.as_str()), (Some(0), ""));
        if source.contains("@test") {
            assert!(test.stdout.contains("test works ... ok"), "{}", test.stdout);
        } else {
            assert!(
                test.stdout.contains("0 passed; 0 failed"),
                "{}",
                test.stdout
            );
        }
    }
}

#[test]
fn explicit_library_file_remains_a_file_entry_and_binary_entry_takes_priority() {
    let project = Project::new(
        "library-entry-selection",
        &[
            ("Pome.toml", ""),
            (
                "src/lib.emel",
                "fn main() { dbg(42) }\n@test\nfn in_lib() { assert True }\n",
            ),
        ],
    );
    let build = project.emela(&["build", "src/lib.emel"]);
    assert_eq!((build.code, build.stderr.as_str()), (Some(0), ""));
    assert!(project.path().join("target/emela/js/main.mjs").exists());
    let run = project.emela(&["run", "src/lib.emel"]);
    assert_eq!(
        (run.code, run.stdout.as_str(), run.stderr.as_str()),
        (Some(0), "", "42\n")
    );

    std::fs::write(
        project.path().join("src/main.emel"),
        "fn main() { dbg(7) }\n@test\nfn in_main() { assert True }\n",
    )
    .unwrap();
    let run = project.emela(&["run"]);
    assert_eq!(
        (run.code, run.stdout.as_str(), run.stderr.as_str()),
        (Some(0), "", "7\n")
    );
    let test = project.emela(&["test"]);
    assert_eq!((test.code, test.stderr.as_str()), (Some(0), ""));
    assert!(
        test.stdout.contains("test in_main ... ok"),
        "{}",
        test.stdout
    );
    assert!(
        test.stdout.contains("test Lib.in_lib ... ok"),
        "{}",
        test.stdout
    );
}

#[test]
fn library_type_errors_are_reported_by_all_commands() {
    let project = Project::new(
        "library-type-error",
        &[
            ("Pome.toml", ""),
            ("src/lib.emel", "pub fn f() -> Int { \"wrong\" }\n"),
        ],
    );
    for command in ["check", "test", "build", "run"] {
        let run = project.emela(&[command]);
        assert_eq!(run.code, Some(1), "{command}: {run:?}");
        assert!(
            run.stderr.starts_with("error[E0301]:"),
            "{command}: {run:?}"
        );
        assert!(!run.stderr.contains("E0230"), "{command}: {run:?}");
        assert!(!run.stderr.contains("internal error"), "{command}: {run:?}");
        assert!(!project.path().join("target").exists());
    }
}

#[test]
fn build_reports_missing_entry() {
    let project = Project::new("no-entry", &[("Pome.toml", ""), ("src/util.emel", "")]);
    for command in ["build", "run"] {
        let run = project.emela(&[command]);
        assert_eq!(run.code, Some(1));
        assert_eq!(
            run.stderr,
            "error[E0209]: the entry `src/main.emel` does not exist\n\n1 error\n"
        );
    }
    assert!(!project.path().join("target").exists());
    // `check` does not require the entry.
    assert_eq!(project.emela(&["check"]).code, Some(0));
}

#[test]
fn run_without_main_is_an_error() {
    // `run` requires `main` in the entry (spec 8.3); `build` emits output without one.
    let project = Project::new(
        "no-main",
        &[
            ("Pome.toml", ""),
            ("src/main.emel", "fn helper() {\n  dbg(1)\n}\n"),
        ],
    );
    let run = project.emela(&["run"]);
    assert_eq!((run.code, run.stdout.as_str()), (Some(1), ""));
    insta::assert_snapshot!(run.stderr, @"
    error[E0230]: entry `src/main.emel` has no `main` function
       ─[ src/main.emel ]

    1 error
    ");
    let build = project.emela(&["build"]);
    assert_eq!((build.code, build.stderr.as_str()), (Some(0), ""));
    assert!(project.path().join("target/emela/js/main.mjs").exists());
}

#[test]
fn run_single_file_without_main_is_an_error() {
    let project = Project::new("no-main-file", &[("lone.emel", "const N = 1\n")]);
    let run = project.emela(&["run", "lone.emel"]);
    assert_eq!((run.code, run.stdout.as_str()), (Some(1), ""));
    insta::assert_snapshot!(run.stderr, @"
    error[E0230]: entry `lone.emel` has no `main` function
       ─[ lone.emel ]

    1 error
    ");
    let build = project.emela(&["build", "lone.emel"]);
    assert_eq!((build.code, build.stderr.as_str()), (Some(0), ""));
}

#[test]
fn check_with_absolute_path_from_elsewhere() {
    let project = Project::new("absolute", BROKEN);
    let inside = project.emela(&["check"]);
    // A path given as absolute from another directory is still reported relative to the
    // project.
    let entry = project.path().join("src/main.emel");
    let run = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("check")
        .arg(&entry)
        .current_dir(std::env::temp_dir())
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(run.stderr).unwrap(), inside.stderr);
    // Same from a directory inside the project.
    let run = Command::new(env!("CARGO_BIN_EXE_emela"))
        .args(["check", "main.emel"])
        .current_dir(project.path().join("src"))
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(run.stderr).unwrap(), inside.stderr);
}

#[test]
fn directory_needs_pome_toml() {
    let project = Project::new("no-pome", &[("src/main.emel", "")]);
    let run = project.emela(&["check"]);
    assert_eq!(run.code, Some(1));
    insta::assert_snapshot!(run.stderr);
}

#[test]
fn file_outside_source_root() {
    let project = Project::new(
        "outside",
        &[
            ("Pome.toml", ""),
            ("src/main.emel", ""),
            ("scripts/x.emel", ""),
        ],
    );
    let run = project.emela(&["check", "scripts/x.emel"]);
    assert_eq!(run.code, Some(1));
    assert!(
        run.stderr
            .starts_with("error[E0208]: file `scripts/x.emel` is outside the source root `src`"),
        "{}",
        run.stderr
    );
}

/// check, build, run and test reject a directory outside the source root like fmt does
/// (spec appendix B). The project directory and directories under the source root still mean the
/// whole project.
#[test]
fn directory_outside_source_root() {
    let project = Project::new(
        "outside-dir",
        &[
            ("Pome.toml", ""),
            ("src/main.emel", "fn main() { () }\n"),
            ("src/http/client.emel", ""),
            ("scripts/x.emel", ""),
        ],
    );
    for command in ["check", "build", "run", "test"] {
        let run = project.emela(&[command, "scripts"]);
        assert_eq!(run.code, Some(1), "{command}: {}", run.stderr);
        assert!(
            run.stderr
                .starts_with("error[E0208]: directory `scripts` is outside the source root `src`"),
            "{command}: {}",
            run.stderr
        );
        for inside in [".", "src", "src/http"] {
            let run = project.emela(&[command, inside]);
            assert_eq!(run.code, Some(0), "{command} {inside}: {}", run.stderr);
        }
    }
}

#[test]
fn single_file_reads_only_imported_files() {
    let project = Project::new(
        "single",
        &[
            ("main.emel", "import Util\n\nfn main() { 1 }\n"),
            ("util.emel", "const Y = $\n"),
            // Not reachable from the entry, so never read.
            ("scratch.emel", "fn broken( {\n"),
            ("Bad.emel", ""),
            ("deep/broken.emel", "const Z = $\n"),
            ("Not-A-Dir/x.emel", ""),
        ],
    );
    let run = project.emela(&["check", "main.emel"]);
    assert_eq!(run.code, Some(1));
    insta::assert_snapshot!(run.stderr);
    // Passes as long as the broken file is not imported.
    let clean = Project::new(
        "single-clean",
        &[
            ("main.emel", "fn main() { 1 }\n"),
            ("scratch.emel", "fn broken( {\n"),
        ],
    );
    let run = clean.emela(&["check", "main.emel"]);
    assert_eq!((run.code, run.stderr.as_str()), (Some(0), ""));
}

#[test]
fn check_reports_syntax_and_import_errors() {
    let project = Project::new(
        "imports",
        &[
            ("Pome.toml", ""),
            (
                "src/main.emel",
                "import Http.Client.{get}\nimport Missing\n\nfn main() {\n  get(\n}\n",
            ),
            (
                "src/http/client.emel",
                "import Json\n\npub fn get(url: String) { url }\n",
            ),
            ("src/json.emel", "import Http.Client\n\nconst Z: = 1\n"),
        ],
    );
    let run = project.emela(&["check"]);
    assert_eq!(run.code, Some(1));
    insta::assert_snapshot!(run.stderr);
}

/// Runs `emela lsp`, waits for workspace diagnostics, then exits with 0 after shutdown and exit.
#[test]
fn lsp_over_stdio() {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::process::Stdio;

    let project = Project::new(
        "lsp",
        &[("Pome.toml", ""), ("src/main.emel", "const X = $\n")],
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut send = |body: String| {
        write!(stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        stdin.flush().unwrap();
    };
    let mut recv = || {
        let mut length = 0;
        loop {
            let mut line = String::new();
            stdout.read_line(&mut line).unwrap();
            let line = line.trim();
            if line.is_empty() {
                break;
            }
            if let Some(n) = line.strip_prefix("Content-Length: ") {
                length = n.parse().unwrap();
            }
        }
        let mut body = vec![0; length];
        stdout.read_exact(&mut body).unwrap();
        String::from_utf8(body).unwrap()
    };
    let root = format!("file://{}", project.path().display());
    send(format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"capabilities":{{}},"rootUri":"{root}"}}}}"#
    ));
    assert!(recv().contains(r#""textDocumentSync""#));
    send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#.to_owned());
    let published = recv();
    assert!(
        published.contains("textDocument/publishDiagnostics"),
        "{published}"
    );
    assert!(published.contains(r#""code":"E0105""#), "{published}");
    assert!(published.contains("src/main.emel"), "{published}");
    send(r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#.to_owned());
    assert!(recv().contains(r#""id":2"#));
    send(r#"{"jsonrpc":"2.0","method":"exit"}"#.to_owned());
    assert_eq!(child.wait().unwrap().code(), Some(0));
}

#[test]
fn check_reports_name_resolution_errors() {
    let project = Project::new(
        "resolve",
        &[
            ("Pome.toml", ""),
            (
                "src/email.emel",
                "pub opaque type Email(value: String)\n\npub fn parse(input: String) -> Email {\n  Email(value: input)\n}\n\nfn secret() { 1 }\n",
            ),
            (
                "src/main.emel",
                "import Email\nimport Email.{Email, secret}\n\nfn main() {\n  count = 1\n  e = Email(value: \"a\")\n  Email.parse(cout)\n}\n\nfn first[Email](e: Email) -> Email { e }\n\nfn twice() { 1 }\nfn twice() { 2 }\n",
            ),
        ],
    );
    let run = project.emela(&["check"]);
    assert_eq!(run.code, Some(1));
    assert_eq!(run.stdout, "");
    insta::assert_snapshot!(run.stderr);
}

#[test]
fn check_resolves_bundled_core() {
    let project = Project::new(
        "core",
        &[
            ("Pome.toml", ""),
            (
                "src/main.emel",
                "fn main() -> Int {\n  xs = List.map([1, 2], fn(x) { x * 2 })\n  Option.or(List.head(xs), 0) + String.length(\"abc\")\n}\n",
            ),
        ],
    );
    let run = project.emela(&["check"]);
    assert_eq!(
        (run.code, run.stdout.as_str(), run.stderr.as_str()),
        (Some(0), "", "")
    );
}

#[test]
fn check_reports_error_set_errors() {
    let project = Project::new(
        "errors",
        &[
            ("Pome.toml", ""),
            (
                "src/main.emel",
                "error NotFound(id: Int)\nerror Timeout\n\nfn find(id: Int) -> Int fails NotFound {\n  if id > 0 { id } else { fail NotFound(id:) }\n}\n\nfn strict() -> Int {\n  find(1)\n}\n\nfn wrong() -> Int fails Timeout {\n  find(1)\n}\n\nfn run(f: fn() -> Int fails {}) -> Int { f() }\n\nfn bad() -> Int { run(fn() { fail Timeout }) }\n\nfn never() -> Int { strict() escape { Timeout -> 0 } }\n\nfn not_error() { fail 1 }\n\nfn catch_all() -> Int { find(1) escape { _ -> 0 } }\n\n# main の E は空でなくてよい（14.3）\nfn main() {\n  find(1)\n}\n",
            ),
        ],
    );
    let run = project.emela(&["check"]);
    assert_eq!(run.code, Some(1));
    assert_eq!(run.stdout, "");
    insta::assert_snapshot!(run.stderr);
}

/// An error that reaches `main` prints `error: <the error's Show>` and exits with 1 (spec 14.3),
/// distinct from 101 for a defect. An error caught by `escape` becomes a normal value (spec 7.3).
#[test]
fn run_reports_unhandled_error_with_exit_code_1() {
    let project = Project::new(
        "fail-run",
        &[
            ("Pome.toml", ""),
            (
                "src/main.emel",
                "error NotFound(id: Int) derive Show\n\nfn find(id: Int) -> Int fails NotFound {\n  if id > 0 { id } else { fail NotFound(id:) }\n}\n\nfn main() {\n  dbg(find(0) escape { NotFound(_) -> 0 })\n  dbg(find(-1))\n}\n",
            ),
        ],
    );
    assert_eq!(project.emela(&["check"]).code, Some(0));
    assert_eq!(project.emela(&["build"]).code, Some(0));
    let run = project.emela(&["run"]);
    assert_eq!(
        (run.code, run.stdout.as_str(), run.stderr.as_str()),
        (Some(1), "", "0\nerror: NotFound(id: -1)\n")
    );
}

#[test]
fn run_matches_negative_number_patterns() {
    // A pattern's `-` and number are one value, compared with `==` (spec 6.5): the minimums of
    // Int and Int64, hex and binary, and `-0.0` matching `0.0`.
    let project = Project::new(
        "negative-patterns",
        &[
            ("Pome.toml", ""),
            (
                "src/main.emel",
                r#"fn int(n: Int) -> Int {
  n match {
    -2147483648 -> 1
    -0x10 -> 2
    -0b11 -> 3
    -1 -> 4
    _ -> 0
  }
}

fn long(n: Int64) -> Int {
  n match {
    -9223372036854775808 -> 1
    -5 -> 2
    _ -> 0
  }
}

fn float(x: Float) -> Int {
  x match {
    -1.5 -> 1
    -0.0 -> 2
    _ -> 0
  }
}

fn main() {
  dbg([int(-2147483647 - 1), int(-16), int(-3), int(-1), int(1)])
  dbg([long(-9223372036854775807 - 1), long(-5), long(5)])
  dbg([float(-1.5), float(0.0), float(-0.0), float(1.5)])
}
"#,
            ),
        ],
    );
    let run = project.emela(&["run"]);
    assert_eq!(
        (run.code, run.stdout.as_str(), run.stderr.as_str()),
        (Some(0), "", "[1, 2, 3, 4, 0]\n[1, 2, 0]\n[1, 2, 2, 0]\n")
    );
}

#[test]
fn run_hello_example() {
    // The README example (`examples/hello`). Console is provided by JsConsole (spec 8.3, 15.2).
    let project = Project::new(
        "hello",
        &[
            ("Pome.toml", ""),
            (
                "src/main.emel",
                include_str!("../../../examples/hello/src/main.emel"),
            ),
        ],
    );
    let run = project.emela(&["run"]);
    assert_eq!(
        (run.code, run.stdout.as_str(), run.stderr.as_str()),
        (Some(0), "hi\nHello, Emela!\nHello, world!\n", "done\n")
    );
}

#[test]
fn run_and_test_errors_example() {
    // The README example (`examples/errors`). An error not caught by `escape` reaches `main` and
    // exits with 1 (spec 14.3); tests swap the handler (spec chapter 13).
    let project = Project::new(
        "errors",
        &[
            ("Pome.toml", ""),
            (
                "src/main.emel",
                include_str!("../../../examples/errors/src/main.emel"),
            ),
        ],
    );
    let run = project.emela(&["run"]);
    assert_eq!(
        (run.code, run.stdout.as_str(), run.stderr.as_str()),
        (
            Some(1),
            "Hello, alice!\nHello, guest!\n",
            "error: Invalid(reason: \"negative id\")\n"
        )
    );
    assert_eq!(project.emela(&["test"]).code, Some(0));
}

#[test]
fn run_and_test_clock_example() {
    // The README example (`examples/clock`). JsClock resumes later; the test's InstantClock resumes
    // immediately (spec 8.4, 8.7).
    let project = Project::new(
        "clock",
        &[
            ("Pome.toml", ""),
            (
                "src/main.emel",
                include_str!("../../../examples/clock/src/main.emel"),
            ),
        ],
    );
    let run = project.emela(&["run"]);
    assert_eq!(
        (run.code, run.stdout.as_str(), run.stderr.as_str()),
        (Some(0), "3\n2\n1\nliftoff\n", "")
    );
    assert_eq!(project.emela(&["test"]).code, Some(0));
}

#[test]
fn run_and_test_fibers_example() {
    // The `examples/fibers` example. The children of a scope wait concurrently; the combinators
    // keep the order of their arguments, and a failing child interrupts its siblings (spec 11.2,
    // 11.3).
    let project = Project::new(
        "fibers",
        &[
            ("Pome.toml", ""),
            (
                "src/main.emel",
                include_str!("../../../examples/fibers/src/main.emel"),
            ),
        ],
    );
    let run = project.emela(&["run"]);
    assert_eq!(
        (run.code, run.stdout.as_str(), run.stderr.as_str()),
        (
            Some(0),
            "fetched 2\nfetched 1\nboth: user 1, user 2\nfetched 5\nfetched 4\nfetched 3\n\
             map: [\"user 3\", \"user 4\", \"user 5\"]\nfetched 7\nrace: user 7\n\
             timeout: None\nfetched -1\nnot found: -1\n",
            ""
        )
    );
    assert_eq!(project.emela(&["test"]).code, Some(0));
}

#[test]
fn run_and_test_notes_example() {
    // The `examples/notes` example. SystemFileSystem (in Platform) writes under the project
    // directory; the tests use TestFileSystem (spec 15.2).
    let project = Project::new(
        "notes",
        &[
            ("Pome.toml", ""),
            (
                "src/main.emel",
                include_str!("../../../examples/notes/src/main.emel"),
            ),
        ],
    );
    let run = project.emela(&["run"]);
    assert_eq!(
        (run.code, run.stdout.as_str(), run.stderr.as_str()),
        (
            Some(0),
            "no notes in notes/monday.txt\n1 note(s)\n[\"monday.txt\"]\n",
            ""
        )
    );
    let run = project.emela(&["run"]);
    assert_eq!(run.stdout, "buy milk\n2 note(s)\n[\"monday.txt\"]\n");
    assert_eq!(project.emela(&["test"]).code, Some(0));
}

#[test]
fn run_and_test_http_example() {
    // The `examples/http` example. TestHttp answers with a function, so neither `run` nor `test`
    // touches the network; a 404 is a response and a missing answer is ConnectionFailed (spec 15.2).
    let project = Project::new(
        "http",
        &[
            ("Pome.toml", ""),
            (
                "src/main.emel",
                include_str!("../../../examples/http/src/main.emel"),
            ),
        ],
    );
    let run = project.emela(&["run"]);
    assert_eq!(
        (run.code, run.stdout.as_str(), run.stderr.as_str()),
        (
            Some(0),
            "Emela: typed errors and effects\nstatus 404\n\
             unreachable: TestServer returned no response\n\
             sent https://search.example.com/search?q=emela&page=1\n\
             sent https://search.example.com/search?q=%E6%9D%B1%E4%BA%AC&page=1\n\
             sent https://search.example.com/search?q=again&page=1\n",
            ""
        )
    );
    assert_eq!(project.emela(&["test"]).code, Some(0));
}

#[test]
fn run_and_test_resources_example() {
    // The `examples/resources` example. A handler with state opens in `init` and closes in
    // `release`; the layer is ordered by dependencies, and a later handler replaces the layer's
    // one for the same effect (spec 8.3, 8.4, 8.5, 14.2).
    let project = Project::new(
        "resources",
        &[
            ("Pome.toml", ""),
            (
                "src/main.emel",
                include_str!("../../../examples/resources/src/main.emel"),
            ),
        ],
    );
    let run = project.emela(&["run"]);
    assert_eq!(
        (run.code, run.stdout.as_str(), run.stderr.as_str()),
        (
            Some(0),
            "open store v1\nv1/alice\nv1/bob\nclose store v1\n",
            ""
        )
    );
    assert_eq!(project.emela(&["test"]).code, Some(0));
}

#[test]
fn run_calls_external_module_next_to_the_source() {
    // An `@external` pointing to a file resolves next to the declaring source (spec 12.1). A thrown
    // exception is a defect, and a lone surrogate in a received string becomes U+FFFD (spec 12.5).
    let project = Project::new(
        "ffi",
        &[
            ("Pome.toml", ""),
            (
                "src/host/ffi.mjs",
                "export function lone() { return \"a\\uD800\"; }\nexport function boom() { throw new TypeError(\"bad\"); }\n",
            ),
            (
                "src/host/host.emel",
                "pub effect Host {\n  fn lone() -> String\n  fn boom() -> ()\n}\n\n@external(js, \"./ffi.mjs\", \"lone\")\nfn js_lone() -> String\n\n@external(js, \"./ffi.mjs\", \"boom\")\nfn js_boom() -> ()\n\npub handler NodeHost: Host {\n  fn lone() { js_lone() }\n  fn boom() { js_boom() }\n}\n",
            ),
            (
                "src/main.emel",
                "import Host.Host.{Host, NodeHost}\n\nfn main() {\n  with NodeHost {\n    h = use Host\n    dbg(String.code_points(h.lone()))\n    h.boom()\n  }\n}\n",
            ),
        ],
    );
    let run = project.emela(&["run"]);
    assert_eq!(
        (run.code, run.stdout.as_str(), run.stderr.as_str()),
        (Some(101), "", "[97, 65533]\ndefect: TypeError: bad\n")
    );
}

/// A const is evaluated on its first reference only, and a defect while evaluating it names the
/// const and is remembered: the tests share the values, and an unused const with a defect does not
/// get in the way (spec 5.6).
#[test]
fn constants_are_evaluated_once() {
    let project = Project::new(
        "const-once",
        &[
            ("Pome.toml", ""),
            (
                "src/main.emel",
                r#"const ANSWER = traced("ANSWER", 21)
const BROKEN = traced("BROKEN", 1) / zero()
const UNUSED = 1 / zero()

fn traced(name: String, n: Int) -> Int {
  dbg("evaluating #{name}")
  n
}

fn zero() -> Int { 0 }

@test
fn first() {
  assert ANSWER + ANSWER == 42
  assert BROKEN == 0
}

@test
fn second() {
  assert ANSWER == 21
  assert BROKEN == 0
}

fn main() {
  dbg(ANSWER + ANSWER)
  dbg(ANSWER)
}
"#,
            ),
        ],
    );
    let run = project.emela(&["run"]);
    assert_eq!(
        (run.code, run.stdout.as_str(), run.stderr.as_str()),
        (Some(0), "", "\"evaluating ANSWER\"\n42\n21\n")
    );
    let run = project.emela(&["test"]);
    assert_eq!(run.code, Some(1), "{run:?}");
    assert_eq!(
        run.stdout
            .matches("defect: while evaluating const BROKEN: division by zero")
            .count(),
        2,
        "{run:?}"
    );
    let traces = format!("{}{}", run.stdout, run.stderr);
    assert_eq!(traces.matches("evaluating ANSWER").count(), 1, "{run:?}");
    assert_eq!(
        traces.matches("\"evaluating BROKEN\"").count(),
        1,
        "{run:?}"
    );
}

/// Referring to a const while it is being evaluated is a defect, and consts that refer to each
/// other by name only are an error at compile time (E0231, spec 5.6).
#[test]
fn const_cycles() {
    let project = Project::new(
        "const-self",
        &[
            ("Pome.toml", ""),
            (
                "src/main.emel",
                "const A = f()\n\nfn f() -> Int { A + 1 }\n\nfn main() {\n  dbg(A)\n}\n",
            ),
        ],
    );
    let run = project.emela(&["run"]);
    assert_eq!(
        (run.code, run.stdout.as_str(), run.stderr.as_str()),
        (
            Some(101),
            "",
            "defect: const A refers to itself while being evaluated\n"
        )
    );

    let project = Project::new(
        "const-cycle",
        &[
            ("Pome.toml", ""),
            (
                "src/main.emel",
                "const A = B\nconst B = A + 1\n\nfn main() {\n  dbg(A)\n}\n",
            ),
        ],
    );
    let run = project.emela(&["check"]);
    assert_eq!(run.code, Some(1));
    assert!(
        run.stderr
            .contains("error[E0231]: constants refer to each other: `A` -> `B` -> `A`"),
        "{run:?}"
    );
}
