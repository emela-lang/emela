//! Emits C11 from the Heap IR: the C target (spec chapter 16, Tier 2).
//!
//! [`compile`] runs the whole pipeline from lowered Core IR: monomorphisation
//! (`emela_core::monomorphize_from`, from the entry), the Flow IR (`emela_lir::from_core`), the
//! Heap IR passes (`emela_lir::to_heap`: closure conversion, explicit defects, state machines,
//! reference counting)
//! and then this crate's emitter, which follows the Heap IR mechanically. The C runtime
//! ([`RUNTIME_C`], [`RUNTIME_H`]) is embedded and written next to the output; the caller compiles
//! both with cc (see `NOTES.md` for the flags).
//!
//! Suspending functions are emitted from the Heap IR's state machines, and the runtime schedules
//! fibers the way the JS runtime does (spec chapter 11, 14.5). `suspend fn` external functions
//! complete a task later, through the runtime's event loop ([`EMELA_H`], spec 12.6). There are
//! no callbacks to host functions and no builtins that need Unicode tables; reaching them is a
//! [`CError`].

mod check;
mod emit;
mod names;
mod writer;

use emela_core::FnId;

/// The C runtime implementation, written next to the output as [`RUNTIME_C_FILE`].
pub const RUNTIME_C: &str = include_str!("emela_runtime.c");
/// The C runtime header, written next to the output as [`RUNTIME_H_FILE`].
pub const RUNTIME_H: &str = include_str!("emela_runtime.h");
pub const RUNTIME_C_FILE: &str = "emela_runtime.c";
pub const RUNTIME_H_FILE: &str = "emela_runtime.h";
/// The public header for host C files (spec 12.6: tasks, completions, the event loop), written
/// next to the output as [`EMELA_H_FILE`]. The runtime header includes it, and host C files
/// include it as `"emela.h"` (the caller compiles with `-I` the output directory).
pub const EMELA_H: &str = include_str!("emela.h");
pub const EMELA_H_FILE: &str = "emela.h";
/// The directory, next to the output, where the caller writes the bundled host files that the
/// program uses ([`COutput::bundled`]).
pub const BUNDLED_DIR: &str = "emela_core";

/// What the generated `main` runs.
#[derive(Debug, Clone)]
pub enum Entry {
    /// The entry module's `main` (an exported fn named `main`). Without one, the program does
    /// nothing.
    Main,
    /// Test functions (spec 13.4), called one by one in order. Each result is written to stdout
    /// as a line of [`TEST_EVENT_PREFIX`] and JSON, the protocol of the JS test runner: `{"event":
    /// "result", "name": …, "outcome": "ok" | "defect" | "error", "message": …}`, then `{"event":
    /// "done"}`.
    Tests(Vec<TestEntry>),
}

/// A test function of [`Entry::Tests`].
#[derive(Debug, Clone)]
pub struct TestEntry {
    /// The name shown in the results.
    pub name: String,
    /// The test function in the Core module. It takes no arguments.
    pub function: FnId,
}

/// The start of a test result line (the same as the JS test runner's).
pub const TEST_EVENT_PREFIX: &str = "\u{1e}emela-test ";

/// The output of [`compile`].
#[derive(Debug, Clone)]
pub struct COutput {
    /// The program (`main.c`). It includes [`RUNTIME_H_FILE`].
    pub main: String,
    /// Bundled host files the program's external functions point to (`HostModule::Bundled`), as
    /// (path relative to `lib/`, contents). The caller writes them under [`BUNDLED_DIR`] and
    /// compiles them with the program.
    pub bundled: Vec<(String, String)>,
    /// C sources of the project that external functions point to (`HostModule::File`, absolute
    /// paths). The caller compiles them with the program.
    pub host_files: Vec<String>,
}

/// Why a program cannot be compiled to C.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CError {
    pub kind: CErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CErrorKind {
    /// A feature the C target does not support yet (Unicode tables).
    Unsupported,
    /// An external function reachable from the entry takes or returns a callback (E0427).
    Callback,
    /// A bug in the compiler (the IR failed a verifier, ...).
    Internal,
}

impl CError {
    fn new(kind: CErrorKind, message: impl Into<String>) -> Self {
        CError {
            kind,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for CError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Compiles lowered Core IR (self tail calls loopified, as `emela_core::lower` leaves it) to C.
pub fn compile(core: &emela_core::Module, entry: &Entry) -> Result<COutput, Vec<CError>> {
    let roots: Vec<FnId> = match entry {
        Entry::Main => core
            .functions
            .iter()
            .filter(|(_, f)| f.exported && f.name == "main")
            .map(|(id, _)| id)
            .collect(),
        Entry::Tests(tests) => tests.iter().map(|t| t.function).collect(),
    };
    // A generic root (a test whose inferred type has type parameters) is instantiated too. One
    // that cannot be (it takes arguments) is reported rather than skipped.
    let (mono, copies) = emela_core::monomorphize_from(core, &roots);
    let mut copied = Vec::with_capacity(roots.len());
    for (&root, copy) in roots.iter().zip(copies) {
        match copy {
            // The Flow IR functions are the Core functions in the same order, so a root keeps the
            // index of its copy.
            Some(copy) => copied.push(emela_lir::FnId::from_raw(la_arena::RawIdx::from(
                u32::from(copy.into_raw()),
            ))),
            None => {
                return Err(vec![CError::new(
                    CErrorKind::Internal,
                    format!(
                        "internal error in monomorphisation: the entry `{}` is generic and takes \
                         arguments, so it has no instantiation",
                        core.functions[root].name
                    ),
                )]);
            }
        }
    }
    let entry = match entry {
        Entry::Main => emit::Run::Main(copied.first().copied()),
        Entry::Tests(tests) => emit::Run::Tests(
            tests
                .iter()
                .zip(copied)
                .map(|(t, copy)| (t.name.clone(), copy))
                .collect(),
        ),
    };
    if let Err(errors) = emela_core::verify_monomorphic(&mono) {
        return Err(internal("monomorphisation", errors));
    }
    let mut lir = emela_lir::from_core(mono).map_err(|errors| internal("from_core", errors))?;
    let errors = check::unsupported(&lir);
    if !errors.is_empty() {
        return Err(errors);
    }
    emela_lir::to_heap(&mut lir);
    if cfg!(debug_assertions)
        && let Err(errors) = emela_lir::verify(&lir)
    {
        return Err(internal("the Heap IR verifier", errors));
    }
    emit::emit(&lir, &entry)
}

fn internal<E: std::fmt::Display>(stage: &str, errors: Vec<E>) -> Vec<CError> {
    let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
    vec![CError::new(
        CErrorKind::Internal,
        format!("internal error in {stage}:\n{}", lines.join("\n")),
    )]
}
