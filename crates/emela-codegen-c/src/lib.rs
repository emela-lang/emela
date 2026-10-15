//! Emits C11 from the Heap IR: the C target (spec chapter 16, Tier 2).
//!
//! [`compile`] runs the whole pipeline from lowered Core IR: monomorphisation
//! (`emela_core::monomorphize_from`, from the entry), the Flow IR (`emela_lir::from_core`), the
//! Heap IR passes (`emela_lir::to_heap`: closure conversion, explicit defects, reference counting)
//! and then this crate's emitter, which follows the Heap IR mechanically. The C runtime
//! ([`RUNTIME_C`], [`RUNTIME_H`]) is embedded and written next to the output; the caller compiles
//! both with cc (see `NOTES.md` for the flags).
//!
//! The first version supports no programs that suspend (the Heap IR leaves suspending functions
//! unconverted), no fibers, no callbacks to host functions and no builtins that need Unicode
//! tables; reaching them is a [`CError`].

mod check;
mod emit;
mod names;

use emela_core::FnId;

/// The C runtime implementation, written next to the output as [`RUNTIME_C_FILE`].
pub const RUNTIME_C: &str = include_str!("emela_runtime.c");
/// The C runtime header, written next to the output as [`RUNTIME_H_FILE`].
pub const RUNTIME_H: &str = include_str!("emela_runtime.h");
pub const RUNTIME_C_FILE: &str = "emela_runtime.c";
pub const RUNTIME_H_FILE: &str = "emela_runtime.h";
/// The directory, next to the output, where the caller writes the bundled host files that the
/// program uses ([`COutput::bundled`]).
pub const BUNDLED_DIR: &str = "emela_core";

/// What the generated `main` runs.
#[derive(Debug, Clone)]
pub enum Entry {
    /// The entry module's `main` (an exported fn named `main`). Without one, the program does
    /// nothing.
    Main,
}

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
    /// A feature the C target does not support yet (suspending code, fibers, Unicode tables).
    Unsupported,
    /// An external function reachable from the entry takes or returns a callback (E0427).
    Callback,
    /// A `suspend fn` external function with `@external(c, ...)` (E0428).
    SuspendExternal,
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
    };
    let (mono, _) = emela_core::monomorphize_from(core, &roots);
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
    emit::emit(&lir, entry)
}

fn internal<E: std::fmt::Display>(stage: &str, errors: Vec<E>) -> Vec<CError> {
    let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
    vec![CError::new(
        CErrorKind::Internal,
        format!("internal error in {stage}:\n{}", lines.join("\n")),
    )]
}
