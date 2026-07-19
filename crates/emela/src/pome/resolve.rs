//! Dependency resolution (spec 0032 V3, R4, F5-F7).
//!
//! Given a Pome's manifest, resolution walks the transitive dependency graph —
//! reading each dependency's own `Pome.toml` at the tag it selects — and pins
//! every Pome to the greatest tag satisfying all requirements on it (V3). The
//! only inputs are the repositories the source paths name (R4); no index is
//! consulted. The result is a [`Lock`], and every selected version is fetched
//! into the local cache so a build can read it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

use super::git;
use super::lock::{Lock, LockedPackage};
use super::manifest::Manifest;
use super::semver::{Requirement, Version};

/// The local cache directory Pomes are fetched into. Overridable via
/// `EMELA_POME_CACHE` (used by tests and offline/mirror setups); otherwise a
/// per-user cache. The cache is a transparent store — the truth is always the
/// upstream repository (spec 0032 R4, Compilation Notes).
pub(crate) fn cache_root() -> PathBuf {
    if let Ok(dir) = std::env::var("EMELA_POME_CACHE") {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join(".cache").join("emela").join("pome")
}

/// The cache directory a specific `source@version` is materialized into.
pub(crate) fn checkout_dir(source: &str, version: &Version) -> PathBuf {
    // The source path may contain `/`; it maps naturally to nested directories.
    cache_root().join(source).join(version.to_string())
}

/// One resolved node in the graph.
struct Resolved {
    version: Version,
    commit: String,
    hash: String,
    dependencies: Vec<String>,
}

/// Resolves `manifest`'s full dependency graph into a [`Lock`], fetching each
/// selected version into the cache. `fetch` controls whether missing checkouts
/// are downloaded (always true for `add`/`update`; `install` also fetches).
pub(crate) fn resolve(manifest: &Manifest) -> Result<Lock> {
    // Every requirement seen for a source, gathered as the graph is walked.
    // The root's dev-dependencies (spec 0040 D2) resolve in the same graph;
    // dependencies' own dev-dependencies are never read (D5) —
    // `read_dependencies` only returns a fetched Pome's `[dependencies]`.
    let mut requirements: BTreeMap<String, Vec<Requirement>> = BTreeMap::new();
    for (source, req) in manifest
        .dependencies
        .iter()
        .chain(manifest.dev_dependencies.iter())
    {
        requirements
            .entry(source.clone())
            .or_default()
            .push(req.clone());
    }

    let mut resolved: BTreeMap<String, Resolved> = BTreeMap::new();

    // Fixpoint: keep (re-)selecting sources until every source has a selection
    // that satisfies all requirements currently known for it. Discovering a
    // transitive dependency can add a new source or a new constraint, so we
    // loop until a full pass makes no change.
    loop {
        let mut changed = false;
        // Snapshot the current source set; new sources added this pass are
        // handled on the next iteration.
        let sources: Vec<String> = requirements.keys().cloned().collect();
        for source in sources {
            let reqs = requirements.get(&source).cloned().unwrap_or_default();
            let already_ok = resolved
                .get(&source)
                .map(|r| reqs.iter().all(|req| req.matches(&r.version)))
                .unwrap_or(false);
            if already_ok {
                continue;
            }

            let selection = select_version(&source, &reqs)?;
            let dir = checkout_dir(&source, &selection);
            let fetched = ensure_fetched(&source, &selection, &dir)?;
            let deps = read_dependencies(&source, &dir)?;

            // Fold this node's own dependency requirements into the graph.
            for (dep_source, dep_req) in &deps {
                requirements
                    .entry(dep_source.clone())
                    .or_default()
                    .push(dep_req.clone());
            }

            resolved.insert(
                source.clone(),
                Resolved {
                    version: selection,
                    commit: fetched.commit,
                    hash: fetched.hash,
                    dependencies: deps.keys().cloned().collect(),
                },
            );
            changed = true;
        }
        if !changed {
            break;
        }
    }

    // A package is dev-only (spec 0040 D2) when the runtime graph — the walk
    // from `[dependencies]` alone — never reaches it.
    let mut runtime: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut queue: Vec<String> = manifest.dependencies.keys().cloned().collect();
    while let Some(source) = queue.pop() {
        if !runtime.insert(source.clone()) {
            continue;
        }
        if let Some(node) = resolved.get(&source) {
            queue.extend(node.dependencies.iter().cloned());
        }
    }

    let packages = resolved
        .into_iter()
        .map(|(source, r)| LockedPackage {
            dev: !runtime.contains(&source),
            source,
            version: r.version,
            commit: r.commit,
            hash: r.hash,
            dependencies: r.dependencies,
        })
        .collect();
    Ok(Lock::from_packages(packages))
}

/// Fetches every package a lock pins into the cache, without re-resolving
/// (`emela pome install`, spec 0032 F5). Missing checkouts are downloaded at the
/// exact locked version; present ones are left alone.
pub(crate) fn install(lock: &Lock) -> Result<()> {
    for package in &lock.packages {
        let dir = checkout_dir(&package.source, &package.version);
        ensure_fetched(&package.source, &package.version, &dir)?;
    }
    Ok(())
}

/// Selects the greatest tag satisfying every requirement on `source` (V3),
/// reporting a clear error when the repository has no tags or none match.
fn select_version(source: &str, reqs: &[Requirement]) -> Result<Version> {
    let tags = git::list_versions(source)?;
    if tags.is_empty() {
        return Err(Error::new(format!(
            "`{source}` has no `v`-prefixed version tags to resolve against"
        )));
    }
    let mut best: Option<&Version> = None;
    for version in &tags {
        if reqs.iter().all(|req| req.matches(version)) && best.map(|b| version > b).unwrap_or(true)
        {
            best = Some(version);
        }
    }
    best.cloned().ok_or_else(|| {
        let available = tags
            .iter()
            .map(Version::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let wanted = reqs
            .iter()
            .map(Requirement::to_toml)
            .collect::<Vec<_>>()
            .join(", ");
        Error::new(format!(
            "no version of `{source}` satisfies {wanted} (available: {available})"
        ))
    })
}

/// Ensures `source@version` is present in `dir`, fetching if absent. Returns the
/// commit and content hash either way.
fn ensure_fetched(source: &str, version: &Version, dir: &Path) -> Result<git::Fetched> {
    // A present checkout with a resolvable HEAD is reused; otherwise (re)fetch.
    if dir.join(".git").exists()
        && let Ok(fetched) = read_fetched(dir)
    {
        return Ok(fetched);
    }
    git::fetch(source, version, dir)
}

/// Reads the commit/hash of an already-present checkout, so `install` need not
/// re-clone what the cache already holds.
fn read_fetched(dir: &Path) -> Result<git::Fetched> {
    let commit = git_output(dir, &["rev-parse", "HEAD"])?;
    let tree = git_output(dir, &["rev-parse", "HEAD^{tree}"])?;
    Ok(git::Fetched {
        commit,
        hash: format!("tree:{tree}"),
    })
}

fn git_output(dir: &Path, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|err| {
            Error::new(format!(
                "failed to inspect cache `{}`: {err}",
                dir.display()
            ))
        })?;
    if !output.status.success() {
        return Err(Error::new(format!(
            "cache checkout `{}` is unreadable",
            dir.display()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Reads a fetched Pome's own dependencies from its `Pome.toml`. A dependency
/// Pome without a manifest is treated as having no dependencies (a library Pome
/// may omit its lock, but a manifest is required if it declares dependencies).
fn read_dependencies(source: &str, dir: &Path) -> Result<BTreeMap<String, Requirement>> {
    let manifest_path = super::manifest::manifest_path(dir);
    if !manifest_path.exists() {
        return Ok(BTreeMap::new());
    }
    let manifest = Manifest::load(dir)
        .map_err(|err| Error::new(format!("while reading dependencies of `{source}`: {err}")))?;
    Ok(manifest.dependencies)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkout_dir_nests_source_and_version() {
        // The tail is `<source-path>/<version>` regardless of the cache root, so
        // this needs no env mutation (unsafe in a multithreaded test binary).
        let dir = checkout_dir("github.com/acme/util", &Version::parse("v1.2.0").unwrap());
        assert!(dir.ends_with("github.com/acme/util/v1.2.0"));
    }
}
