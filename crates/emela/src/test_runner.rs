//! The test runner, `emela test` (spec 0040).
//!
//! The runner targets the current Pome (C1), discovers every `@test` function
//! declared under its `src/` tree (C2) — independent of import reachability —
//! and compiles each module with the ordinary frontend, `main` not required
//! (C3). Every test then runs in its own freshly instantiated `wasm-wasi`
//! module (C5): the module's `main` is a synthesized harness that calls the
//! test and returns exit code 0, so a clean exit is a pass and anything else —
//! the implicit-try trap a failed assertion raises (T3/C4), a `panic`, any
//! other trap — is a failure. stdout/stderr are captured per test; a failure's
//! detail is its captured stderr (C6), where the implicit-try wrap wrote the
//! `threw ...` report before trapping (C7).

use std::fs;
use std::path::{Path, PathBuf};

use emela_codegen::{BackendOptions, EffectRow, FunctionType, IrExpr, IrFunction, IrProgram, Type};

use crate::error::{Error, Result};
use crate::lower;
use crate::run::{Captured, RunOutcome, execute_captured};

/// One discovered test: its ID (`module.function`, spec 0040 T6), the emit name
/// of its lowered function, and its declared effect row (the harness `main`
/// declares the same row).
struct TestCase {
    id: String,
    emit_name: String,
    effects: EffectRow,
}

/// A module with tests: its lowered IR, shared by every harness built from it.
struct ModuleTests {
    tests: Vec<TestCase>,
    ir: IrProgram,
}

pub(crate) fn run() -> Result<()> {
    let project = crate::pome::project_dir()?;
    let src = project.join("src");
    if !src.is_dir() {
        return Err(Error::new(format!(
            "no `src/` directory under `{}`; a Pome's modules live in `src/` (spec 0032)",
            project.display()
        )));
    }
    let mut files = Vec::new();
    collect_emel_files(&src, &mut files)?;
    files.sort();

    // Compile every module first (C2/C8): a compile error anywhere reports as
    // usual and no test runs.
    let mut compile_errors: Vec<Error> = Vec::new();
    let mut modules: Vec<ModuleTests> = Vec::new();
    for file in &files {
        let (program, typed) = match crate::driver::compile_frontend(file, &[], false) {
            Ok(compiled) => compiled,
            Err(error) => {
                compile_errors.push(error);
                continue;
            }
        };
        let module_id = module_id_of(&src, file);
        // Only the module's own tests run (C2): imported modules' tests carry a
        // non-empty `module_path` and are discovered when their file is the
        // compilation root instead.
        let tests: Vec<TestCase> = program
            .functions
            .iter()
            .filter(|function| function.is_test && function.module_path.is_empty())
            .map(|function| TestCase {
                id: format!("{module_id}.{}", function.name),
                // A compilation-root function's emit name is its bare name.
                emit_name: function.name.clone(),
                effects: function.effects.clone(),
            })
            .collect();
        if tests.is_empty() {
            continue;
        }
        let ir = lower::lower(&program, &typed);
        modules.push(ModuleTests { tests, ir });
    }
    if !compile_errors.is_empty() {
        return Err(Error::new(
            compile_errors
                .iter()
                .map(Error::render)
                .collect::<Vec<_>>()
                .join("\n\n"),
        ));
    }

    let backend = crate::driver::registry();
    let backend = backend
        .get("wasm-wasi")
        .ok_or_else(|| Error::new("`emela test` needs the `wasm-wasi` backend"))?;

    let total: usize = modules.iter().map(|module| module.tests.len()).sum();
    println!("running {total} test{}", if total == 1 { "" } else { "s" });

    struct Failure {
        id: String,
        detail: String,
    }
    let mut failures: Vec<Failure> = Vec::new();
    for module in &modules {
        for test in &module.tests {
            let ir = harness_program(&module.ir, test);
            let artifact = backend
                .compile(&ir, &BackendOptions::default())
                .map_err(Error::from)?;
            let (outcome, captured) = execute_captured(&artifact.bytes)?;
            let passed = matches!(outcome, RunOutcome::Exit(0));
            println!(
                "test {} ... {}",
                test.id,
                if passed { "ok" } else { "FAILED" }
            );
            if !passed {
                failures.push(Failure {
                    id: test.id.clone(),
                    detail: failure_detail(outcome, &captured),
                });
            }
        }
    }

    if !failures.is_empty() {
        println!("\nfailures:");
        for failure in &failures {
            println!("\n---- {} ----", failure.id);
            print!("{}", failure.detail);
        }
    }
    let failed = failures.len();
    let passed = total - failed;
    println!();
    if failed == 0 {
        println!("test result: ok. {passed} passed; 0 failed");
        Ok(())
    } else {
        println!("test result: FAILED. {passed} passed; {failed} failed");
        // All tests ran; a non-zero exit reports the failures (C8).
        std::process::exit(1)
    }
}

