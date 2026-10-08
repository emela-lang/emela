//! `emela` をプロセスとして起動する結合テスト．

use std::path::{Path, PathBuf};
use std::process::Command;

/// テストごとのプロジェクト．作るときに前の残りを消し，落とすときに消す．
struct Project {
    dir: PathBuf,
}

impl Project {
    fn new(name: &str, files: &[(&str, &str)]) -> Self {
        let dir = std::env::temp_dir().join(format!("emela-cli-{}-{name}", std::process::id()));
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
        let output = Command::new(env!("CARGO_BIN_EXE_emela"))
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

#[test]
fn check_reports_all_errors() {
    let project = Project::new("broken", BROKEN);
    let run = project.emela(&["check"]);
    assert_eq!(run.code, Some(1));
    assert_eq!(run.stdout, "");
    insta::assert_snapshot!(run.stderr);
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
    // プロジェクトの外からディレクトリを渡しても同じ．
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
fn build_and_run_without_js_backend() {
    let project = Project::new(
        "no-backend",
        &[("Pome.toml", ""), ("src/main.emel", "fn main() { 1 }\n")],
    );
    for command in ["build", "run"] {
        let run = project.emela(&[command]);
        assert_eq!(run.code, Some(1), "{command}");
        assert_eq!(
            run.stderr, "error[E0905]: JS output is not implemented yet\n\n1 error\n",
            "{command}"
        );
    }
}

#[test]
fn build_reports_missing_entry() {
    let project = Project::new("no-entry", &[("Pome.toml", ""), ("src/util.emel", "")]);
    let run = project.emela(&["build"]);
    assert_eq!(run.code, Some(1));
    assert_eq!(
        run.stderr,
        "error[E0209]: the entry `src/main.emel` does not exist\n\n1 error\n"
    );
    // check はエントリを求めない．
    assert_eq!(project.emela(&["check"]).code, Some(0));
}

#[test]
fn check_with_absolute_path_from_elsewhere() {
    let project = Project::new("absolute", BROKEN);
    let inside = project.emela(&["check"]);
    // 別のディレクトリから絶対パスで渡しても，診断のパスはプロジェクトからの相対パス．
    let entry = project.path().join("src/main.emel");
    let run = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("check")
        .arg(&entry)
        .current_dir(std::env::temp_dir())
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(run.stderr).unwrap(), inside.stderr);
    // プロジェクトの中のディレクトリからでも同じ．
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
            .starts_with("error[E0208]: `scripts/x.emel` is outside the source root `"),
        "{}",
        run.stderr
    );
}

#[test]
fn single_file_reads_only_imported_files() {
    let project = Project::new(
        "single",
        &[
            ("main.emel", "import Util\n\nfn main() { 1 }\n"),
            ("util.emel", "const Y = $\n"),
            // エントリからたどれないものは読まない．
            ("scratch.emel", "fn broken( {\n"),
            ("Bad.emel", ""),
            ("deep/broken.emel", "const Z = $\n"),
            ("Not-A-Dir/x.emel", ""),
        ],
    );
    let run = project.emela(&["check", "main.emel"]);
    assert_eq!(run.code, Some(1));
    insta::assert_snapshot!(run.stderr);
    // 誤りのあるファイルを import しなければ通る．
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

/// `emela lsp` を起動し，ワークスペースの診断が届いてから shutdown と exit で 0 で終わる．
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
