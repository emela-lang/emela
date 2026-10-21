//! Embedded C backend foundations.
//!
//! This crate verifies Flow IR and emits C with fixed-capacity memory and feature-specific
//! runtimes without running Heap IR passes. A convenience entry also lowers ordinary Core IR.

pub mod boards;
mod const_eval;
mod const_origins;
mod const_runtime;
mod derived;
pub use derived::{DerivedOperation, derived_binding};
mod discarded;
mod emit;
mod fiber;
mod float_parse;
mod float_to_string;
mod function_coerce;
mod heap;
mod scope_abi;
mod static_closure;
mod static_handler;
mod type_expansion;
mod type_normalize;
mod unicode_grapheme;
mod writer;

/// Ownership contract for a synchronous board adapter's result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalResult {
    /// Acquire a complete owning copy before returning to Emela. The board keeps the result
    /// alive until that copy completes; returning pointers into the C callee's stack is invalid.
    Borrowed,
    /// Transfer a complete Emela owning value, including every managed field, to the caller.
    /// Owners must belong to this generated module's fixed heap, never malloc or a C stack.
    Owned,
    /// Return an ownerless String pointing at the additional caller-provided output buffer.
    /// The board must check its byte capacity before writing; valid only for String results.
    /// For verified Flow failures, the error is acquired using the Borrowed contract.
    StringBuffer,
}

/// Explicit synchronous C adapter using the generated module's C value representations.
/// All arguments are borrowed until the call returns. Retaining them requires acquiring
/// independent ownership while they are still valid; ownerless String bytes need copying.
#[derive(Debug, Clone)]
pub struct ExternalAdapter {
    symbol: String,
    result: ExternalResult,
}

impl ExternalAdapter {
    /// Selects a board symbol and its result ownership or caller-output-buffer contract.
    pub fn new(symbol: impl Into<String>, result: ExternalResult) -> Self {
        Self {
            symbol: symbol.into(),
            result,
        }
    }
}

/// Configuration for embedded C generation.
///
/// Configuration for board-independent generation and fixed embedded resource limits.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Options {
    string_capacity: u16,
    peripheral_buffer_capacity: u16,
    heap_bytes: u32,
    const_diagnostic_bytes: u16,
    fiber_capacity: u16,
    frame_capacity: u16,
    frame_bytes: u16,
    task_capacity: u16,
    event_capacity: u16,
    deferred_capacity: u16,
    task_adapters: Vec<(String, String)>,
    external_adapters: Vec<(String, ExternalAdapter)>,
    isr_entries: Vec<String>,
    isr_safe_externals: Vec<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            string_capacity: 256,
            peripheral_buffer_capacity: 256,
            heap_bytes: 0,
            const_diagnostic_bytes: 1024,
            fiber_capacity: 8,
            frame_capacity: 32,
            frame_bytes: 256,
            task_capacity: 8,
            event_capacity: 8,
            deferred_capacity: 8,
            task_adapters: Vec::new(),
            external_adapters: Vec::new(),
            isr_entries: Vec::new(),
            isr_safe_externals: Vec::new(),
        }
    }
}

impl Options {
    /// Sets the maximum number of uint16_t words in a synchronous peripheral transfer.
    /// Zero is invalid. The default is 256 words; buffers are allocated on the C call stack.
    pub fn with_peripheral_buffer_capacity(mut self, words: u16) -> Self {
        self.peripheral_buffer_capacity = words;
        self
    }

    /// Returns the configured transfer capacity in uint16_t words.
    pub fn peripheral_buffer_capacity(&self) -> u16 {
        self.peripheral_buffer_capacity
    }

    /// Sets the maximum size in bytes for a dynamically generated String buffer.
    ///
    /// Values from 1 through 512 are accepted. The generated code reports a String capacity
    /// defect if an operation exceeds this limit.
    pub fn with_string_capacity(mut self, bytes: u16) -> Self {
        self.string_capacity = bytes;
        self
    }

    /// Returns the maximum configured size in bytes for a dynamically generated String buffer.
    pub fn string_capacity(&self) -> u16 {
        self.string_capacity
    }

    /// Sets the total byte capacity reserved for the generated fixed heap.
    ///
    /// A capacity of zero disables heap-backed values. The default is zero.
    pub fn with_heap_bytes(mut self, bytes: u32) -> Self {
        self.heap_bytes = bytes;
        self
    }

    /// Returns the configured byte capacity for the generated fixed heap.
    pub fn heap_bytes(&self) -> u32 {
        self.heap_bytes
    }

