//! Owned connection data. No compiler or process implementation is stored here.
use crate::GeneratedFile;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformPackage {
    pub id: String,
    pub abi_version: u32,
    pub services: BTreeSet<String>,
    pub external_symbols: BTreeSet<String>,
    pub isr_safe_symbols: BTreeSet<String>,
    pub supports_isr: bool,
    pub files: Vec<PackageFile>,
    /// Paths of C/assembly sources relative to this package.
    pub sources: Vec<String>,
    /// Explicit compiler and linker arguments, relative to the build directory.
    pub compile_flags: Vec<String>,
    pub link_flags: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageFile {
    pub path: String,
    pub contents: Vec<u8>,
}
impl From<GeneratedFile> for PackageFile {
    fn from(file: GeneratedFile) -> Self {
        Self {
            path: file.path,
            contents: file.contents.into_bytes(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolchainInfo {
    pub compiler: PathBuf,
    pub identity: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectOptions {
    pub optimization: Optimization,
    pub executable: String,
    pub main: bool,
}
impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            optimization: Optimization::O2,
            executable: "program".into(),
            main: true,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Optimization {
    O0,
    O2,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandPlan {
    pub program: PathBuf,
    pub arguments: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildPlan {
    pub files: Vec<PackageFile>,
    pub commands: Vec<CommandPlan>,
    pub executable: String,
    pub common_hash: String,
    pub platform_hash: String,
    pub toolchain: ToolchainInfo,
}
