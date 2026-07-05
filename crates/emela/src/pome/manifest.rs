//! The Pome manifest, `Pome.toml` (spec 0032 F1-F4).
//!
//! The manifest is the *source-side declaration* of a Pome: its identity
//! (`[pome]` with `name`/`version`/`emela`, F2) and what it depends on
//! (`[dependencies]`, F3). Dependency keys are always canonical source paths
//! (S3). It is distinct from the capability manifest a build embeds (0025); this
//! file says what the Pome depends on, not what the artifact requires (F4).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

use super::semver::{Requirement, Version};
use super::source_path;
use super::toml_lite;

/// The file name a Pome's manifest always uses (spec 0032 F1).
pub(crate) const FILE_NAME: &str = "Pome.toml";

/// A parsed `Pome.toml`.
#[derive(Debug, Clone)]
pub(crate) struct Manifest {
    /// Canonical source path of this Pome (`[pome].name`, F2).
    pub(crate) name: String,
    /// This Pome's own version (`[pome].version`, F2).
    pub(crate) version: Version,
    /// The language spec version this Pome targets (`[pome].emela`, F2). Kept as
    /// a raw string; the compatibility rule is an Open Question in 0032.
    pub(crate) emela: String,
    /// Dependencies keyed by canonical source path (F3), ordered for
    /// deterministic output.
    pub(crate) dependencies: BTreeMap<String, Requirement>,
}

impl Manifest {
    /// A fresh manifest for a newly scaffolded Pome (`emela new`, spec 0032 C2).
    pub(crate) fn new(name: String, emela: String) -> Self {
        Manifest {
            name,
            version: Version {
                major: 0,
                minor: 1,
                patch: 0,
                pre: Vec::new(),
            },
            emela,
            dependencies: BTreeMap::new(),
        }
    }

    /// Reads and validates the `Pome.toml` in `dir`.
    pub(crate) fn load(dir: &Path) -> Result<Self> {
        let path = dir.join(FILE_NAME);
        let source = std::fs::read_to_string(&path)
            .map_err(|err| Error::new(format!("failed to read `{}`: {err}", path.display())))?;
        Self::parse(&source, &path)
    }

    /// Parses manifest text. `origin` is used only for diagnostics.
    pub(crate) fn parse(source: &str, origin: &Path) -> Result<Self> {
        let doc = toml_lite::parse(source)
            .map_err(|err| Error::new(format!("failed to parse `{}`: {err}", origin.display())))?;
        let pome = doc.table("pome").ok_or_else(|| {
            Error::new(format!(
                "`{}` is missing the required `[pome]` table",
                origin.display()
            ))
        })?;
        // `pome.name` SHOULD be a canonical source path (S1) for anything
        // published, but a freshly scaffolded entry Pome has no host yet — the
        // spec's own example runs `emela new hello` and lists it as `hello`. So
        // the name is kept as written; only *dependency* keys (which are
        // fetched) are normalized to canonical source paths (S3).
        let name = required(pome.get_string("name"), "pome.name", origin)?.to_string();
        let version = Version::parse(required(
            pome.get_string("version"),
            "pome.version",
            origin,
        )?)?;
        let emela = required(pome.get_string("emela"), "pome.emela", origin)?.to_string();

        let mut dependencies = BTreeMap::new();
        if let Some(deps) = doc.table("dependencies") {
            for (path, req) in deps.string_entries() {
                let canonical = source_path::normalize(path)?;
                let requirement = Requirement::parse(req)
                    .map_err(|err| Error::new(format!("dependency `{path}`: {err}")))?;
                if dependencies
                    .insert(canonical.clone(), requirement)
                    .is_some()
                {
                    return Err(Error::new(format!(
                        "duplicate dependency `{canonical}` in `{}`",
                        origin.display()
                    )));
                }
            }
        }

        Ok(Manifest {
            name,
            version,
            emela,
            dependencies,
        })
    }

    /// Serializes to canonical `Pome.toml` text. Deterministic: dependencies are
    /// emitted in sorted source-path order (BTreeMap).
    pub(crate) fn to_toml(&self) -> String {
        let mut out = String::new();
        out.push_str("[pome]\n");
        out.push_str(&format!("name = {}\n", toml_lite::quote(&self.name)));
        out.push_str(&format!(
            "version = {}\n",
            toml_lite::quote(self.version.to_string().trim_start_matches('v'))
        ));
        out.push_str(&format!("emela = {}\n", toml_lite::quote(&self.emela)));

        if !self.dependencies.is_empty() {
            out.push_str("\n[dependencies]\n");
            for (path, req) in &self.dependencies {
                out.push_str(&format!(
                    "{} = {}\n",
                    toml_lite::quote(path),
                    toml_lite::quote(&req.to_toml())
                ));
            }
        }
        out
    }

    /// Writes the manifest into `dir`.
    pub(crate) fn save(&self, dir: &Path) -> Result<()> {
        let path = dir.join(FILE_NAME);
        std::fs::write(&path, self.to_toml())
            .map_err(|err| Error::new(format!("failed to write `{}`: {err}", path.display())))
    }
}

/// The path of the `Pome.toml` that governs `dir`, if one exists there.
pub(crate) fn manifest_path(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

fn required<'a>(value: Option<&'a str>, key: &str, origin: &Path) -> Result<&'a str> {
    value.ok_or_else(|| {
        Error::new(format!(
            "`{}` is missing required key `{key}`",
            origin.display()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_spec_example() {
        // Verbatim from spec 0032's Examples section.
        let source = r#"
[pome]
name = "github.com/emela-lang/json"
version = "1.2.0"
emela = "0.1"

[dependencies]
"github.com/emela-lang/parser" = "^2.0"
"gitlab.com/acme/util"         = "^0.3"
"#;
        let manifest = Manifest::parse(source, Path::new("Pome.toml")).unwrap();
        assert_eq!(manifest.name, "github.com/emela-lang/json");
        assert_eq!(manifest.version.to_string(), "v1.2.0");
        assert_eq!(manifest.emela, "0.1");
        assert_eq!(manifest.dependencies.len(), 2);
        assert!(
            manifest
                .dependencies
                .contains_key("github.com/emela-lang/parser")
        );
    }

    #[test]
    fn round_trips() {
        let source = r#"
[pome]
name = "github.com/acme/app"
version = "0.1.0"
emela = "0.1"

[dependencies]
"github.com/emela-lang/stdlib" = "^1.4.0"
"#;
        let manifest = Manifest::parse(source, Path::new("Pome.toml")).unwrap();
        let reparsed = Manifest::parse(&manifest.to_toml(), Path::new("Pome.toml")).unwrap();
        assert_eq!(manifest.name, reparsed.name);
        assert_eq!(manifest.dependencies, reparsed.dependencies);
    }

    #[test]
    fn dependency_keys_are_canonicalized() {
        // A shorthand dependency key normalizes to a canonical source path (S3),
        // while the entry Pome's own `name` is kept verbatim.
        let source = r#"
[pome]
name = "hello"
version = "0.1.0"
emela = "0.1"

[dependencies]
"github:emela-lang/stdlib" = "^1.0"
"#;
        let manifest = Manifest::parse(source, Path::new("Pome.toml")).unwrap();
        assert_eq!(manifest.name, "hello");
        assert!(
            manifest
                .dependencies
                .contains_key("github.com/emela-lang/stdlib")
        );
    }

    #[test]
    fn missing_pome_table_is_rejected() {
        let err = Manifest::parse("[dependencies]\n", Path::new("Pome.toml")).unwrap_err();
        assert!(err.to_string().contains("[pome]"));
    }
}
