//! The WASM target (spec chapter 16, Tier 1): the output stage that turns the Core IR into WASM
//! with emela-codegen-wasm and runs the module with `wasmtime` (WASI).

use std::path::PathBuf;

use emela_codegen_wasm::{Entry, TestEntry, WasmError, WasmErrorKind, compile};

use crate::code;
use crate::diagnostic::Diagnostic;
use crate::output::{JsOutput, OutputFile};
use crate::pipeline::{Analysis, JsBackend, Runner};
use crate::testing::{TestBackend, TestModule};

/// The module written to the output directory.
pub const WASM_ENTRY_FILE: &str = "main.wasm";

/// Turns the Core IR into WASM (`emela build --target wasm`). The module is a WASI command run
/// with `wasmtime`.
#[derive(Debug, Default, Clone, Copy)]
pub struct CoreWasm;

impl JsBackend<emela_core::Module> for CoreWasm {
    fn emit(
        &mut self,
        module: &emela_core::Module,
        _: &Analysis,
    ) -> Result<JsOutput, Vec<Diagnostic>> {
        let out = compile(module, &Entry::Main).map_err(diagnostics)?;
        Ok(JsOutput {
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
    ) -> Result<JsOutput, Vec<Diagnostic>> {
        let tests = selected
            .iter()
            .map(|&i| TestEntry {
                name: program.tests[i].name.clone(),
                function: program.tests[i].function,
            })
            .collect();
        let out = compile(&program.module, &Entry::Tests(tests)).map_err(diagnostics)?;
        Ok(JsOutput {
            files: vec![OutputFile::new(WASM_ENTRY_FILE, out.module)],
            entry: PathBuf::from(WASM_ENTRY_FILE),
        })
    }

    fn runner(&self) -> Runner {
        Runner::Wasmtime
    }
}

fn diagnostics(errors: Vec<WasmError>) -> Vec<Diagnostic> {
    errors
        .into_iter()
        .map(|e| {
            let d = Diagnostic::error(e.message);
            match e.kind {
                WasmErrorKind::Callback => d.with_code(code::CALLBACK_EXTERNAL),
                // E0428 is retired: the C target now supports `suspend fn` external functions
                // (12.6). WASM still does not, but there is no code for it any more.
                WasmErrorKind::SuspendExternal
                | WasmErrorKind::Unsupported
                | WasmErrorKind::Internal => d,
            }
        })
        .collect()
}
