//! Persisted interface data only; headers define C signatures.
use serde::{Deserialize, Serialize};
pub const MANIFEST_FILE: &str = "emela_artifact.json";
pub const SCHEMA_VERSION: u32 = 2;
pub const ABI_VERSION: u32 = 2;
pub const PUBLIC_HEADER: &str = "emela_embedded.h";
pub const PLATFORM_HEADER: &str = "emela_board.h";
pub const ALIASES_HEADER: &str = "emela_embedded_exports.h";

/// One ordered build input. All paths are relative to the manifest directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactFile {
    pub path: String,
    pub role: FileRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileRole {
    Source,
    PublicHeader,
    PlatformHeader,
    AliasesHeader,
    Header,
}

/// C11 source files and their generated interfaces, independent of a board/toolchain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    pub abi_version: u32,
    pub files: Vec<ArtifactFile>,
    pub metadata: Metadata,
}

/// Service requirements only. C headers remain the authority for signatures/layouts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub entry: Option<String>,
    pub exports: Vec<crate::contract::ExportedFunction>,
    pub services: Vec<String>,
    pub external_symbols: Vec<String>,
    pub isr_entries: Vec<String>,
    pub isr_safe_symbols: Vec<String>,
    pub task_capacity: u16,
}
