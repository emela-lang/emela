//! Capability manifest (spec 0025).
//!
//! The manifest is a JSON document that records the program's transitive
//! requirements: platform functions, capabilities, and intrinsics. It is
//! embedded in the WASM binary as a custom section `emela:capabilities`.

use serde::Serialize;
use std::collections::BTreeSet;

use crate::ir::IrProgram;
use crate::ir_walk::{used_intrinsics, used_platform_fns};
use crate::platform::PlatformFn;

/// The capability manifest (spec 0025).
#[derive(Debug, Clone, Serialize)]
pub struct CapabilityManifest {
    /// Manifest format version.
    pub format: u32,
    /// Platform function canonical names (sorted, deduplicated).
    pub requires: Vec<String>,
    /// Capability identifiers (sorted, deduplicated, lowercase).
    pub capabilities: Vec<String>,
    /// Intrinsic names (sorted, deduplicated).
    pub intrinsics: Vec<String>,
    /// Whether the program has a `main` entry point.
    pub entry: bool,
}

/// Computes the capability manifest for a program.
pub fn compute_manifest(
    program: &IrProgram,
    platform_registry: &[PlatformFn],
) -> CapabilityManifest {
    let used_platform = used_platform_fns(program);
    let used_intrinsics = used_intrinsics(program);
    let has_main = program.functions.iter().any(|f| f.name == "main");

    let mut requires = BTreeSet::new();
    let mut capabilities = BTreeSet::new();

    for name in &used_platform {
        requires.insert(name.clone());
        if let Some(entry) = platform_registry.iter().find(|e| e.canonical() == *name) {
            capabilities.insert(entry.capability.to_lowercase());
        }
    }

    let intrinsics: Vec<String> = {
        let mut set = BTreeSet::new();
        for name in &used_intrinsics {
            set.insert(name.clone());
        }
        set.into_iter().collect()
    };

    CapabilityManifest {
        format: 1,
        requires: requires.into_iter().collect(),
        capabilities: capabilities.into_iter().collect(),
        intrinsics,
        entry: has_main,
    }
}

/// Serializes the manifest to compact JSON (deterministic encoding).
pub fn serialize_manifest(manifest: &CapabilityManifest) -> String {
    serde_json::to_string(manifest).expect("manifest serialization should not fail")
}
