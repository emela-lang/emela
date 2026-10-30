//! Explicit inputs to C generation. No environment or platform discovery.

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
    entry: String,
    export_aliases: Vec<(String, String)>,
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
            entry: "main".into(),
            export_aliases: Vec::new(),
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

impl Options {
    pub(crate) fn const_diagnostic_bytes(&self) -> u16 {
        self.const_diagnostic_bytes
    }
    pub(crate) fn fiber_capacity(&self) -> u16 {
        self.fiber_capacity
    }
    pub(crate) fn frame_capacity(&self) -> u16 {
        self.frame_capacity
    }
    pub(crate) fn frame_bytes(&self) -> u16 {
        self.frame_bytes
    }
    pub(crate) fn task_capacity(&self) -> u16 {
        self.task_capacity
    }
    pub(crate) fn event_capacity(&self) -> u16 {
        self.event_capacity
    }
    pub(crate) fn deferred_capacity(&self) -> u16 {
        self.deferred_capacity
    }
    pub(crate) fn task_adapters(&self) -> &[(String, String)] {
        &self.task_adapters
    }
    pub(crate) fn external_adapters(&self) -> &[(String, ExternalAdapter)] {
        &self.external_adapters
    }
    pub(crate) fn isr_entries(&self) -> &[String] {
        &self.isr_entries
    }
    pub(crate) fn isr_safe_externals(&self) -> &[String] {
        &self.isr_safe_externals
    }
}

impl ExternalAdapter {
    pub(crate) fn symbol(&self) -> &str {
        &self.symbol
    }
    pub(crate) fn result(&self) -> ExternalResult {
        self.result
    }
}

impl Options {
    pub fn with_entry(mut self, name: impl Into<String>) -> Self {
        self.entry = name.into();
        self
    }
    pub fn with_export_alias(
        mut self,
        alias: impl Into<String>,
        function: impl Into<String>,
    ) -> Self {
        self.export_aliases.push((alias.into(), function.into()));
        self
    }
    pub(crate) fn entry(&self) -> &str {
        &self.entry
    }
    pub(crate) fn export_aliases(&self) -> &[(String, String)] {
        &self.export_aliases
    }
}
