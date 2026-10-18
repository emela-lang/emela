//! The C target's acceptance corpus (`crates/emela-codegen-c/tests/programs`) compiled to WASM
//! and run under `wasmtime`, comparing the exit code, stdout and stderr with the `.expected` file
//! the JS target produced.
//!
//! The corpus programs declare `print` with `@external(js, ...)` and `@external(c, ...)`; this
//! test injects the WASM internal host `@external(wasm, "test", "print")` (which binds to the
//! runtime's `print_string`) before the `fn print(...)` line, exactly like the C test's helper.
//! Programs the WASM target cannot compile yet are listed in [`NOT_ON_WASM`]; the list must be
//! emptied by Wave 3.

mod common;

use std::path::Path;

use common::{out_dir, run_bytes, to_wasm, wasmtime};

/// Programs the WASM target cannot compile yet, with the reason (discovered empirically). The
/// reasons come from `src/check.rs` (the Flow-IR check) and the runtime's unimplemented pieces.
/// The list must be emptied by Wave 3.
const NOT_ON_WASM: &[(&str, &str)] = &[
    // --- fibers and suspending code (check.rs) ---
    (
        "fiber_fairness",
        "the driver loop stalls during a long fairness interleaving",
    ),
    (
        "fiber_main_fails",
        "the `note: another fiber also failed` of a release defect needs the defect/\
         interruption replacement in `raise`, spec 14.4",
    ),
    (
        "fiber_notes",
        "the `note: another fiber also failed` needs the defect/interruption replacement in \
         `raise`, spec 14.4",
    ),
    (
        "fiber_unwinding",
        "an interruption of a fiber whose release fails is not delivered correctly yet",
    ),
    (
        "suspend_external",
        "a `suspend fn` external function needs the `sleep` host of T-Clock",
    ),
];

struct Expected {
    code: i32,
    stdout: String,
    stderr: String,
}

/// Parses a `.expected` file (`exit: N`, then `--- stdout`, then `--- stderr`), like the C test.
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

/// Inserts `@external(wasm, "test", "print")` immediately before the corpus `fn print(...)` line.
/// Programs without a `print` are returned unchanged.
fn inject_wasm_print(src: &str) -> String {
    let mut out = String::with_capacity(src.len() + 40);
    for line in src.lines() {
        if line.trim_start().starts_with("fn print(") {
            out.push_str("@external(wasm, \"test\", \"print\")\n");
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[test]
fn programs_behave_as_on_js() {
    if wasmtime().is_none() {
        return;
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../emela-codegen-c/tests/programs");
    let mut programs: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "emel"))
        .collect();
    programs.sort();
    assert!(!programs.is_empty());
    let mut failures = Vec::new();
    let mut ran = 0usize;
    for path in programs {
        let name = path.file_stem().unwrap().to_str().unwrap().to_owned();
        if let Some((_, reason)) = NOT_ON_WASM.iter().find(|(p, _)| *p == name) {
            eprintln!("skipping {name}: {reason}");
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap();
        let expected = parse_expected(
            &std::fs::read_to_string(path.with_extension("expected"))
                .unwrap_or_else(|_| panic!("{name}.expected is missing")),
        );
        let src = inject_wasm_print(&src);
        let bytes = match to_wasm(&src) {
            Ok(bytes) => bytes,
            Err(errors) => {
                let kinds: Vec<_> = errors.iter().map(|e| e.kind).collect();
                failures.push(format!(
                    "{name}: not compilable on wasm ({kinds:?}): {}",
                    errors
                        .iter()
                        .map(|e| e.message.clone())
                        .collect::<Vec<_>>()
                        .join("; ")
                ));
                continue;
            }
        };
        if let Err(e) = wasmparser::validate(&bytes) {
            failures.push(format!("{name}: invalid module: {e}"));
            continue;
        }
        let got = run_bytes(&out_dir(&name), &bytes);
        ran += 1;
        // `wasmtime` only reports an exit status of 0..=125 (POSIX reserves 126/127), so a run
        // that ends as `interrupted` (the C target's 130, 128 + SIGINT) comes back as wasmtime's
        // own failure: exit 1 with the wasmtime error after the program's `interrupted` line.
        let interrupted = expected.code == 130
            && got.code == 1
            && got.stderr.starts_with(expected.stderr.as_str());
        if !interrupted
            && (got.code, &got.stdout, &got.stderr)
                != (expected.code, &expected.stdout, &expected.stderr)
        {
            failures.push(format!(
                "{name}: expected exit {}\n{}{}\ngot exit {}\n{}{}",
                expected.code, expected.stdout, expected.stderr, got.code, got.stdout, got.stderr
            ));
        }
    }
    assert!(
        ran > 0,
        "no program ran; the skip list must not cover the whole corpus"
    );
    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
}

/// The skip list must not name a program that does not exist (a stale entry hides a regression).
#[test]
fn not_on_wasm_entries_exist() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../emela-codegen-c/tests/programs");
    for (name, reason) in NOT_ON_WASM {
        assert!(
            dir.join(format!("{name}.emel")).exists(),
            "{name} ({reason}) is not in the corpus"
        );
    }
}