    /// Sets the module-wide storage for memoized runtime const defect messages (default 1024).
    /// Storage is generated only for runtime-backed consts. Exhaustion reports a resource defect
    /// through a separate bounded emergency buffer without allocating from the fixed heap.
    pub fn with_const_diagnostic_bytes(mut self, bytes: u16) -> Self {
        self.const_diagnostic_bytes = bytes;
        self
    }

    /// Sets the maximum number of active or completed, unreleased fiber handles (default 8).
    /// Storage is emitted only when a reachable function suspends.
    pub fn with_fiber_capacity(mut self, count: u16) -> Self {
        self.fiber_capacity = count;
        self
    }

    /// Sets the maximum number of simultaneously live suspension frames (default 32).
    pub fn with_frame_capacity(mut self, count: u16) -> Self {
        self.frame_capacity = count;
        self
    }

    /// Sets the byte capacity of each suspension frame, including its header (default 256).
    /// Generated C checks each typed frame against this target-dependent limit.
    pub fn with_frame_bytes(mut self, bytes: u16) -> Self {
        self.frame_bytes = bytes;
        self
    }

    /// Sets the maximum number of pending external tasks (default 8).
    pub fn with_task_capacity(mut self, count: u16) -> Self {
        self.task_capacity = count;
        self
    }

    /// Sets the maximum number of queued completion events (default 8).
    pub fn with_event_capacity(mut self, count: u16) -> Self {
        self.event_capacity = count;
        self
    }

    /// Sets the maximum number of live Deferred values (default 8).
    /// Storage is emitted only when a reachable program can use Deferred.
    pub fn with_deferred_capacity(mut self, count: u16) -> Self {
        self.deferred_capacity = count;
        self
    }

    /// Maps an external C binding to a board adapter using the generated EmelaTask ABI.
    /// An explicit mapping prevents linking a host suspend function with a different ABI.
    pub fn with_task_adapter(
        mut self,
        c_symbol: impl Into<String>,
        adapter: impl Into<String>,
    ) -> Self {
        let c_symbol = c_symbol.into();
        self.task_adapters.retain(|(symbol, _)| symbol != &c_symbol);
        self.task_adapters.push((c_symbol, adapter.into()));
        self
    }

    /// Maps a synchronous external C binding to an explicit generated-value board adapter.
    /// Existing scalar bindings need no override. Non-scalar bindings require one to avoid
    /// accidentally linking a host C backend function with an incompatible ownership ABI.
    pub fn with_external_adapter(
        mut self,
        c_symbol: impl Into<String>,
        adapter: ExternalAdapter,
    ) -> Self {
        let c_symbol = c_symbol.into();
        self.external_adapters
            .retain(|(symbol, _)| symbol != &c_symbol);
        self.external_adapters.push((c_symbol, adapter));
        self
    }

    /// Marks an exported Emela function as an interrupt-service entry point.
    pub fn with_isr_entry(mut self, function_name: impl Into<String>) -> Self {
        self.isr_entries.push(function_name.into());
        self
    }

    /// Marks a C symbol as safe to call from a configured interrupt-service entry point.
    pub fn with_isr_safe_external(mut self, symbol: impl Into<String>) -> Self {
        self.isr_safe_externals.push(symbol.into());
        self
    }
}

/// Generated files and public entry points returned by the backend.
#[derive(Debug, Clone, Default)]
pub struct Output {
    /// Files keyed by their path relative to the generated output directory.
    pub files: Vec<GeneratedFile>,
    /// Public functions with their actual callable C symbols.
    pub exports: Vec<ExportedFunction>,
}

/// A public entry point emitted into the generated header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedFunction {
    /// The function name recorded by Flow IR, before C identifier sanitization.
    pub source_name: String,
    /// The callable C symbol; suspension entries include the `_start` suffix.
    pub c_symbol: String,
    /// Whether calling the entry starts a suspendable fiber.
    pub suspends: bool,
}

/// One file produced by the backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedFile {
    /// Path relative to the generated output directory.
    pub path: String,
    /// File contents.
    pub contents: String,
}

/// The stage that failed while compiling Flow IR to embedded C.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The Core module could not be verified after monomorphization.
    InvalidCore,
    /// Core IR could not be converted to Flow IR.
    FlowLowering,
    /// The Flow IR module was not valid for the Flow stage.
    InvalidFlowIr,
    /// A valid Flow IR construct is not implemented by this backend yet.
    Unsupported,
    /// Backend options are outside their documented range.
    InvalidOptions,
}

