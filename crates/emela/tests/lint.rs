//! End-to-end tests for `emela lint` (spec 0035): each rule fires on a
//! violation and stays quiet on clean code, findings carry rule ids and are
//! all printed, only the root file is reported, and the exit codes follow
//! L5 (0 clean / 1 findings).

use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> std::path::PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-lint-test-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Lints `source` as a single file and returns (success, stderr).
fn lint(source: &str) -> (bool, String) {
    let dir = temp_dir();
    let input = dir.join("main.emel");
    fs::write(&input, source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("lint")
        .arg(&input)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(&dir);
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn lint_clean(source: &str) {
    let (success, stderr) = lint(source);
    assert!(success, "expected no findings, got:\n{stderr}");
    assert!(stderr.is_empty(), "expected empty stderr, got:\n{stderr}");
}

fn lint_warns(source: &str, rule: &str) -> String {
    let (success, stderr) = lint(source);
    assert!(!success, "expected findings, but lint passed");
    assert!(
        stderr.contains(&format!("[{rule}]")),
        "expected a `{rule}` finding, got:\n{stderr}"
    );
    stderr
}

const CLEAN: &str = "fn add(x: Int, y: Int) -> Int {\n    x + y\n}\n\nfn main() -> Unit uses {} {\n    let sum = add(1, 2)\n    let _ignored = sum\n    ()\n}\n";

#[test]
fn clean_file_exits_zero_with_no_output() {
    lint_clean(CLEAN);
}

#[test]
fn snake_case_rule() {
    lint_warns(
        "fn BadName() -> Int {\n    3\n}\n\nfn main() -> Unit uses {} {\n    let _x = BadName()\n    ()\n}\n",
        "naming/snake-case",
    );
}

#[test]
fn pascal_case_rule() {
    lint_warns(
        "enum bad_color {\n    Red\n}\n\nfn main() -> Unit uses {} {\n    let _c = bad_color::Red\n    ()\n}\n",
        "naming/pascal-case",
    );
}

#[test]
fn unused_let_rule_and_underscore_exemption() {
    let stderr = lint_warns(
        "fn main() -> Unit uses {} {\n    let unused = 1\n    let _intentional = 2\n    ()\n}\n",
        "bindings/unused-let",
    );
    assert!(stderr.contains("`unused`"));
    assert!(
        !stderr.contains("_intentional"),
        "`_`-prefixed bindings are exempt:\n{stderr}"
    );
}

#[test]
fn unused_param_rule_and_underscore_exemption() {
    let stderr = lint_warns(
        "fn f(unused: Int, _ok: Int) -> Int {\n    1\n}\n\nfn main() -> Unit uses {} {\n    let _x = f(1, 2)\n    ()\n}\n",
        "bindings/unused-param",
    );
    assert!(stderr.contains("`unused`"));
    assert!(!stderr.contains("`_ok`"), "{stderr}");
}

#[test]
fn impl_method_params_are_exempt_from_unused_param() {
    // The parameter list of an impl method is fixed by the trait; an unused
    // parameter there is not the author's choice.
    lint_clean(
        "enum Wrap {\n    One(Int)\n}\n\ntrait Weigh {\n    fn weigh(value: Self, scale: Int) -> Int\n}\n\nimpl Weigh for Wrap {\n    fn weigh(value: Self, scale: Int) -> Int {\n        7\n    }\n}\n\nfn main() -> Unit uses {} {\n    let _n = Weigh.weigh(Wrap::One(3), 2)\n    ()\n}\n",
    );
}

#[test]
fn over_declared_effects_rule() {
    // `quiet` declares Stdout but needs nothing; `main` inherits Stdout from
    // the call (a callee's declared row propagates), so only `quiet` warns.
    let stderr = lint_warns(
        "fn quiet() -> Int uses { Stdout } {\n    3\n}\n\nfn main() -> Unit uses { Stdout } {\n    let _x = quiet()\n    ()\n}\n",
        "effects/over-declared",
    );
    assert!(stderr.contains("`Stdout`"), "{stderr}");
    assert!(stderr.contains("1 warning emitted"), "{stderr}");
}

#[test]
fn declared_and_used_effects_are_clean() {
    // Calling through an effectful function value (spec 0008) makes the body
    // genuinely require Stdout, so the declared row is not over-declared.
    lint_clean(
        "fn call_it(f: () -> Unit uses { Stdout }) -> Unit uses { Stdout } {\n    f()\n}\n\nfn main() -> Unit uses {} {\n    ()\n}\n",
    );
}

#[test]
fn unused_import_rule_and_root_only_reporting() {
    let dir = temp_dir();
    fs::create_dir_all(dir.join("util")).unwrap();
    // The imported module contains its own naming violation (`Helper`) and an
    // over-declared effect; neither may be reported for the root file (L2).
    fs::write(
        dir.join("util/math.emel"),
        "module util.math\n\npub fn double(x: Int) -> Int {\n    x * 2\n}\n\npub fn Helper() -> Int uses { Stdout } {\n    3\n}\n",
    )
    .unwrap();
    let input = dir.join("main.emel");
    fs::write(
        &input,
        "import util.math.double\n\nfn main() -> Unit uses {} {\n    ()\n}\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("lint")
        .arg(&input)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("[imports/unused]"), "{stderr}");
    assert!(
        stderr.contains("`double` is imported but never used"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("Helper") && !stderr.contains("over-declared"),
        "imported-module findings leaked into the root report:\n{stderr}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn used_import_is_clean() {
    let dir = temp_dir();
    fs::create_dir_all(dir.join("util")).unwrap();
    fs::write(
        dir.join("util/math.emel"),
        "module util.math\n\npub fn double(x: Int) -> Int {\n    x * 2\n}\n",
    )
    .unwrap();
    let input = dir.join("main.emel");
    fs::write(
        &input,
        "import util.math.double\n\nfn main() -> Unit uses {} {\n    let _x = double(4)\n    ()\n}\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("lint")
        .arg(&input)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn multiple_findings_are_all_printed_in_order() {
    let (success, stderr) = lint(
        "fn BadOne() -> Int {\n    let unused = 1\n    2\n}\n\nfn main() -> Unit uses {} {\n    let _x = BadOne()\n    ()\n}\n",
    );
    assert!(!success);
    let bad_name = stderr.find("[naming/snake-case]").expect(&stderr);
    let unused_let = stderr.find("[bindings/unused-let]").expect(&stderr);
    assert!(
        bad_name < unused_let,
        "findings not in source order:\n{stderr}"
    );
    assert!(stderr.contains("2 warnings emitted"), "{stderr}");
}

#[test]
fn type_error_reports_the_error_not_lints() {
    let (success, stderr) = lint(
        "fn BadName() -> Int {\n    \"not an int\"\n}\n\nfn main() -> Unit uses {} {\n    ()\n}\n",
    );
    assert!(!success);
    assert!(stderr.contains("error:"), "{stderr}");
    assert!(
        !stderr.contains("[naming/snake-case]"),
        "lints must not be reported for a file that fails the frontend:\n{stderr}"
    );
}
