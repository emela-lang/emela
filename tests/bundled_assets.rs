use std::fs;
use std::process::Command;

#[test]
fn compiler_binary_rejects_std_import_without_std_package() {
    let temp = std::env::temp_dir().join(format!("emela-missing-std-test-{}", std::process::id()));
    fs::create_dir_all(&temp).unwrap();
    let source = temp.join("main.emel");
    fs::write(
        &source,
        r##"
import std.io.write_stdout_utf8!

fn main!() -> Result<Unit, PlatformError> {
  write_stdout_utf8!("hello")
}
"##,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .current_dir(&temp)
        .arg("check")
        .arg("--backend")
        .arg("js-node")
        .arg(&source)
        .output()
        .unwrap();

    let _ = fs::remove_file(&source);
    let _ = fs::remove_dir(&temp);
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("package `std` is not available"));
}

#[test]
fn compiler_binary_imports_source_package() {
    let temp = std::env::temp_dir().join(format!("emela-package-test-{}", std::process::id()));
    let package = temp.join("math");
    fs::create_dir_all(package.join("src")).unwrap();
    fs::write(
        package.join("emela-package.json"),
        r#"{"name":"math","source":"src"}"#,
    )
    .unwrap();
    fs::write(
        package.join("src/ops.emel"),
        r#"
fn add_one(value: I32) -> I32 {
  value + 1
}
"#,
    )
    .unwrap();
    let source = temp.join("main.emel");
    fs::write(
        &source,
        r#"
import math.ops.add_one

fn main() -> I32 {
  add_one(41)
}
"#,
    )
    .unwrap();

    let status = Command::new(env!("CARGO_BIN_EXE_emela"))
        .current_dir(&temp)
        .arg("check")
        .arg("--backend")
        .arg("js-node")
        .arg("--package")
        .arg(&package)
        .arg(&source)
        .status()
        .unwrap();

    let _ = fs::remove_file(&source);
    let _ = fs::remove_dir_all(&temp);
    assert!(status.success());
}

#[test]
fn compiler_binary_can_use_std_as_external_package() {
    let temp = std::env::temp_dir().join(format!("emela-std-package-test-{}", std::process::id()));
    fs::create_dir_all(&temp).unwrap();
    let source = temp.join("main.emel");
    fs::write(
        &source,
        r##"
import std.io.write_stdout_utf8!

fn main!() -> Result<Unit, PlatformError> {
  write_stdout_utf8!("hello")
}
"##,
    )
    .unwrap();
    let stdlib = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../stdlib");

    let status = Command::new(env!("CARGO_BIN_EXE_emela"))
        .current_dir(&temp)
        .arg("check")
        .arg("--backend")
        .arg("js-node")
        .arg("--package")
        .arg(stdlib)
        .arg(&source)
        .status()
        .unwrap();

    let _ = fs::remove_file(&source);
    let _ = fs::remove_dir(&temp);
    assert!(status.success());
}

