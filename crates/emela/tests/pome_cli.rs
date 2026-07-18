//! End-to-end tests for the Pome packaging CLI (spec 0032).
//!
//! Resolution reads versions straight from the repositories that source paths
//! name (R4). To exercise that offline, each test stands up throwaway local Git
//! repositories as upstream Pomes and points the resolver at them through
//! `EMELA_POME_REPLACE` — the same source-path → URL override a company mirror
//! or offline build would use. The cache is redirected with `EMELA_POME_CACHE`
//! so tests never touch a developer's real cache.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

/// A fresh, unique scratch directory for one test.
fn scratch(tag: &str) -> PathBuf {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-pome-{}-{tag}-{id}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Whether `git` is on PATH. When it is missing the tests skip rather than fail,
/// since packaging inherently needs Git (there is no other fetch path).
fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Runs a git command in `dir` with a fixed identity, so the tests do not depend
/// on the machine's git config. Tags are annotated (`-a -m`) for the same
/// reason.
fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Creates an upstream Pome as a local Git repo under `root/<leaf>`, with a
/// `Pome.toml`, one module source file, and one annotated version tag. `deps` is
/// rendered into a `[dependencies]` table. Returns `(source_path, repo_dir)`.
fn upstream(
    root: &Path,
    source: &str,
    version: &str,
    deps: &[(&str, &str)],
    module: &str,
    module_body: &str,
) -> (String, PathBuf) {
    let leaf = source.rsplit('/').next().unwrap();
    let dir = root.join("upstream").join(leaf);
    fs::create_dir_all(dir.join("src")).unwrap();

    let mut manifest = format!(
        "[pome]\nname = \"{source}\"\nversion = \"{}\"\nemela = \"0.1\"\n",
        version.trim_start_matches('v')
    );
    if !deps.is_empty() {
        manifest.push_str("\n[dependencies]\n");
        for (src, req) in deps {
            manifest.push_str(&format!("\"{src}\" = \"{req}\"\n"));
        }
    }
    fs::write(dir.join("Pome.toml"), manifest).unwrap();
    fs::write(dir.join("src").join(format!("{module}.emel")), module_body).unwrap();

    git(&dir, &["init", "-q"]);
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-q", "-m", "release"]);
    git(&dir, &["tag", "-a", version, "-m", version]);
    (source.to_string(), dir)
}

/// Invokes the `emela` binary in `project` with the packaging env pointed at the
/// test's upstreams and cache.
fn emela(project: &Path, replace: &str, cache: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_emela"))
        .current_dir(project)
        .env("EMELA_POME_REPLACE", replace)
        .env("EMELA_POME_CACHE", cache)
        .args(args)
        .output()
        .unwrap()
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn assert_ok(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "command failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn new_scaffolds_an_entry_pome() {
    let root = scratch("new");
    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .current_dir(&root)
        .arg("new")
        .arg("hello")
        .output()
        .unwrap();
    assert_ok(&output);

    let manifest = fs::read_to_string(root.join("hello").join("Pome.toml")).unwrap();
    assert!(manifest.contains("name = \"hello\""), "{manifest}");
    assert!(manifest.contains("version = \"0.1.0\""), "{manifest}");
    let main = fs::read_to_string(root.join("hello").join("src").join("main.emel")).unwrap();
    assert!(main.contains("fn main()"), "{main}");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn add_resolves_pins_and_lists() {
    if !git_available() {
        eprintln!("skipping: git not available");
        return;
    }
    let root = scratch("add");
    let (stdlib, stdlib_dir) = upstream(
        &root,
        "github.com/emela-lang/stdlib",
        "v1.4.0",
        &[],
        "io",
        "module io\n\neffect Io {\n  extern fn write_stdout(s: String) -> Unit\n}\n\npub fn log(s: String) -> Unit uses { Io } {\n  ()\n}\n",
    );
    // A second, older tag proves the resolver selects the greatest match (V3).
    git(&stdlib_dir, &["tag", "-a", "v1.2.0", "-m", "v1.2.0"]);
    let replace = format!("{stdlib}={}", stdlib_dir.display());
    let cache = root.join("cache");

    let project = root.join("app");
    assert_ok(&emela(&root, &replace, &cache, &["new", "app"]));

    let add = emela(
        &project,
        &replace,
        &cache,
        &["pome", "add", "github:emela-lang/stdlib"],
    );
    assert_ok(&add);
    let out = stdout(&add);
    assert!(out.contains("Resolved v1.4.0"), "{out}");
    // Capability audit computed from source (CAP1).
    assert!(out.contains("Io"), "{out}");

    // Pome.toml records the canonical source path (S3), not the shorthand.
    let manifest = fs::read_to_string(project.join("Pome.toml")).unwrap();
    assert!(
        manifest.contains("\"github.com/emela-lang/stdlib\""),
        "{manifest}"
    );

    // Pome.lock pins the tag, a commit, and a content hash (F6).
    let lock = fs::read_to_string(project.join("Pome.lock")).unwrap();
    assert!(lock.contains("version = \"v1.4.0\""), "{lock}");
    assert!(lock.contains("commit = "), "{lock}");
    assert!(lock.contains("hash = \"tree:"), "{lock}");

    let list = emela(&project, &replace, &cache, &["pome", "list"]);
    assert_ok(&list);
    let tree = stdout(&list);
    assert!(tree.contains("app 0.1.0"), "{tree}");
    assert!(
        tree.contains("github.com/emela-lang/stdlib v1.4.0"),
        "{tree}"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn add_audits_transitive_capabilities() {
    if !git_available() {
        eprintln!("skipping: git not available");
        return;
    }
    let root = scratch("cap");
    let (util, util_dir) = upstream(
        &root,
        "gitlab.com/acme/util",
        "v0.3.1",
        &[],
        "util",
        "module util\n\npub fn tick() -> Int uses { Clock } {\n  0\n}\n",
    );
    let (http, http_dir) = upstream(
        &root,
        "github.com/acme/http",
        "v0.4.0",
        &[("gitlab.com/acme/util", "^0.3")],
        "http",
        "module http\n\npub fn get(u: String) -> String uses { Net } {\n  u\n}\n",
    );
    let replace = format!(
        "{http}={};{util}={}",
        http_dir.display(),
        util_dir.display()
    );
    let cache = root.join("cache");

    let project = root.join("app");
    assert_ok(&emela(&root, &replace, &cache, &["new", "app"]));
    let add = emela(
        &project,
        &replace,
        &cache,
        &["pome", "add", "github:acme/http"],
    );
    assert_ok(&add);
    let out = stdout(&add);
    // The union of the added Pome's and its transitive dependency's effects,
    // computed from source (CAP1/CAP2).
    assert!(out.contains("Net"), "{out}");
    assert!(out.contains("Clock"), "{out}");

    // The lock records the transitive dependency and the edge to it.
    let lock = fs::read_to_string(project.join("Pome.lock")).unwrap();
    assert!(lock.contains("github.com/acme/http"), "{lock}");
    assert!(lock.contains("gitlab.com/acme/util"), "{lock}");
    assert!(
        lock.contains("dependencies = [\"gitlab.com/acme/util\"]"),
        "{lock}"
    );

    // `list` renders util nested under http.
    let list = stdout(&emela(&project, &replace, &cache, &["pome", "list"]));
    let http_at = list.find("github.com/acme/http").unwrap();
    let util_at = list.find("gitlab.com/acme/util").unwrap();
    assert!(
        http_at < util_at,
        "util should be nested under http:\n{list}"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn remove_prunes_manifest_and_lock() {
    if !git_available() {
        eprintln!("skipping: git not available");
        return;
    }
    let root = scratch("remove");
    let (stdlib, stdlib_dir) = upstream(
        &root,
        "github.com/emela-lang/stdlib",
        "v1.0.0",
        &[],
        "io",
        "module io\n\npub fn log(s: String) -> Unit {\n  s\n}\n",
    );
    let replace = format!("{stdlib}={}", stdlib_dir.display());
    let cache = root.join("cache");
    let project = root.join("app");
    assert_ok(&emela(&root, &replace, &cache, &["new", "app"]));
    assert_ok(&emela(
        &project,
        &replace,
        &cache,
        &["pome", "add", "github:emela-lang/stdlib"],
    ));

    let remove = emela(
        &project,
        &replace,
        &cache,
        &["pome", "remove", "github:emela-lang/stdlib"],
    );
    assert_ok(&remove);

    let manifest = fs::read_to_string(project.join("Pome.toml")).unwrap();
    assert!(
        !manifest.contains("stdlib"),
        "dependency not pruned: {manifest}"
    );
    // The last dependency gone, the lock is removed rather than left empty (C3).
    assert!(
        !project.join("Pome.lock").exists(),
        "Pome.lock should be gone once no dependencies remain"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn install_materializes_from_lock() {
    if !git_available() {
        eprintln!("skipping: git not available");
        return;
    }
    let root = scratch("install");
    let (stdlib, stdlib_dir) = upstream(
        &root,
        "github.com/emela-lang/stdlib",
        "v1.0.0",
        &[],
        "io",
        "module io\n\npub fn log(s: String) -> Unit {\n  s\n}\n",
    );
    let replace = format!("{stdlib}={}", stdlib_dir.display());
    let cache = root.join("cache");
    let project = root.join("app");
    assert_ok(&emela(&root, &replace, &cache, &["new", "app"]));
    assert_ok(&emela(
        &project,
        &replace,
        &cache,
        &["pome", "add", "github:emela-lang/stdlib"],
    ));

    // Wipe the cache and install straight from the lock (F5).
    let _ = fs::remove_dir_all(&cache);
    let install = emela(&project, &replace, &cache, &["pome", "install"]);
    assert_ok(&install);
    assert!(
        stdout(&install).contains("Installed 1 package"),
        "{}",
        stdout(&install)
    );
    assert!(
        cache
            .join("github.com/emela-lang/stdlib/v1.0.0/.git")
            .exists(),
        "install should have re-fetched into the cache"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn search_reports_when_no_orchard() {
    let root = scratch("search");
    let project = root.join("app");
    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .current_dir(&root)
        .arg("new")
        .arg("app")
        .output()
        .unwrap();
    assert_ok(&output);
    let search = Command::new(env!("CARGO_BIN_EXE_emela"))
        .current_dir(&project)
        .env_remove("EMELA_ORCHARD_URL")
        .args(["pome", "search", "json"])
        .output()
        .unwrap();
    assert_ok(&search);
    assert!(
        String::from_utf8_lossy(&search.stdout).contains("No Orchard"),
        "{}",
        String::from_utf8_lossy(&search.stdout)
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn pome_command_outside_a_project_errors() {
    let root = scratch("noproject");
    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .current_dir(&root)
        .args(["pome", "list"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Pome.toml"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn build_resolves_imports_from_a_dependency_pome() {
    if !git_available() {
        eprintln!("skipping: git not available");
        return;
    }
    let root = scratch("build-import");
    // A dependency Pome exposing `module math` with a `pub` function. Its import
    // root is the source-path leaf `mathlib` (spec 0032 M1/M2).
    let (mathlib, mathlib_dir) = upstream(
        &root,
        "github.com/acme/mathlib",
        "v1.0.0",
        &[],
        "math",
        "module math\n\npub fn add_one(x: Int) -> Int {\n  x + 1\n}\n",
    );
    let replace = format!("{mathlib}={}", mathlib_dir.display());
    let cache = root.join("cache");
    let project = root.join("app");
    assert_ok(&emela(&root, &replace, &cache, &["new", "app"]));
    assert_ok(&emela(
        &project,
        &replace,
        &cache,
        &["pome", "add", "github:acme/mathlib"],
    ));

    // Import the dependency's module (module-unit, spec 0037) and call by the
    // leaf and fully-qualified names.
    fs::write(
        project.join("src").join("main.emel"),
        "import mathlib.math\n\n\
         fn main() -> Int {\n  math.add_one(1) + mathlib.math.add_one(40)\n}\n",
    )
    .unwrap();

    let build = emela(
        &project,
        &replace,
        &cache,
        &["build", "--backend", "js-node", "src/main.emel"],
    );
    assert_ok(&build);
    assert!(
        stdout(&build).contains("function main()"),
        "the dependency import should compile:\n{}",
        stdout(&build)
    );
    let _ = fs::remove_dir_all(&root);
}

/// A dependency Pome whose manifest claims import root `std` (`module =
/// "std"`) may not provide a module reserved by the embedded core (spec
/// 0038); the same eager check as for `--package` fires at build time.
#[test]
fn build_rejects_a_std_pome_shadowing_an_embedded_module() {
    if !git_available() {
        eprintln!("skipping: git not available");
        return;
    }
    let root = scratch("std-reserved");
    let source = "github.com/acme/stdfork";
    let dir = root.join("upstream").join("stdfork");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("Pome.toml"),
        format!(
            "[pome]\nname = \"{source}\"\nversion = \"1.0.0\"\nemela = \"0.1\"\nmodule = \"std\"\n"
        ),
    )
    .unwrap();
    fs::write(
        dir.join("src").join("io.emel"),
        "module io\n\npub fn shadow(s: String) -> String {\n  s\n}\n",
    )
    .unwrap();
    git(&dir, &["init", "-q"]);
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-q", "-m", "release"]);
    git(&dir, &["tag", "-a", "v1.0.0", "-m", "v1.0.0"]);

    let replace = format!("{source}={}", dir.display());
    let cache = root.join("cache");
    let project = root.join("app");
    assert_ok(&emela(&root, &replace, &cache, &["new", "app"]));
    assert_ok(&emela(
        &project,
        &replace,
        &cache,
        &["pome", "add", "github:acme/stdfork"],
    ));

    let build = emela(
        &project,
        &replace,
        &cache,
        &["build", "--backend", "js-node", "src/main.emel"],
    );
    assert!(!build.status.success(), "expected the build to fail");
    let err = String::from_utf8_lossy(&build.stderr);
    assert!(
        err.contains("embedded in the compiler") && err.contains("io.emel"),
        "unexpected diagnostic:\n{err}"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn build_errors_when_a_locked_dependency_is_not_fetched() {
    if !git_available() {
        eprintln!("skipping: git not available");
        return;
    }
    let root = scratch("build-missing");
    let (mathlib, mathlib_dir) = upstream(
        &root,
        "github.com/acme/mathlib",
        "v1.0.0",
        &[],
        "math",
        "module math\n\npub fn add_one(x: Int) -> Int {\n  x + 1\n}\n",
    );
    let replace = format!("{mathlib}={}", mathlib_dir.display());
    let cache = root.join("cache");
    let project = root.join("app");
    assert_ok(&emela(&root, &replace, &cache, &["new", "app"]));
    assert_ok(&emela(
        &project,
        &replace,
        &cache,
        &["pome", "add", "github:acme/mathlib"],
    ));
    fs::write(
        project.join("src").join("main.emel"),
        "import mathlib.math.add_one\n\nfn main() -> Int {\n  add_one(41)\n}\n",
    )
    .unwrap();

    // Drop the cache: the lock still pins the dependency, but it is not present.
    let _ = fs::remove_dir_all(&cache);
    let build = emela(
        &project,
        &replace,
        &cache,
        &["check", "--backend", "js-node", "src/main.emel"],
    );
    assert!(!build.status.success());
    assert!(
        String::from_utf8_lossy(&build.stderr).contains("emela pome install"),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let _ = fs::remove_dir_all(&root);
}