/// Builds the per-test module (C3/C5): the shared lowered IR minus any user
/// `main` (present but never the entry), plus a synthesized harness `main`
/// that calls the test and exits 0. Failure never returns through `main` — a
/// failed test traps out of the call (T3/C4).
fn harness_program(base: &IrProgram, test: &TestCase) -> IrProgram {
    let mut functions: Vec<IrFunction> = base
        .functions
        .iter()
        .filter(|function| function.name != "main")
        .cloned()
        .collect();
    functions.push(IrFunction {
        name: "main".to_string(),
        params: Vec::new(),
        ret: Type::Int,
        throws: None,
        effects: test.effects.clone(),
        body: IrExpr::Let {
            name: "__test_outcome".to_string(),
            value_ty: Type::Unit,
            value: Box::new(IrExpr::Call {
                callee: Box::new(IrExpr::FunctionRef {
                    name: test.emit_name.clone(),
                    sig: FunctionType {
                        params: Vec::new(),
                        ret: Box::new(Type::Unit),
                        throws: None,
                        effects: test.effects.clone(),
                    },
                }),
                args: Vec::new(),
                ret: Type::Unit,
            }),
            next: Box::new(IrExpr::Int(0)),
        },
    });
    IrProgram { functions }
}

/// The failure detail shown under `failures:` (C6): the captured stderr — where
/// the implicit-try wrap wrote its `threw ...` report (C7) — or the trap/exit
/// description when nothing was written (a genuine `panic` or stray exit).
fn failure_detail(outcome: RunOutcome, captured: &Captured) -> String {
    let stderr = String::from_utf8_lossy(&captured.stderr);
    match outcome {
        RunOutcome::Exit(code) => format!("{stderr}test exited with code {code}\n"),
        RunOutcome::Trap(trap) => {
            if stderr.trim().is_empty() {
                format!("trapped: {trap}\n")
            } else {
                stderr.into_owned()
            }
        }
    }
}

/// The test-ID module component (T6): the file's path relative to the source
/// root, `.emel` stripped, separators as dots (`src/util/strings.emel` →
/// `util.strings`).
fn module_id_of(src: &Path, file: &Path) -> String {
    let rel = file.strip_prefix(src).unwrap_or(file);
    let mut parts: Vec<String> = rel
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    if let Some(last) = parts.last_mut()
        && let Some(stem) = last.strip_suffix(".emel")
    {
        *last = stem.to_string();
    }
    parts.join(".")
}

/// Recursively collects `*.emel` files, skipping hidden directories and build
/// output — the same convention as `emela fmt` (C2).
fn collect_emel_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let entries = fs::read_dir(dir)
        .map_err(|error| Error::new(format!("failed to read {}: {error}", dir.display())))?;
    let mut children: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    children.sort();
    for child in children {
        let name = child
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if child.is_dir() {
            if name.starts_with('.') || name == "target" {
                continue;
            }
            collect_emel_files(&child, files)?;
        } else if name.ends_with(".emel") {
            files.push(child);
        }
    }
    Ok(())
}
