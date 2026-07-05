//! Git operations backing Pome fetching (spec 0032 V1/V3/F6/R4).
//!
//! Resolution reads a Pome's versions straight from the repository its source
//! path names — there is no central registry (R4). This module shells out to
//! the system `git`: it lists version tags (`git ls-remote --tags`), resolves a
//! tag to its commit and tree, and materializes a tag into a local checkout.
//!
//! The fetch URL comes from [`source_path::git_url`], which honors
//! `EMELA_POME_REPLACE` so a source path can point at a local repository — how
//! the tests exercise resolution offline, and how a company mirror or offline
//! build would be wired (the Compilation Notes' transparent-cache idea).

use std::path::Path;
use std::process::Command;

use crate::error::{Error, Result};

use super::semver::Version;
use super::source_path;

/// Lists the semver version tags (V1: `v`-prefixed) a source path's repository
/// publishes. Non-version tags are skipped. This list is the sole source of
/// truth for what versions exist — read straight from the repository, with no
/// registry (R4). The commit each tag points at is determined when the selected
/// version is fetched (see [`fetch`]).
pub(crate) fn list_versions(source: &str) -> Result<Vec<Version>> {
    let url = source_path::git_url(source);
    let output = run(
        Command::new("git").arg("ls-remote").arg("--tags").arg(&url),
        &format!("list tags of `{source}`"),
    )?;

    let mut versions = Vec::new();
    for line in output.lines() {
        let Some((_commit, refname)) = line.split_once('\t') else {
            continue;
        };
        // Skip the peeled `^{}` entries git prints for annotated tags; the
        // un-peeled line already names the tag.
        if refname.ends_with("^{}") {
            continue;
        }
        let Some(tag) = refname.strip_prefix("refs/tags/") else {
            continue;
        };
        // Only `v`-prefixed semver tags are versions (V1); ignore anything else.
        if !tag.starts_with('v') {
            continue;
        }
        let Ok(version) = Version::parse(tag) else {
            continue;
        };
        versions.push(version);
    }
    Ok(versions)
}

/// Materializes `source` at `version` into `dest` (a fresh directory) and
/// returns the resolved commit SHA and content hash (the tree object id, F6).
///
/// Uses a shallow single-tag clone so only the needed revision is fetched.
pub(crate) fn fetch(source: &str, version: &Version, dest: &Path) -> Result<Fetched> {
    let url = source_path::git_url(source);
    let tag = version.to_string();

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| Error::new(format!("failed to create `{}`: {err}", parent.display())))?;
    }
    // A stale directory from an interrupted fetch would make clone fail; clear
    // it first so `install`/`update` are idempotent.
    if dest.exists() {
        std::fs::remove_dir_all(dest)
            .map_err(|err| Error::new(format!("failed to clear `{}`: {err}", dest.display())))?;
    }

    run(
        Command::new("git")
            .arg("clone")
            .arg("--quiet")
            .arg("--depth")
            .arg("1")
            .arg("--branch")
            .arg(&tag)
            .arg(&url)
            .arg(dest),
        &format!("fetch `{source}` at {tag}"),
    )?;

    let commit = rev_parse(dest, "HEAD")?;
    let tree = rev_parse(dest, "HEAD^{tree}")?;
    Ok(Fetched {
        commit,
        hash: format!("tree:{tree}"),
    })
}

/// The result of [`fetch`]: the concrete commit and its content hash.
#[derive(Debug, Clone)]
pub(crate) struct Fetched {
    pub(crate) commit: String,
    pub(crate) hash: String,
}

/// Resolves a revision to a full object id inside a checkout.
fn rev_parse(dir: &Path, rev: &str) -> Result<String> {
    let output = run(
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .arg("rev-parse")
            .arg(rev),
        &format!("resolve `{rev}`"),
    )?;
    Ok(output.trim().to_string())
}

/// Runs a git command, returning its stdout on success and a descriptive error
/// (including git's stderr) otherwise. A missing `git` binary is reported
/// clearly rather than as an opaque OS error.
fn run(command: &mut Command, action: &str) -> Result<String> {
    let output = command.output().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            Error::new("`git` was not found on PATH; it is required to fetch Pomes")
        } else {
            Error::new(format!("failed to {action}: {err}"))
        }
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::new(format!("failed to {action}: {}", stderr.trim())));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
