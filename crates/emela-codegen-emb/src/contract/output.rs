//! Generated artifact data shared with independent consumers.

/// Generated C, headers, their artifact manifest, and public entry points.
#[derive(Debug, Clone, Default)]
pub struct Output {
    /// Files keyed by their path relative to the generated output directory.
    pub files: Vec<GeneratedFile>,
    /// Public functions with their actual callable C symbols.
    pub exports: Vec<ExportedFunction>,
}

/// A public entry point emitted into the generated header.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
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
