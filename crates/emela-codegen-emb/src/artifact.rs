//! Versioned contract between C generation and a C/BSP build.
//!
//! Headers define C layouts, signatures, and ownership. This manifest only identifies
//! build inputs and interface roles; it does not duplicate declarations from the headers.

use std::collections::HashSet;

use crate::{GeneratedFile, Output};

pub use crate::contract::artifact::{
    ABI_VERSION, ALIASES_HEADER, ArtifactFile, FileRole, MANIFEST_FILE, Manifest, Metadata,
    PLATFORM_HEADER, PUBLIC_HEADER, SCHEMA_VERSION,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactError(String);

impl std::fmt::Display for ArtifactError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid generated artifact: {}", self.0)
    }
}

impl std::error::Error for ArtifactError {}

impl Manifest {
    /// Describes an in-memory output, preserving requirements from its previous manifest if present.
    pub fn from_files(files: &[GeneratedFile]) -> Result<Self, ArtifactError> {
        let mut entries = Vec::new();
        let mut manifests = 0;
        for file in files {
            let role = match file.path.as_str() {
                MANIFEST_FILE => {
                    manifests += 1;
                    continue;
                }
                PUBLIC_HEADER => FileRole::PublicHeader,
                PLATFORM_HEADER => FileRole::PlatformHeader,
                ALIASES_HEADER => FileRole::AliasesHeader,
                path if path.ends_with(".c") => FileRole::Source,
                path if path.ends_with(".h") => FileRole::Header,
                path => {
                    return Err(ArtifactError(format!(
                        "unsupported generated file `{path}`"
                    )));
                }
            };
            entries.push(ArtifactFile {
                path: file.path.clone(),
                role,
            });
        }
        if manifests > 1 {
            return Err(ArtifactError("duplicate manifest".into()));
        }
        let manifest = Self {
            schema_version: SCHEMA_VERSION,
            abi_version: ABI_VERSION,
            files: entries,
            metadata: files
                .iter()
                .find(|file| file.path == MANIFEST_FILE)
                .map(|file| Self::from_json(&file.contents).map(|manifest| manifest.metadata))
                .transpose()?
                .unwrap_or_default(),
        };
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn from_json(json: &str) -> Result<Self, ArtifactError> {
        let manifest: Self =
            serde_json::from_str(json).map_err(|error| ArtifactError(error.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn to_json(&self) -> Result<String, ArtifactError> {
        self.validate()?;
        serde_json::to_string_pretty(self)
            .map(|json| json + "\n")
            .map_err(|error| ArtifactError(error.to_string()))
    }

    pub fn validate(&self) -> Result<(), ArtifactError> {
        if self.schema_version != SCHEMA_VERSION || self.abi_version != ABI_VERSION {
            return Err(ArtifactError(format!(
                "unsupported schema/ABI version {}/{} (expected {SCHEMA_VERSION}/{ABI_VERSION})",
                self.schema_version, self.abi_version
            )));
        }
        for names in [
            &self.metadata.services,
            &self.metadata.external_symbols,
            &self.metadata.isr_entries,
            &self.metadata.isr_safe_symbols,
        ] {
            let mut seen = HashSet::new();
            if names
                .iter()
                .any(|name| name.is_empty() || !seen.insert(name))
            {
                return Err(ArtifactError(
                    "empty or duplicate interface requirement".into(),
                ));
            }
        }
        let c_symbol = |name: &str| {
            let mut bytes = name.bytes();
            matches!(bytes.next(), Some(b'a'..=b'z' | b'A'..=b'Z' | b'_'))
                && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
        };
        if self
            .metadata
            .entry
            .as_ref()
            .is_some_and(|name| !c_symbol(name))
            || self
                .metadata
                .exports
                .iter()
                .any(|export| !c_symbol(&export.c_symbol))
        {
            return Err(ArtifactError("invalid exported C symbol".into()));
        }
        let mut paths = HashSet::new();
        let mut interfaces = HashSet::new();
        let mut sources = 0;
        for file in &self.files {
            // Use the same portable, normalized POSIX paths in Rust and Python. Reject
            // Windows paths too, even when generation is running on a Unix host.
            if file.path.contains(['\\', ':', '\0'])
                || file
                    .path
                    .split('/')
                    .any(|part| matches!(part, "" | "." | ".."))
            {
                return Err(ArtifactError(format!(
                    "invalid relative path `{}`",
                    file.path
                )));
            }
            if !paths.insert(&file.path) {
                return Err(ArtifactError(format!("duplicate path `{}`", file.path)));
            }
            let expected = match file.role {
                FileRole::Source => {
                    sources += 1;
                    if !file.path.ends_with(".c") {
                        return Err(ArtifactError("source must have the .c suffix".into()));
                    }
                    continue;
                }
                FileRole::Header => {
                    if !file.path.ends_with(".h")
                        || [PUBLIC_HEADER, PLATFORM_HEADER, ALIASES_HEADER]
                            .contains(&file.path.as_str())
                    {
                        return Err(ArtifactError(format!(
                            "invalid header role for `{}`",
                            file.path
                        )));
                    }
                    continue;
                }
                FileRole::PublicHeader => PUBLIC_HEADER,
                FileRole::PlatformHeader => PLATFORM_HEADER,
                FileRole::AliasesHeader => ALIASES_HEADER,
            };
            if file.path != expected || !interfaces.insert(file.role) {
                return Err(ArtifactError(format!(
                    "invalid interface role for `{}`",
                    file.path
                )));
            }
        }
        if sources == 0
            || !interfaces.contains(&FileRole::PublicHeader)
            || !interfaces.contains(&FileRole::PlatformHeader)
        {
            return Err(ArtifactError(
                "source, public_header, and platform_header are required".into(),
            ));
        }
        Ok(())
    }
}

impl Output {
    /// Refreshes the file inventory while preserving ABI requirements. Platform files belong
    /// in a separate package, never in the frozen common output.
    pub fn refresh_manifest(&mut self) -> Result<(), ArtifactError> {
        let mut manifest = Manifest::from_files(&self.files)?;
        if let Some(previous) = self.files.iter().find(|file| file.path == MANIFEST_FILE) {
            manifest.metadata = Manifest::from_json(&previous.contents)?.metadata;
        }
        manifest.metadata.exports = self.exports.clone();
        let contents = manifest.to_json()?;
        self.files.retain(|file| file.path != MANIFEST_FILE);
        self.files.push(GeneratedFile {
            path: MANIFEST_FILE.into(),
            contents,
        });
        Ok(())
    }
}

impl From<ArtifactError> for crate::Error {
    fn from(error: ArtifactError) -> Self {
        Self {
            kind: crate::ErrorKind::InvalidOutput,
            message: error.to_string(),
        }
    }
}