#[test]
fn package_fetch_caches_local_git_dependency_and_check_imports_it() {
    let temp = std::env::temp_dir().join(format!("emela-git-package-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp);
    let repo = temp.join("repo");
    let project = temp.join("project");
    let emela_home = temp.join("home");
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::create_dir_all(&project).unwrap();
    fs::write(
        repo.join("emela-package.json"),
        r#"{"name":"math","version":"0.1.0","source":"src"}"#,
    )
    .unwrap();
    fs::write(
        repo.join("src/ops.emel"),
        r#"
fn add_one(value: I32) -> I32 {
  value + 1
}

#[test]
fn package_add_updates_manifest_fetches_dependency_and_check_imports_it() {
    let temp = std::env::temp_dir().join(format!("emela-package-add-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp);
    let repo = temp.join("repo");
    let project = temp.join("project");
    let emela_home = temp.join("home");
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::create_dir_all(&project).unwrap();
    fs::write(
        repo.join("emela-package.json"),
        r#"{"name":"math","version":"0.1.0","source":"src"}"#,
    )
    .unwrap();
    fs::write(
        repo.join("src/ops.emel"),
        r#"
fn add_one(value: I32) -> I32 {
  value + 1
}
"#,
    )
    .unwrap();
    run_git(&repo, &["init"]);
    run_git(&repo, &["add", "."]);
    run_git(
        &repo,
        &[
            "-c",
            "user.name=Emela Test",
            "-c",
            "user.email=emela@example.test",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            "initial",
        ],
    );
    let rev = git_stdout(&repo, &["rev-parse", "HEAD"]);

    fs::write(
        project.join("emela.json"),
        format!(
            r#"{{
  "package": {{"name":"app","version":"0.1.0"}},
  "dependencies": {{"math": {{"git":"{}","rev":"{}"}}}}
}}"#,
            repo.display(),
            rev.trim()
        ),
    )
    .unwrap();
    let source = project.join("main.emel");
    fs::write(
        &source,
        r#"
import math.ops.add_one

fn main() -> I32 {
  add_one(41)
}
"#,
    )
    .unwrap();

    let fetch = Command::new(env!("CARGO_BIN_EXE_emela"))
        .current_dir(&project)
        .env("EMELA_HOME", &emela_home)
        .arg("package")
        .arg("fetch")
        .status()
        .unwrap();
    assert!(fetch.success());

    let check = Command::new(env!("CARGO_BIN_EXE_emela"))
        .current_dir(&project)
        .env("EMELA_HOME", &emela_home)
        .arg("check")
        .arg("--backend")
        .arg("js-node")
        .arg(&source)
        .status()
        .unwrap();

    let _ = fs::remove_dir_all(&temp);
    assert!(check.success());
}

#[test]
fn package_add_updates_manifest_fetches_dependency_and_check_imports_it() {
    let temp = std::env::temp_dir().join(format!("emela-package-add-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp);
    let repo = temp.join("repo");
    let project = temp.join("project");
    let emela_home = temp.join("home");
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::create_dir_all(&project).unwrap();
    fs::write(
        repo.join("emela-package.json"),
        r#"{"name":"math","version":"0.1.0","source":"src"}"#,
    )
    .unwrap();
    fs::write(
        repo.join("src/ops.emel"),
        r#"
fn add_one(value: I32) -> I32 {
  value + 1
}
"#,
    )
    .unwrap();
    run_git(&repo, &["init"]);
    run_git(&repo, &["add", "."]);
    run_git(
        &repo,
        &[
            "-c",
            "user.name=Emela Test",
            "-c",
            "user.email=emela@example.test",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            "initial",
        ],
    );
    let rev = git_stdout(&repo, &["rev-parse", "HEAD"]);
    fs::write(
        project.join("emela.json"),
        r#"{"package":{"name":"app","version":"0.1.0"},"dependencies":{}}"#,
    )
    .unwrap();
    let source = project.join("main.emel");
    fs::write(
        &source,
        r#"
import math.ops.add_one

fn main() -> I32 {
  add_one(41)
}
"#,
    )
    .unwrap();

    let add = Command::new(env!("CARGO_BIN_EXE_emela"))
        .current_dir(&project)
        .env("EMELA_HOME", &emela_home)
        .arg("package")
        .arg("add")
        .arg("math")
        .arg("--git")
        .arg(&repo)
        .arg("--rev")
        .arg(rev.trim())
        .status()
        .unwrap();
    assert!(add.success());
    let manifest = fs::read_to_string(project.join("emela.json")).unwrap();
    assert!(manifest.contains("\"math\""));

    let check = Command::new(env!("CARGO_BIN_EXE_emela"))
        .current_dir(&project)
        .env("EMELA_HOME", &emela_home)
        .arg("check")
        .arg("--backend")
        .arg("js-node")
        .arg(&source)
        .status()
        .unwrap();

    let _ = fs::remove_dir_all(&temp);
    assert!(check.success());
}

#[test]
fn package_add_rejects_duplicate_dependency() {
    let temp = std::env::temp_dir().join(format!(
        "emela-package-add-duplicate-test-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&temp);
    let project = temp.join("project");
    fs::create_dir_all(&project).unwrap();
    fs::write(
        project.join("emela.json"),
        r#"{
  "package": {"name":"app","version":"0.1.0"},
  "dependencies": {"math": {"git":"file:///already","rev":"deadbeef"}}
}"#,
    )
    .unwrap();

    let status = Command::new(env!("CARGO_BIN_EXE_emela"))
        .current_dir(&project)
        .arg("package")
        .arg("add")
        .arg("math")
        .arg("--git")
        .arg("file:///other")
        .arg("--rev")
        .arg("cafebabe")
        .status()
        .unwrap();

    let _ = fs::remove_dir_all(&temp);
    assert!(!status.success());
}

#[test]
fn manifest_dependency_colliding_with_package_flag_is_rejected() {
    let temp = std::env::temp_dir().join(format!(
        "emela-duplicate-package-test-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&temp);
    let project = temp.join("project");
    let package = temp.join("math");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(package.join("src")).unwrap();
    fs::write(
        package.join("emela-package.json"),
        r#"{"name":"math","version":"0.1.0","source":"src"}"#,
    )
    .unwrap();
    fs::write(
        project.join("emela.json"),
        r#"{
  "package": {"name":"app","version":"0.1.0"},
  "dependencies": {"math": {"git":"file:///missing","rev":"deadbeef"}}
}"#,
    )
    .unwrap();
    let source = project.join("main.emel");
    fs::write(
        &source,
        r#"
fn main() -> Unit {
}
"#,
    )
    .unwrap();

    let status = Command::new(env!("CARGO_BIN_EXE_emela"))
        .current_dir(&project)
        .arg("check")
        .arg("--backend")
        .arg("js-node")
        .arg("--package")
        .arg(&package)
        .arg(&source)
        .status()
        .unwrap();

    let _ = fs::remove_dir_all(&temp);
    assert!(!status.success());
}

fn run_git(current_dir: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(current_dir)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success());
}

fn git_stdout(current_dir: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(current_dir)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap()
}
