//! End-to-end tests for attributes (spec 0039) and unit testing (spec 0040):
//! the `@test` attribute, the `emela test` runner, and `[dev-dependencies]`.
//! Each test drives the compiled `emela` binary like a user would.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

fn scratch(tag: &str) -> PathBuf {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-test-{tag}-{}-{id}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn emela_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_emela"))
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap()
}

/// Writes `source` to a temp file and runs `emela <command> FILE`.
fn on_source(tag: &str, command: &[&str], source: &str) -> Output {
    let dir = scratch(tag);
    let input = dir.join("main.emel");
    fs::write(&input, source).unwrap();
    let mut args: Vec<&str> = command.to_vec();
    let input_str = input.to_str().unwrap().to_string();
    args.push(&input_str);
    let output = emela_in(&dir, &args);
    let _ = fs::remove_dir_all(&dir);
    output
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

// ---------------------------------------------------------------------------
// Spec 0039: attributes
// ---------------------------------------------------------------------------

// R5: an unknown attribute is a compile error, not a warning.
#[test]
fn unknown_attribute_is_an_error() {
    let output = on_source(
        "attr-unknown",
        &["check"],
        "@deprecated\nfn old() -> Unit uses {} { () }\n\nfn main() -> Int uses {} { 0 }\n",
    );
    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(text.contains("Unknown attribute"), "{text}");
    assert!(text.contains("`@test`"), "{text}");
}

// R3: a duplicated attribute is an error; R7: `@test` takes no argument;
// R6: `@test` applies to `fn` only; R2: attributes precede `pub`. All four
// report in one pass (spec 0033 multi-error collection).
#[test]
fn attribute_shape_violations_all_report() {
    let output = on_source(
        "attr-shapes",
        &["check"],
        concat!(
            "@test\n@test\nfn twice() -> Unit uses {} { () }\n\n",
            "@test(\"named\")\nfn named() -> Unit uses {} { () }\n\n",
            "@test\nenum Color {\n    Red\n}\n\n",
            "pub @test\nfn after_pub() -> Unit uses {} { () }\n\n",
            "fn main() -> Int uses {} { 0 }\n"
        ),
    );
    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(text.contains("Duplicate attribute"), "{text}");
    assert!(text.contains("takes no arguments"), "{text}");
    assert!(text.contains("Attribute does not apply here"), "{text}");
    assert!(text.contains("Attribute after `pub`"), "{text}");
}

// R8: fmt puts each attribute on its own line directly above the declaration.
#[test]
fn fmt_normalizes_attribute_placement() {
    let dir = scratch("attr-fmt");
    let input = dir.join("main.emel");
    fs::write(
        &input,
        "@test fn inline() -> Unit uses {} { () }\n\n@test\n\nfn spaced() -> Unit uses {} { () }\n",
    )
    .unwrap();
    let output = emela_in(&dir, &["fmt", input.to_str().unwrap()]);
    assert!(output.status.success(), "{}", stderr(&output));
    let formatted = fs::read_to_string(&input).unwrap();
    assert_eq!(
        formatted,
        "@test\nfn inline() -> Unit uses {} { () }\n\n@test\nfn spaced() -> Unit uses {} { () }\n"
    );
    // Idempotent (spec 0035 F10).
    let check = emela_in(&dir, &["fmt", "--check", input.to_str().unwrap()]);
    assert!(check.status.success(), "{}", stderr(&check));
    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Spec 0041: the `@lang` attribute (lang-item binding)
// ---------------------------------------------------------------------------

// R7: `@lang("<role>")` takes a string-literal argument and binds a top-level
// `enum` to a recognized role. The bare mechanism accepts a well-formed use.
#[test]
fn lang_attribute_binds_an_enum() {
    let output = on_source(
        "lang-ok",
        &["check"],
        concat!(
            "@lang(\"option\")\nenum Maybe<T> {\n    Just(T)\n    Nothing\n}\n\n",
            "fn main() -> Int uses {} { 0 }\n"
        ),
    );
    assert!(output.status.success(), "{}", stderr(&output));
}

// `@lang` applies to an `enum` only (R6), its role name is closed (spec 0042),
// and it requires an argument (R7). All three report in one pass.
#[test]
fn lang_attribute_violations_all_report() {
    let output = on_source(
        "lang-bad",
        &["check"],
        concat!(
            "@lang(\"option\")\nfn not_an_enum() -> Unit uses {} { () }\n\n",
            "@lang(\"maybe\")\nenum Bad<T> {\n    Just(T)\n    Nothing\n}\n\n",
            "@lang\nenum NoArg<T> {\n    A(T)\n    B\n}\n\n",
            "fn main() -> Int uses {} { 0 }\n"
        ),
    );
    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(
        text.contains("may only be applied to a top-level `enum`"),
        "{text}"
    );
    assert!(text.contains("unknown lang-item role"), "{text}");
    assert!(text.contains("requires a role name"), "{text}");
}

// ---------------------------------------------------------------------------
// Spec 0040: `@test` semantics (T rules)
// ---------------------------------------------------------------------------

// T3: bare throwing calls, bare `throw`, and explicit `try`/`catch` all check
// inside a test body without `?`.
#[test]
fn implicit_try_accepts_bare_throwing_code() {
    let output = on_source(
        "t-implicit-try",
        &["check"],
        concat!(
            "enum E {\n    Bad\n}\n\n",
            "fn risky(flag: Bool) -> Int throws E uses {} {\n",
            "    if flag { throw E::Bad } else { 7 }\n}\n\n",
            "@test\nfn bare_call() -> Unit uses {} {\n    let x = risky(false)\n    ()\n}\n\n",
            "@test\nfn bare_throw() -> Unit uses {} {\n    throw E::Bad\n}\n\n",
            "@test\nfn explicit_try() -> Unit uses {} {\n",
            "    let x = try { risky(true) } catch { Bad -> 0 }\n    ()\n}\n\n",
            "fn main() -> Int uses {} { 0 }\n"
        ),
    );
    assert!(output.status.success(), "{}", stderr(&output));
}

// T3: `?` has nothing to propagate to in a test and is rejected with guidance.
#[test]
fn question_in_test_body_is_redundant() {
    let output = on_source(
        "t-question",
        &["check"],
        concat!(
            "enum E {\n    Bad\n}\n\n",
            "fn risky() -> Int throws E uses {} { throw E::Bad }\n\n",
            "@test\nfn uses_question() -> Unit uses {} {\n    let x = risky()?\n    ()\n}\n\n",
            "fn main() -> Int uses {} { 0 }\n"
        ),
    );
    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(text.contains("Redundant `?` in a test"), "{text}");
}

// T2/T5: the signature rules — no `pub`, no parameters, `Unit` return, no
// `throws` — all report.
#[test]
fn test_signature_violations_all_report() {
    let output = on_source(
        "t-signature",
        &["check"],
        concat!(
            "enum E {\n    Bad\n}\n\n",
            "@test\npub fn visible() -> Unit uses {} { () }\n\n",
            "@test\nfn with_arg(x: Int) -> Unit uses {} { () }\n\n",
            "@test\nfn wrong_ret() -> Int uses {} { 1 }\n\n",
            "@test\nfn declares_throws() -> Unit throws E uses {} { () }\n\n",
            "fn main() -> Int uses {} { 0 }\n"
        ),
    );
    assert!(!output.status.success());
    let text = stderr(&output);
    assert_eq!(text.matches("Invalid test function").count(), 4, "{text}");
}

// T5: no source code can reference a test function, by bare or qualified name.
#[test]
fn test_functions_are_unreferenceable() {
    let output = on_source(
        "t-unreferenceable",
        &["check"],
        concat!(
            "@test\nfn target() -> Unit uses {} { () }\n\n",
            "fn caller() -> Unit uses {} {\n    target()\n}\n\n",
            "fn main() -> Int uses {} { 0 }\n"
        ),
    );
    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(text.contains("Unknown name"), "{text}");
}

// T3: a nested function literal's body follows the ordinary rules.
#[test]
fn lambda_inside_test_follows_normal_rules() {
    let output = on_source(
        "t-lambda",
        &["check"],
        concat!(
            "enum E {\n    Bad\n}\n\n",
            "fn risky() -> Int throws E uses {} { throw E::Bad }\n\n",
            "@test\nfn with_lambda() -> Unit uses {} {\n",
            "    let f = fn () -> Int uses {} { risky() }\n    ()\n}\n\n",
            "fn main() -> Int uses {} { 0 }\n"
        ),
    );
    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(text.contains("Unhandled throwing call"), "{text}");
}

// T8: `ir` (and thus `build`) excludes test functions from the artifact, and
// `run` executes `main` untroubled by tests in the same file.
#[test]
fn tests_are_stripped_from_artifacts() {
    let source = concat!(
        "@test\nfn stripped_from_ir() -> Unit uses {} { () }\n\n",
        "fn main() -> Int uses {} { 42 }\n"
    );
    let ir = on_source("t8-ir", &["ir"], source);
    assert!(ir.status.success(), "{}", stderr(&ir));
    assert!(!stdout(&ir).contains("stripped_from_ir"), "{}", stdout(&ir));

    let run = on_source("t8-run", &["run"], source);
    assert_eq!(run.status.code(), Some(42), "{}", stderr(&run));
}

// ---------------------------------------------------------------------------
// Spec 0040: the runner (C rules)
// ---------------------------------------------------------------------------

/// Lays out a Pome with the given `(relative path, source)` module files.
fn pome_fixture(tag: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = scratch(tag);
    fs::write(
        dir.join("Pome.toml"),
        "[pome]\nname = \"fixture\"\nversion = \"0.1.0\"\nemela = \"0.1\"\n",
    )
    .unwrap();
    for (path, source) in files {
        let file = dir.join("src").join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, source).unwrap();
    }
    dir
}

// C4/C6/C7/C8: passes and failures report in the canonical format, the failure
// detail carries the `Show`-rendered thrown error, and the exit code is 1.
#[test]
fn runner_reports_passes_and_failures() {
    let dir = pome_fixture(
        "runner-mixed",
        &[(
            "rle.emel",
            concat!(
                "module rle\n\n",
                "enum AssertError {\n    Failed(String)\n}\n\n",
                "impl Show for AssertError {\n",
                "    fn to_string(e: AssertError) -> String uses {} {\n",
                "        match e {\n",
                "            AssertError::Failed(message) -> \"assertion failed: \" ++ message\n",
                "        }\n    }\n}\n\n",
                "fn assert_eq_int(actual: Int, expected: Int) -> Unit throws AssertError uses {} {\n",
                "    if actual == expected { () } else {\n",
                "        throw AssertError::Failed(\n",
                "            \"expected `\" ++ expected.to_string() ++ \"`, got `\" ++ actual.to_string() ++ \"`\"\n",
                "        )\n    }\n}\n\n",
                "fn double(x: Int) -> Int uses {} { x + x }\n\n",
                "@test\nfn double_of_two_is_four() -> Unit uses {} {\n",
                "    assert_eq_int(double(2), 4)\n}\n\n",
                "@test\nfn deliberately_fails() -> Unit uses {} {\n",
                "    assert_eq_int(double(2), 5)\n}\n\n",
                "fn main() -> Int uses {} { 0 }\n"
            ),
        )],
    );
    let output = emela_in(&dir, &["test"]);
    assert_eq!(output.status.code(), Some(1));
    let text = stdout(&output);
    assert!(text.contains("running 2 tests"), "{text}");
    assert!(
        text.contains("test rle.double_of_two_is_four ... ok"),
        "{text}"
    );
    assert!(
        text.contains("test rle.deliberately_fails ... FAILED"),
        "{text}"
    );
    assert!(text.contains("---- rle.deliberately_fails ----"), "{text}");
    assert!(
        text.contains("threw AssertError: assertion failed: expected `5`, got `4`"),
        "{text}"
    );
    assert!(
        text.contains("test result: FAILED. 1 passed; 1 failed"),
        "{text}"
    );
    let _ = fs::remove_dir_all(&dir);
}

// C2 (nested modules, `main` not required), C4 (a panic is a failure), C5 (a
// trapped test does not stop the others), C8 (all green exits 0 — checked in
// `runner_all_green_exits_zero`).
#[test]
fn runner_walks_nested_modules_and_survives_traps() {
    let dir = pome_fixture(
        "runner-nested",
        &[
            (
                "util/strings.emel",
                concat!(
                    "module strings\n\n",
                    "@test\nfn genuine_panic_is_a_failure() -> Unit uses {} {\n",
                    "    panic(\"boom\")\n}\n"
                ),
            ),
            (
                "zz.emel",
                concat!(
                    "module zz\n\n",
                    "@test\nfn still_runs_after_the_trap() -> Unit uses {} { () }\n"
                ),
            ),
        ],
    );
    let output = emela_in(&dir, &["test"]);
    assert_eq!(output.status.code(), Some(1));
    let text = stdout(&output);
    assert!(
        text.contains("test util.strings.genuine_panic_is_a_failure ... FAILED"),
        "{text}"
    );
    assert!(
        text.contains("test zz.still_runs_after_the_trap ... ok"),
        "{text}"
    );
    assert!(
        text.contains("test result: FAILED. 1 passed; 1 failed"),
        "{text}"
    );
    let _ = fs::remove_dir_all(&dir);
}

// C8: every test green (and effects supplied, T4) exits 0.
#[test]
fn runner_all_green_exits_zero() {
    let dir = pome_fixture(
        "runner-green",
        &[(
            "io_test.emel",
            concat!(
                "module io_test\n\n",
                "import std.io\n\n",
                "@test\nfn prints_and_passes() -> Unit uses { Io } {\n",
                "    Io.print(\"debug: fine\\n\")\n    ()\n}\n"
            ),
        )],
    );
    let output = emela_in(&dir, &["test"]);
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    let text = stdout(&output);
    assert!(text.contains("running 1 test"), "{text}");
    assert!(
        text.contains("test result: ok. 1 passed; 0 failed"),
        "{text}"
    );
    // The passing test's own stdout is captured, not echoed (C9).
    assert!(!text.contains("debug: fine"), "{text}");
    let _ = fs::remove_dir_all(&dir);
}

// C1: `emela test` needs an enclosing Pome.
#[test]
fn runner_requires_a_pome() {
    let dir = scratch("runner-no-pome");
    let output = emela_in(&dir, &["test"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("Pome.toml"), "{}", stderr(&output));
    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Spec 0040: `[dev-dependencies]` (D rules)
// ---------------------------------------------------------------------------

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

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

/// Stands up a dev-dependency upstream (an assertion library) as a local git
/// repo, and a consumer Pome that lists it under `[dev-dependencies]`. Returns
/// `(consumer_dir, replace_spec, cache_dir)`.
fn dev_dep_fixture(tag: &str, consumer_files: &[(&str, &str)]) -> (PathBuf, String, PathBuf) {
    let root = scratch(tag);
    let upstream = root.join("upstream").join("devkit");
    fs::create_dir_all(upstream.join("src")).unwrap();
    fs::write(
        upstream.join("Pome.toml"),
        "[pome]\nname = \"github.com/test/devkit\"\nversion = \"0.1.0\"\nemela = \"0.1\"\n",
    )
    .unwrap();
    fs::write(
        upstream.join("src").join("assert.emel"),
        concat!(
            "module assert\n\n",
            "pub enum AssertError {\n    Failed(String)\n}\n\n",
            "impl Show for AssertError {\n",
            "    fn to_string(e: AssertError) -> String uses {} {\n",
            "        match e {\n",
            "            AssertError::Failed(message) -> \"assertion failed: \" ++ message\n",
            "        }\n    }\n}\n\n",
            "pub fn ok(condition: Bool) -> Unit throws AssertError uses {} {\n",
            "    if condition { () } else { throw AssertError::Failed(\"expected true\") }\n}\n"
        ),
    )
    .unwrap();
    git(&upstream, &["init", "-q"]);
    git(&upstream, &["add", "-A"]);
    git(&upstream, &["commit", "-q", "-m", "release"]);
    git(&upstream, &["tag", "-a", "v0.1.0", "-m", "v0.1.0"]);

    let consumer = root.join("consumer");
    fs::create_dir_all(consumer.join("src")).unwrap();
    fs::write(
        consumer.join("Pome.toml"),
        concat!(
            "[pome]\nname = \"consumer\"\nversion = \"0.1.0\"\nemela = \"0.1\"\n\n",
            "[dev-dependencies]\n\"github.com/test/devkit\" = \"^0.1\"\n"
        ),
    )
    .unwrap();
    for (path, source) in consumer_files {
        let file = consumer.join("src").join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, source).unwrap();
    }
    let replace = format!("github.com/test/devkit={}", upstream.display());
    let cache = root.join("cache");
    (consumer, replace, cache)
}

fn emela_pome(dir: &Path, replace: &str, cache: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_emela"))
        .current_dir(dir)
        .env("EMELA_POME_REPLACE", replace)
        .env("EMELA_POME_CACHE", cache)
        .args(args)
        .output()
        .unwrap()
}

// D1/D2/D3: a dev dependency resolves, is marked `dev = true` in the lock, and
// its import root serves `emela test`.
#[test]
fn dev_dependency_resolves_and_serves_tests() {
    if !git_available() {
        return;
    }
    let (consumer, replace, cache) = dev_dep_fixture(
        "dev-tests",
        &[(
            "checks.emel",
            concat!(
                "module checks\n\n",
                "import devkit.assert\n\n",
                "@test\nfn dev_dep_assert_passes() -> Unit uses {} {\n",
                "    assert.ok(true)\n}\n"
            ),
        )],
    );
    let install = emela_pome(&consumer, &replace, &cache, &["pome", "update"]);
    assert!(install.status.success(), "{}", stderr(&install));
    let lock = fs::read_to_string(consumer.join("Pome.lock")).unwrap();
    assert!(lock.contains("dev = true"), "{lock}");

    let output = emela_pome(&consumer, &replace, &cache, &["test"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}\n{}",
        stdout(&output),
        stderr(&output)
    );
    assert!(
        stdout(&output).contains("test result: ok. 1 passed; 0 failed"),
        "{}",
        stdout(&output)
    );
}

// D4: a build whose artifact reaches dev-dependency code is rejected; an
// unused import of the dev root is fine (tests stripped, nothing reachable).
#[test]
fn build_rejects_reachable_dev_dependency_code() {
    if !git_available() {
        return;
    }
    let (consumer, replace, cache) = dev_dep_fixture(
        "dev-build",
        &[(
            "main.emel",
            concat!(
                "import devkit.assert\n\n",
                "@test\nfn only_tests_use_the_dev_dep() -> Unit uses {} {\n",
                "    assert.ok(true)\n}\n\n",
                "fn main() -> Int uses {} { 0 }\n"
            ),
        )],
    );
    let install = emela_pome(&consumer, &replace, &cache, &["pome", "update"]);
    assert!(install.status.success(), "{}", stderr(&install));

    // Import present, tests stripped, nothing reachable: builds fine.
    let clean = emela_pome(&consumer, &replace, &cache, &["ir", "src/main.emel"]);
    assert!(clean.status.success(), "{}", stderr(&clean));

    // Production `main` reaching the dev dependency is the D4 error.
    fs::write(
        consumer.join("src").join("main.emel"),
        concat!(
            "import devkit.assert\n\n",
            "fn main() -> Int uses {} {\n",
            "    try { assert.ok(true) } catch { e -> () }\n",
            "    0\n}\n"
        ),
    )
    .unwrap();
    let leaking = emela_pome(&consumer, &replace, &cache, &["ir", "src/main.emel"]);
    assert!(!leaking.status.success());
    assert!(
        stderr(&leaking).contains("dev-dependency"),
        "{}",
        stderr(&leaking)
    );
}

// `emela pome add --dev` files the source under `[dev-dependencies]` (D1),
// pins it as dev in the lock (D2), and the tests can use it right away (D3).
#[test]
fn pome_add_dev_files_a_dev_dependency() {
    if !git_available() {
        return;
    }
    let (consumer, replace, cache) = dev_dep_fixture(
        "dev-add",
        &[(
            "checks.emel",
            concat!(
                "module checks\n\n",
                "import devkit.assert\n\n",
                "@test\nfn dev_dep_assert_passes() -> Unit uses {} {\n",
                "    assert.ok(true)\n}\n"
            ),
        )],
    );
    // Start from a manifest with no dependency tables at all; `add --dev`
    // creates the `[dev-dependencies]` section itself.
    fs::write(
        consumer.join("Pome.toml"),
        "[pome]\nname = \"consumer\"\nversion = \"0.1.0\"\nemela = \"0.1\"\n",
    )
    .unwrap();
    let add = emela_pome(
        &consumer,
        &replace,
        &cache,
        &[
            "pome",
            "add",
            "--dev",
            "github.com/test/devkit@^0.1",
            "--yes",
        ],
    );
    assert!(add.status.success(), "{}\n{}", stdout(&add), stderr(&add));
    let manifest = fs::read_to_string(consumer.join("Pome.toml")).unwrap();
    assert!(manifest.contains("[dev-dependencies]"), "{manifest}");
    assert!(!manifest.contains("\n[dependencies]"), "{manifest}");
    let lock = fs::read_to_string(consumer.join("Pome.lock")).unwrap();
    assert!(lock.contains("dev = true"), "{lock}");

    // Adding the same source again as a runtime dependency is rejected.
    let conflict = emela_pome(
        &consumer,
        &replace,
        &cache,
        &["pome", "add", "github.com/test/devkit@^0.1", "--yes"],
    );
    assert!(!conflict.status.success());
    assert!(
        stderr(&conflict).contains("already a dev-dependency"),
        "{}",
        stderr(&conflict)
    );

    let output = emela_pome(&consumer, &replace, &cache, &["test"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}\n{}",
        stdout(&output),
        stderr(&output)
    );
}
