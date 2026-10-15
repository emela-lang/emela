//! Programs compiled to C, built with cc and run. Each `tests/programs/NAME.emel` has the output
//! of the JS target in `NAME.expected` (the exit code, stdout and stderr, written by running the
//! program with `emela run`), and the C build must give the same. Every run checks for leaked
//! allocations, and runs again with ASan and UBSan when cc supports them.

mod common;

use std::path::Path;

use common::{PRELUDE, cc, run, to_c};
use emela_codegen_c::CErrorKind;

struct Expected {
    code: i32,
    stdout: String,
    stderr: String,
}

fn parse_expected(text: &str) -> Expected {
    let (head, rest) = text.split_once("\n--- stdout\n").expect("an exit line");
    let (stdout, stderr) = rest.split_once("--- stderr\n").expect("a stderr section");
    Expected {
        code: head
            .trim_start_matches("exit: ")
            .parse()
            .expect("an exit code"),
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
    }
}

#[test]
fn programs_behave_as_on_js() {
    if cc().is_none() {
        return;
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/programs");
    let mut programs: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "emel"))
        .collect();
    programs.sort();
    assert!(!programs.is_empty());
    let mut failures = Vec::new();
    for path in programs {
        let name = path.file_stem().unwrap().to_str().unwrap().to_owned();
        let src = std::fs::read_to_string(&path).unwrap();
        let expected = parse_expected(
            &std::fs::read_to_string(path.with_extension("expected"))
                .unwrap_or_else(|_| panic!("{name}.expected is missing")),
        );
        let got = run(&name, &src).expect("cc is available");
        if (got.code, &got.stdout, &got.stderr)
            != (expected.code, &expected.stdout, &expected.stderr)
        {
            failures.push(format!(
                "{name}: expected exit {}\n{}{}\ngot exit {}\n{}{}",
                expected.code, expected.stdout, expected.stderr, got.code, got.stdout, got.stderr
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
}

/// The message of the only error compiling `src` (after [`PRELUDE`]) to C.
fn error_of(src: &str) -> (CErrorKind, String) {
    let errors = to_c(&format!("{PRELUDE}{src}")).expect_err("compiling should fail");
    assert_eq!(errors.len(), 1, "{errors:#?}");
    (errors[0].kind, errors[0].message.clone())
}

#[test]
fn grapheme_builtins_are_rejected() {
    assert_eq!(
        error_of("fn main() { print(Int.to_string(String.length(\"a\"))) }\n"),
        (
            CErrorKind::Unsupported,
            "`String.length` needs the Unicode grapheme cluster tables, which target `c` does not support yet".into()
        )
    );
    assert_eq!(
        error_of("fn main() { print(String.join(String.split(\"ab\", \"\"), \",\")) }\n").1,
        "`String.split` with an empty separator needs the Unicode grapheme cluster tables, which target `c` does not support yet"
    );
}

#[test]
fn callbacks_to_c_are_rejected() {
    let (kind, message) = error_of(
        r#"
@external(c, "emela_on")
fn on(f: fn(Int) -> Int) -> () use {}

fn main() { on(fn(x) { x }) }
"#,
    );
    assert_eq!(kind, CErrorKind::Callback);
    assert_eq!(
        message,
        "external function `on` takes a callback, which target `c` does not support yet"
    );
}
