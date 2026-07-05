//! The lock file, `Pome.lock` (spec 0032 F5-F8).
//!
//! Resolution pins each dependency to a concrete tag and the commit it points
//! at, plus a content hash for integrity checking (F6). The encoding is
//! deterministic — packages are emitted sorted by source path — so the same
//! inputs always produce byte-identical output (F7). It is generated, not
//! hand-edited (F8).

use std::path::Path;

use crate::error::{Error, Result};

use super::semver::Version;
use super::toml_lite;

/// The file name a lock always uses (spec 0032 F5).
pub(crate) const FILE_NAME: &str = "Pome.lock";

/// The current lock format version, bumped if the schema changes.
const VERSION: u32 = 1;

/// One resolved dependency (spec 0032 F6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LockedPackage {
    /// Canonical source path (S3).
    pub(crate) source: String,
    /// The selected tag (V3).
    pub(crate) version: Version,
    /// The commit SHA the tag points at (V3).
    pub(crate) commit: String,
    /// Content hash of the fetched tree, for integrity verification (F6). Here
    /// the Git tree object id (`tree:<sha>`), which is content-addressed and
    /// deterministic without extra crypto dependencies.
    pub(crate) hash: String,
    /// This package's own direct dependencies, by source path. Lets `list`
    /// render the resolved tree without re-reading every manifest.
    pub(crate) dependencies: Vec<String>,
}

/// A parsed / resolved `Pome.lock`.
#[derive(Debug, Clone, Default)]
pub(crate) struct Lock {
    pub(crate) packages: Vec<LockedPackage>,
}

impl Lock {
    /// Builds a lock from resolved packages, normalizing to canonical order so
    /// the encoding is deterministic (F7).
    pub(crate) fn from_packages(mut packages: Vec<LockedPackage>) -> Self {
        packages.sort_by(|a, b| a.source.cmp(&b.source));
        packages.dedup_by(|a, b| a.source == b.source);
        Lock { packages }
    }

    pub(crate) fn find(&self, source: &str) -> Option<&LockedPackage> {
        self.packages.iter().find(|pkg| pkg.source == source)
    }

    /// Reads `Pome.lock` from `dir`, if present. Returns an empty lock when the
    /// file does not exist (a Pome may have no dependencies yet).
    pub(crate) fn load(dir: &Path) -> Result<Self> {
        let path = dir.join(FILE_NAME);
        let source = match std::fs::read_to_string(&path) {
            Ok(source) => source,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Lock::default());
            }
            Err(err) => {
                return Err(Error::new(format!(
                    "failed to read `{}`: {err}",
                    path.display()
                )));
            }
        };
        Self::parse(&source, &path)
    }

    fn parse(source: &str, origin: &Path) -> Result<Self> {
        let doc = toml_lite::parse(source)
            .map_err(|err| Error::new(format!("failed to parse `{}`: {err}", origin.display())))?;
        // Guard against a lock written by an incompatible future encoder (F7/F8).
        if let Some(found) = doc.root().get_string("version")
            && found != VERSION.to_string()
        {
            return Err(Error::new(format!(
                "`{}` uses lock format version {found}, but this tool writes version {VERSION}; \
                 regenerate it with `emela pome update`",
                origin.display()
            )));
        }
        let mut packages = Vec::new();
        for table in doc.array_of_tables("package") {
            let source_path = table.get_string("source").ok_or_else(|| {
                Error::new(format!(
                    "`{}`: a [[package]] is missing `source`",
                    origin.display()
                ))
            })?;
            let version = Version::parse(table.get_string("version").ok_or_else(|| {
                Error::new(format!(
                    "`{}`: package `{source_path}` is missing `version`",
                    origin.display()
                ))
            })?)?;
            let commit = table
                .get_string("commit")
                .ok_or_else(|| {
                    Error::new(format!(
                        "`{}`: package `{source_path}` is missing `commit`",
                        origin.display()
                    ))
                })?
                .to_string();
            let hash = table.get_string("hash").unwrap_or_default().to_string();
            let dependencies = table
                .get_array("dependencies")
                .map(|deps| deps.to_vec())
                .unwrap_or_default();
            packages.push(LockedPackage {
                source: source_path.to_string(),
                version,
                commit,
                hash,
                dependencies,
            });
        }
        Ok(Lock::from_packages(packages))
    }

    /// Serializes to canonical, deterministic `Pome.lock` text (F7).
    pub(crate) fn to_toml(&self) -> String {
        let mut out = String::new();
        out.push_str("# This file is generated by `emela pome`; do not edit by hand.\n");
        out.push_str(&format!("version = {VERSION}\n"));
        for package in &self.packages {
            out.push_str("\n[[package]]\n");
            out.push_str(&format!("source = {}\n", toml_lite::quote(&package.source)));
            out.push_str(&format!(
                "version = {}\n",
                toml_lite::quote(&package.version.to_string())
            ));
            out.push_str(&format!("commit = {}\n", toml_lite::quote(&package.commit)));
            out.push_str(&format!("hash = {}\n", toml_lite::quote(&package.hash)));
            if !package.dependencies.is_empty() {
                let mut deps = package.dependencies.clone();
                deps.sort();
                let rendered = deps
                    .iter()
                    .map(|dep| toml_lite::quote(dep))
                    .collect::<Vec<_>>()
                    .join(", ");
                out.push_str(&format!("dependencies = [{rendered}]\n"));
            }
        }
        out
    }

    pub(crate) fn save(&self, dir: &Path) -> Result<()> {
        let path = dir.join(FILE_NAME);
        std::fs::write(&path, self.to_toml())
            .map_err(|err| Error::new(format!("failed to write `{}`: {err}", path.display())))
    }

    /// Removes `Pome.lock` from `dir` if it exists. Used when the last
    /// dependency is removed so no stale lock is left behind.
    pub(crate) fn remove_file(dir: &Path) -> Result<()> {
        let path = dir.join(FILE_NAME);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(Error::new(format!(
                "failed to remove `{}`: {err}",
                path.display()
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Lock {
        Lock::from_packages(vec![
            LockedPackage {
                source: "gitlab.com/acme/util".to_string(),
                version: Version::parse("v0.3.1").unwrap(),
                commit: "deadbeef".to_string(),
                hash: "tree:1111".to_string(),
                dependencies: vec![],
            },
            LockedPackage {
                source: "github.com/emela-lang/stdlib".to_string(),
                version: Version::parse("v1.4.0").unwrap(),
                commit: "a1b2c3d".to_string(),
                hash: "tree:2222".to_string(),
                dependencies: vec!["gitlab.com/acme/util".to_string()],
            },
        ])
    }

    #[test]
    fn encoding_is_deterministic_and_sorted() {
        // Spec 0032 F7: sorted by source path regardless of input order.
        let lock = sample();
        let text = lock.to_toml();
        let first = text.find("gitlab.com/acme/util").unwrap();
        let second = text.find("github.com/emela-lang/stdlib").unwrap();
        // `github.com` < `gitlab.com` lexically, so it must come first.
        assert!(second < first, "packages must be sorted by source path");
        // Same lock re-encodes identically.
        assert_eq!(text, sample().to_toml());
    }

    #[test]
    fn round_trips() {
        let lock = sample();
        let text = lock.to_toml();
        let reparsed = Lock::parse(&text, Path::new("Pome.lock")).unwrap();
        assert_eq!(lock.packages, reparsed.packages);
    }

    #[test]
    fn missing_file_is_an_empty_lock() {
        let dir = std::env::temp_dir().join(format!("emela-lock-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let lock = Lock::load(&dir).unwrap();
        assert!(lock.packages.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
