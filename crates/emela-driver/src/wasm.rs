//! The WASM target (spec chapter 16, Tier 1): the output stage that turns the Core IR into a
//! WASI 0.3 component with emela-codegen-wasm and runs it with `wasmtime`.

use std::path::PathBuf;

use emela_codegen_wasm::{
    Entry, TestEntry, WasmError, WasmErrorKind, WasmOptions, compile_with_wasm_options,
};

use crate::analysis::Analysis;
use crate::code;
use crate::diagnostic::Diagnostic;
use crate::output::{BuildOutput, OutputFile};
use crate::pipeline::{Backend, Runner};
use crate::testing::{TestBackend, TestModule};

/// The component written to the output directory.
pub const WASM_ENTRY_FILE: &str = "main.wasm";

/// Turns the Core IR into a WASI 0.3 component (`emela build --target wasm`), exporting
/// `wasi:cli/run@0.3.0`. It is run with `wasmtime -S p3=y`.
#[derive(Debug, Clone, Copy)]
pub struct CoreWasm {
    /// Whether the LIR optimisation passes run (`emela_lir::to_heap_with`). On by default.
    optimize: bool,
    /// Which LIR passes run (see [`emela_lir::opt::Options`]). The default: the M3 memory passes
    /// that pay for themselves run, as do switch normalisation and closure devirtualisation with
    /// closure SRA; drop specialisation, reuse and record SRA do not.
    lir_options: emela_lir::opt::Options,
    /// The emitter's representation choices (see [`WasmOptions`]). The default holds eligible
    /// records and tuples by value (E1 step 6); [`WasmOptions::boxed`] is the representation from
    /// before step 6.
    wasm_options: WasmOptions,
}

impl Default for CoreWasm {
    fn default() -> Self {
        CoreWasm {
            optimize: true,
            lir_options: emela_lir::opt::Options::default(),
            wasm_options: WasmOptions::default(),
        }
    }
}

impl CoreWasm {
    /// Turns the LIR optimisation passes on or off (`emela build --no-opt`).
    pub fn with_optimize(mut self, optimize: bool) -> Self {
        self.optimize = optimize;
        self
    }

    /// Chooses which LIR passes run, for a test or a tool that needs a pass the default build
    /// leaves off (`emela_lir::opt::LirRules`). It does not change the default backend.
    pub fn with_lir_options(mut self, options: emela_lir::opt::Options) -> Self {
        self.lir_options = options;
        self
    }

    /// Chooses the emitter's representation (`WasmOptions`). The default is
    /// [`WasmOptions::default`] (records and tuples by value), which is the build `emela` makes;
    /// [`WasmOptions::boxed`] is the representation from before E1 step 6.
    pub fn with_wasm_options(mut self, options: WasmOptions) -> Self {
        self.wasm_options = options;
        self
    }
}

impl Backend<emela_core::Module> for CoreWasm {
    fn emit(
        &mut self,
        module: &emela_core::Module,
        _: &Analysis,
    ) -> Result<BuildOutput, Vec<Diagnostic>> {
        let out = compile_with_wasm_options(
            module,
            &Entry::Main,
            self.optimize,
            self.lir_options,
            self.wasm_options,
        )
        .map_err(diagnostics)?;
        Ok(BuildOutput {
            files: vec![OutputFile::new(WASM_ENTRY_FILE, out.module)],
            entry: PathBuf::from(WASM_ENTRY_FILE),
        })
    }

    fn runner(&self) -> Runner {
        Runner::Wasmtime
    }
}

impl TestBackend<TestModule> for CoreWasm {
    fn test_names(&self, program: &TestModule) -> Vec<String> {
        program.tests.iter().map(|t| t.name.clone()).collect()
    }

    fn emit_tests(
        &mut self,
        program: &TestModule,
        selected: &[usize],
        _: &Analysis,
    ) -> Result<BuildOutput, Vec<Diagnostic>> {
        let tests = selected
            .iter()
            .map(|&i| TestEntry {
                name: program.tests[i].name.clone(),
                function: program.tests[i].function,
            })
            .collect();
        let out = compile_with_wasm_options(
            &program.module,
            &Entry::Tests(tests),
            self.optimize,
            self.lir_options,
            self.wasm_options,
        )
        .map_err(diagnostics)?;
        Ok(BuildOutput {
            files: vec![OutputFile::new(WASM_ENTRY_FILE, out.module)],
            entry: PathBuf::from(WASM_ENTRY_FILE),
        })
    }

    fn runner(&self) -> Runner {
        Runner::Wasmtime
    }
}

/// emela-codegen-wasm's errors as diagnostics without a location (spec
/// `[target.wasm.unsupported]`). Only external functions called back by the host get a code
/// (E5201).
fn diagnostics(errors: Vec<WasmError>) -> Vec<Diagnostic> {
    errors
        .into_iter()
        .map(|e| {
            let d = Diagnostic::error(e.message);
            match e.kind {
                WasmErrorKind::Callback => d.with_code(code::CALLBACK_EXTERNAL),
                // The native target supports `suspend fn` external functions (12.6). WASM does
                // not, and reports it without a code (`[target.wasm.external]`).
                WasmErrorKind::SuspendExternal
                | WasmErrorKind::Unsupported
                | WasmErrorKind::Internal => d,
            }
        })
        .collect()
}
