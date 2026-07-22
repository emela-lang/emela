//! The pluggable code-generation interface.
//!
//! A [`Backend`] turns an [`crate::IrProgram`] into an [`Artifact`]. Built-in
//! backends implement this trait directly; external-process plugins are wrapped
//! behind it as well (see [`crate::plugin`]).

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::ir::IrProgram;

/// Support level of a backend, mirroring Rust's target tiers.
///
/// `Tier1` backends are built and run in CI; `Tier2` are built and smoke
/// tested; `Tier3` are best-effort. The tier is metadata: it does not gate
/// compilation, but the CLI surfaces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tier {
    Tier1,
    Tier2,
    Tier3,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self {
            Tier::Tier1 => "Tier 1",
            Tier::Tier2 => "Tier 2",
            Tier::Tier3 => "Tier 3",
        }
    }
}

/// The kind of bytes an [`Artifact`] holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactKind {
    JsSource,
    WasmBinary,
    WasmText,
    /// A WebAssembly component binary (WASI 0.2 / spec 0052).
    WasmComponent,
    Other(String),
}

impl ArtifactKind {
    /// Whether the bytes are human-readable text (vs. a binary blob).
    pub fn is_text(&self) -> bool {
        matches!(
            self,
            ArtifactKind::JsSource | ArtifactKind::WasmText | ArtifactKind::Other(_)
        )
    }
}

/// The output of a backend.
#[derive(Debug, Clone)]
pub struct Artifact {
    pub kind: ArtifactKind,
    pub bytes: Vec<u8>,
}

impl Artifact {
    pub fn text(kind: ArtifactKind, text: String) -> Self {
        Self {
            kind,
            bytes: text.into_bytes(),
        }
    }
}

/// How a backend should emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EmitMode {
    /// The backend's primary artifact (e.g. a wasm binary, JS source).
    #[default]
    Default,
    /// A textual form of the artifact, when one exists (e.g. WAT).
    Text,
}

/// Options passed to a backend at compile time.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BackendOptions {
    pub mode: EmitMode,
    pub target: Option<String>,
    pub runtime: Option<String>,
    /// The platform registry (standard + host externs) for capability manifest
    /// generation (spec 0025). Backends use this to map platform function names
    /// to their capabilities.
    ///
    /// `PlatformFn` does not implement `Serialize`/`Deserialize`, so this field
    /// is skipped. External (process) backends use `PluginRequest` instead,
    /// which omits the registry; only in-process backends (WASM) consume it
    /// directly from `BackendOptions`.
    #[serde(skip)]
    pub platform_registry: Vec<crate::PlatformFn>,
}

/// A code-generation target.
pub trait Backend {
    fn name(&self) -> &str;
    fn tier(&self) -> Tier;
    fn compile(&self, ir: &IrProgram, options: &BackendOptions) -> Result<Artifact>;
}