/// An error encountered while preparing a module for embedded code generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

/// Converts Core IR to Flow IR and emits embedded C files.
pub fn compile(core: &emela_core::Module, options: &Options) -> Result<Output, Vec<Error>> {
    let flow = flow_from_core(core)?;
    compile_flow(&flow, options)
}

/// Verifies and emits a Flow IR module without running Heap IR passes.
pub fn compile_flow(flow: &emela_lir::Module, options: &Options) -> Result<Output, Vec<Error>> {
    if flow.stage != emela_lir::Stage::Flow {
        verify_flow(flow)?;
    }
    let mut normalized = flow.clone();
    type_normalize::normalize(&mut normalized)?;
    verify_flow(&normalized)?;
    emit::emit(&normalized, options)
}

/// Monomorphizes Core IR, lowers it to Flow IR, and verifies the Flow-stage invariants.
///
/// The backend normalizes the conservative failure edge of an infallible CauseRaise before
/// verifying Flow IR. This path does not call `emela_lir::to_heap`.
pub fn flow_from_core(core: &emela_core::Module) -> Result<emela_lir::Module, Vec<Error>> {
    derived::check_core_input(core)?;
    let prepared_consts = const_origins::monomorphize(core);
    let mono = prepared_consts.module;
    if let Err(errors) = emela_core::verify_monomorphic(&mono) {
        return Err(errors
            .into_iter()
            .map(|error| Error {
                kind: ErrorKind::InvalidCore,
                message: format!("invalid Core IR after monomorphization: {error}"),
            })
            .collect());
    }

    derived::check_core_input(&mono)?;
    let mut flow = emela_lir::from_core(mono).map_err(|errors| {
        errors
            .into_iter()
            .map(|error| Error {
                kind: ErrorKind::FlowLowering,
                message: format!("failed to lower Core IR to Flow IR: {error}"),
            })
            .collect::<Vec<_>>()
    })?;

    const_origins::attach(&mut flow, &prepared_consts.groups);
    const_origins::attach_function_groups(&mut flow, &prepared_consts.function_groups);
    fiber::normalize_infallible_cause_raise(&mut flow);
    verify_flow(&flow)?;
    Ok(flow)
}

/// Verifies that a LIR module is still in the Flow stage and satisfies its invariants.
pub fn verify_flow(flow: &emela_lir::Module) -> Result<(), Vec<Error>> {
    if flow.stage != emela_lir::Stage::Flow {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "expected Flow IR, found {:?}; embedded codegen does not run Heap IR passes",
                flow.stage
            ),
        }]);
    }

    emela_lir::verify(flow).map_err(|errors| {
        errors
            .into_iter()
            .map(|error| Error {
                kind: ErrorKind::InvalidFlowIr,
                message: format!("invalid Flow IR: {error}"),
            })
            .collect::<Vec<_>>()
    })?;

    // The shared verifier checks representations but does not reject every operation that
    // belongs to a later stage or that from_core must already have lowered into Flow control.
    let mut errors = Vec::new();
    for (_, function) in flow.functions.iter() {
        for (_, statement) in function.stmts.iter() {
            let emela_lir::Stmt::Let { value, .. } = statement else {
                continue;
            };
            let reason = match value {
                emela_lir::Value::Runtime {
                    op:
                        op @ (emela_lir::RuntimeOp::Defect
                        | emela_lir::RuntimeOp::DefectTake
                        | emela_lir::RuntimeOp::DefectResume
                        | emela_lir::RuntimeOp::Yield
                        | emela_lir::RuntimeOp::YieldPerform),
                    ..
                } => Some(format!("Heap IR runtime `{}` at stage Flow", op.name())),
                emela_lir::Value::Builtin {
                    op:
                        op @ (emela_core::Builtin::Panic
                        | emela_core::Builtin::Todo
                        | emela_core::Builtin::Dbg
                        | emela_core::Builtin::Inspect
                        | emela_core::Builtin::ShowNested
                        | emela_core::Builtin::ShowString
                        | emela_core::Builtin::DebugPrint),
                    ..
                } => Some(format!(
                    "unlowered builtin `{}` at stage Flow",
                    op.info().path()
                )),
                _ => None,
            };
            if let Some(reason) = reason {
                errors.push(Error {
                    kind: ErrorKind::InvalidFlowIr,
                    message: format!("invalid Flow IR in function `{}`: {reason}", function.name),
                });
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}
