//! Packaging: Pomes and dependency management (spec 0032).
//!
//! A **Pome** is Emela's unit of distribution and dependency — one or more
//! modules (0010) supplied as a Git repository, versioned by tag, and identified
//! by its **source path** (`host/path`). There is no central registry: a Pome is
//! fetched straight from the repository its source path names (R4), and because
//! the capabilities it requires can be computed from source (0025), a fetch can
//! be audited without trusting anyone's self-report (CAP1).
//!
//! This module implements the `emela pome <verb>` CLI (C1) — `add`, `remove`,
//! `list`, `update`, `install`, `search` — plus the `emela new` scaffold (C2).
//! The pieces live in submodules: source-path normalization, semver, the
//! `Pome.toml`/`Pome.lock`/`Bushel.toml` file formats, Git fetching, graph
//! resolution, and capability auditing.

mod bushel;
mod capability;
mod git;
mod lock;
mod manifest;
mod resolve;
mod semver;
mod source_path;
mod toml_lite;

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

use lock::Lock;
use manifest::Manifest;
use semver::{Requirement, Version};

/// The language spec version a scaffolded Pome targets (`[pome].emela`, F2).
/// Kept in step with the specs this compiler implements.
const EMELA_SPEC_VERSION: &str = "0.1";

/// Dispatches `emela pome <verb> ...` (spec 0032 C1). `args` is everything after
/// `pome`.
pub(crate) fn run(args: &[String]) -> Result<()> {
    let Some((verb, rest)) = args.split_first() else {
        return Err(pome_usage());
    };
    match verb.as_str() {
        "add" => add(rest),
        "remove" => remove(rest),
        "list" => list(rest),
        "update" => update(rest),
        "install" => install(rest),
        "search" => search(rest),
        other => Err(Error::new(format!(
            "unknown `emela pome` verb `{other}`\n{}",
            pome_usage().render()
        ))),
    }
}

/// `emela new <name>` — scaffolds a new entry Pome (spec 0032 C2).
pub(crate) fn scaffold(name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::new("usage: emela new <name>"));
    }
    // The directory is the source path's leaf (`github.com/you/hello` -> `hello`,
    // `hello` -> `hello`), while the Pome keeps the full name.
    let dir_name = source_path::leaf(name);
    let dir = PathBuf::from(dir_name);
    if dir.exists() {
        return Err(Error::new(format!("`{}` already exists", dir.display())));
    }
    std::fs::create_dir_all(dir.join("src"))
        .map_err(|err| Error::new(format!("failed to create `{}`: {err}", dir.display())))?;

    let manifest = Manifest::new(name.to_string(), EMELA_SPEC_VERSION.to_string());
    manifest.save(&dir)?;

    let main = "fn main() -> Unit {\n}\n";
    let main_path = dir.join("src").join("main.emel");
    std::fs::write(&main_path, main)
        .map_err(|err| Error::new(format!("failed to write `{}`: {err}", main_path.display())))?;

    println!("Created Pome `{name}` in {}/", dir.display());
    println!("  {}", manifest::manifest_path(&dir).display());
    println!("  {}", main_path.display());
    Ok(())
}

/// `emela pome add <src>[@<req>]` (spec 0032 C1/C3, CAP1).
fn add(args: &[String]) -> Result<()> {
    let mut spec = None;
    let mut assume_yes = false;
    for arg in args {
        match arg.as_str() {
            "--yes" | "-y" => assume_yes = true,
            other if other.starts_with('-') => {
                return Err(Error::new(format!(
                    "unsupported option `{other}` for `pome add`"
                )));
            }
            other => {
                if spec.replace(other.to_string()).is_some() {
                    return Err(Error::new("`pome add` takes a single <src>[@<req>]"));
                }
            }
        }
    }
    let spec = spec.ok_or_else(|| Error::new("usage: emela pome add <src>[@<req>]"))?;
    let (source, requirement) = parse_add_spec(&spec)?;

    let dir = project_dir()?;
    let mut manifest = Manifest::load(&dir)?;

    // Resolve the requirement, defaulting to a caret on the latest tag when the
    // user gave none (V2/V3).
    let requirement = match requirement {
        Some(req) => req,
        None => Requirement::caret_for(&latest_version(&source)?),
    };

    println!("  Fetched {source}");
    manifest
        .dependencies
        .insert(source.clone(), requirement.clone());

    // Resolve the whole graph (fetches into the cache) so the new dependency and
    // its transitive dependencies are pinned together (V3).
    let resolved = resolve::resolve(&manifest)?;
    let Some(pinned) = resolved.find(&source) else {
        return Err(Error::new(format!("resolution did not select `{source}`")));
    };
    println!(
        "  Resolved {} (commit {})",
        pinned.version,
        short_commit(&pinned.commit)
    );

    // Compute and present the capabilities the added Pome and its transitive
    // dependencies require, straight from source (CAP1/CAP2). The sandbox stays
    // the real gate (CAP3); this is an audit aid.
    let checkouts = checkout_dirs(&resolved);
    let capabilities = capability::required_capabilities(&checkouts)?;
    if capabilities.is_empty() {
        println!("  Capabilities required (computed from source): none");
    } else {
        println!(
            "  Capabilities required by this pome and its dependencies (computed from source):"
        );
        println!(
            "    {}",
            capabilities.iter().cloned().collect::<Vec<_>>().join(", ")
        );
    }

    if !confirm(assume_yes)? {
        println!("  Aborted; Pome.toml and Pome.lock left unchanged.");
        return Ok(());
    }

    manifest.save(&dir)?;
    resolved.save(&lock_dir(&dir)?)?;
    println!("  Updated {} , {}", manifest::FILE_NAME, lock::FILE_NAME);
    Ok(())
}

/// `emela pome remove <src>` (spec 0032 C1/C3).
fn remove(args: &[String]) -> Result<()> {
    let spec = single_source(args, "remove")?;
    let source = source_path::normalize(&spec)?;
    let dir = project_dir()?;
    let mut manifest = Manifest::load(&dir)?;
    if manifest.dependencies.remove(&source).is_none() {
        return Err(Error::new(format!(
            "`{source}` is not a dependency of this Pome"
        )));
    }
    manifest.save(&dir)?;
    // Re-pin the remaining graph so the lock never keeps an orphaned entry (C3).
    let lock_dir = lock_dir(&dir)?;
    if manifest.dependencies.is_empty() {
        Lock::remove_file(&lock_dir)?;
    } else {
        let resolved = resolve::resolve(&manifest)?;
        resolved.save(&lock_dir)?;
    }
    println!("  Removed {source}");
    println!("  Updated {} , {}", manifest::FILE_NAME, lock::FILE_NAME);
    Ok(())
}

/// `emela pome list` — prints the resolved dependency tree (spec 0032 C1).
fn list(args: &[String]) -> Result<()> {
    no_args(args, "list")?;
    let dir = project_dir()?;
    let manifest = Manifest::load(&dir)?;
    let lock = Lock::load(&lock_dir(&dir)?)?;

    // The root line shows the entry Pome's bare version (spec example).
    println!(
        "{} {}",
        manifest.name,
        manifest.version.to_string().trim_start_matches('v')
    );
    let mut roots: Vec<&String> = manifest.dependencies.keys().collect();
    roots.sort();
    let mut visited = std::collections::BTreeSet::new();
    for (index, source) in roots.iter().enumerate() {
        let last = index + 1 == roots.len();
        print_tree(source, &lock, "", last, &mut visited);
    }
    Ok(())
}

/// `emela pome update [<src>]` — re-resolves within the manifest's requirements,
/// picking up newer tags (spec 0032 C1). Without `<src>`, updates every
/// dependency.
fn update(args: &[String]) -> Result<()> {
    // A named source narrows what may move, but resolution still re-pins the
    // whole graph from the requirements; the argument is validated and reported.
    let target = match args {
        [] => None,
        [one] => Some(source_path::normalize(one)?),
        _ => return Err(Error::new("usage: emela pome update [<src>]")),
    };
    let dir = project_dir()?;
    let manifest = Manifest::load(&dir)?;
    if let Some(source) = &target
        && !manifest.dependencies.contains_key(source)
    {
        return Err(Error::new(format!(
            "`{source}` is not a dependency of this Pome"
        )));
    }
    let resolved = resolve::resolve(&manifest)?;
    resolved.save(&lock_dir(&dir)?)?;
    match &target {
        Some(source) => println!("  Updated {source}"),
        None => println!("  Updated all dependencies"),
    }
    println!("  Wrote {}", lock::FILE_NAME);
    Ok(())
}

/// `emela pome install` — materializes the locked dependencies (spec 0032 F5).
fn install(args: &[String]) -> Result<()> {
    no_args(args, "install")?;
    let dir = project_dir()?;
    let lock_dir = lock_dir(&dir)?;
    let lock = Lock::load(&lock_dir)?;
    if lock.packages.is_empty() {
        // No lock (or an empty one): resolve from the manifest so `install` on a
        // fresh clone still produces a lock, matching the usual expectation.
        let manifest = Manifest::load(&dir)?;
        if manifest.dependencies.is_empty() {
            println!("  No dependencies to install");
            return Ok(());
        }
        let resolved = resolve::resolve(&manifest)?;
        resolved.save(&lock_dir)?;
        resolve::install(&resolved)?;
        println!("  Installed {} package(s)", resolved.packages.len());
        return Ok(());
    }
    resolve::install(&lock)?;
    println!("  Installed {} package(s)", lock.packages.len());
    Ok(())
}

/// `emela pome search <query>` — queries an Orchard index (spec 0032 C1). The
/// Orchard is optional and never on the resolution path (R4); when none is
/// configured this reports so rather than failing.
fn search(args: &[String]) -> Result<()> {
    let query = single_source(args, "search")?;
    match std::env::var("EMELA_ORCHARD_URL") {
        Ok(url) if !url.is_empty() => {
            // A concrete Orchard protocol is an Open Question in 0032; until it
            // is specified, report the configured index rather than guess a wire
            // format.
            println!(
                "  Orchard `{url}` is configured, but the search protocol is not yet specified."
            );
            println!(
                "  (Dependency resolution never needs an Orchard; it fetches from source paths directly — R4.)"
            );
            let _ = &query;
            Ok(())
        }
        _ => {
            println!("  No Orchard index is configured (set EMELA_ORCHARD_URL to search one).");
            println!(
                "  An Orchard is discovery-only and optional; resolution fetches from source paths directly (R4)."
            );
            Ok(())
        }
    }
}

// --- helpers ---------------------------------------------------------------

/// Splits `src[@req]` into a canonical source path and an optional requirement.
fn parse_add_spec(spec: &str) -> Result<(String, Option<Requirement>)> {
    match spec.split_once('@') {
        Some((src, req)) => {
            let source = source_path::normalize(src)?;
            let requirement = Requirement::parse(req)?;
            Ok((source, Some(requirement)))
        }
        None => Ok((source_path::normalize(spec)?, None)),
    }
}

/// The greatest tag a repository publishes, used to derive the default caret
/// requirement for `add` (V2/V3).
fn latest_version(source: &str) -> Result<Version> {
    let tags = git::list_versions(source)?;
    tags.into_iter()
        .max()
        .ok_or_else(|| Error::new(format!("`{source}` has no `v`-prefixed version tags")))
}

/// The cache checkout directories for every package in a resolved lock, used for
/// capability auditing.
fn checkout_dirs(lock: &Lock) -> Vec<PathBuf> {
    lock.packages
        .iter()
        .map(|pkg| resolve::checkout_dir(&pkg.source, &pkg.version))
        .collect()
}

/// Renders one dependency subtree of `emela pome list`.
fn print_tree(
    source: &str,
    lock: &Lock,
    prefix: &str,
    last: bool,
    visited: &mut std::collections::BTreeSet<String>,
) {
    let connector = if last { "└── " } else { "├── " };
    match lock.find(source) {
        Some(package) => {
            println!("{prefix}{connector}{source} {}", package.version);
            if !visited.insert(source.to_string()) {
                // Already expanded elsewhere in the tree; don't recurse again.
                return;
            }
            let child_prefix = format!("{prefix}{}", if last { "    " } else { "│   " });
            let mut deps = package.dependencies.clone();
            deps.sort();
            for (index, dep) in deps.iter().enumerate() {
                let child_last = index + 1 == deps.len();
                print_tree(dep, lock, &child_prefix, child_last, visited);
            }
        }
        None => {
            // A declared dependency with no lock entry: resolution hasn't run.
            println!("{prefix}{connector}{source} (unresolved — run `emela pome install`)");
        }
    }
}

/// Prompts for confirmation of a capability audit (spec 0032 CAP1). In a
/// non-interactive context (no TTY, or `--yes`), proceeds without prompting so
/// scripts and CI are not blocked.
fn confirm(assume_yes: bool) -> Result<bool> {
    if assume_yes || !std::io::stdin().is_terminal() {
        return Ok(true);
    }
    print!("  Proceed? [y/N] ");
    std::io::stdout()
        .flush()
        .map_err(|err| Error::new(format!("failed to write prompt: {err}")))?;
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .map_err(|err| Error::new(format!("failed to read answer: {err}")))?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "Yes"))
}

fn short_commit(commit: &str) -> &str {
    commit.get(..7).unwrap_or(commit)
}

/// Where the `Pome.lock` for `project` lives. A standalone Pome keeps its lock
/// beside its own `Pome.toml`; a Pome that is a member of an enclosing Bushel
/// shares the single lock at the Bushel root (spec 0032 F10).
fn lock_dir(project: &Path) -> Result<PathBuf> {
    if let Some(bushel) = bushel::discover(project)? {
        let is_member = bushel
            .member_dirs()
            .iter()
            .any(|member| same_dir(member, project));
        if is_member {
            return Ok(bushel.root);
        }
    }
    Ok(project.to_path_buf())
}

/// Compares two directories for identity, tolerating non-canonical spellings by
/// canonicalizing when possible.
fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Finds the current Pome: the nearest ancestor of the working directory that
/// holds a `Pome.toml`.
fn project_dir() -> Result<PathBuf> {
    let cwd = std::env::current_dir()
        .map_err(|err| Error::new(format!("failed to read the working directory: {err}")))?;
    find_project_dir(&cwd).ok_or_else(|| {
        Error::new(format!(
            "no `{}` found in `{}` or any parent directory (run `emela new` first)",
            manifest::FILE_NAME,
            cwd.display()
        ))
    })
}

/// Walks up from `start` to the nearest directory holding a `Pome.toml`.
fn find_project_dir(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start.to_path_buf());
    while let Some(current) = dir {
        if current.join(manifest::FILE_NAME).exists() {
            return Some(current);
        }
        dir = current.parent().map(Path::to_path_buf);
    }
    None
}

/// The dependency Pomes to place on the import search path when building the
/// file at `input` (spec 0032 M1). Each locked dependency of the Pome enclosing
/// `input` contributes an import root: its source-path leaf (M2) mapping to the
/// Pome's module directory in the cache. Returns `(import_root, source_root)`
/// pairs.
///
/// A single file that is not inside any Pome yields nothing, so plain
/// `emela build file.emel` keeps working. A locked dependency that has not been
/// fetched is a hard error pointing at `emela pome install`, rather than a
/// confusing "unknown package" later.
pub(crate) fn dependency_packages(input: &Path) -> Result<Vec<(String, PathBuf)>> {
    let base = input.parent().unwrap_or_else(|| Path::new("."));
    // Resolve to an absolute path so the walk up to the enclosing Pome is
    // reliable even when `input` is given relative to the working directory.
    let start = base.canonicalize().unwrap_or_else(|_| base.to_path_buf());
    let Some(project) = find_project_dir(&start) else {
        return Ok(Vec::new());
    };

    let lock = Lock::load(&lock_dir(&project)?)?;
    let mut roots = Vec::new();
    for package in &lock.packages {
        let checkout = resolve::checkout_dir(&package.source, &package.version);
        if !checkout.exists() {
            return Err(Error::new(format!(
                "dependency `{}` {} is locked but not fetched; run `emela pome install`",
                package.source, package.version
            )));
        }
        // A Pome's modules live under `src/` by convention (the `emela new`
        // layout); fall back to the checkout root for a flat Pome.
        let src = checkout.join("src");
        let source_root = if src.is_dir() { src } else { checkout };
        roots.push((source_path::leaf(&package.source).to_string(), source_root));
    }
    Ok(roots)
}

fn single_source(args: &[String], verb: &str) -> Result<String> {
    match args {
        [one] => Ok(one.clone()),
        _ => Err(Error::new(format!("usage: emela pome {verb} <src>"))),
    }
}

fn no_args(args: &[String], verb: &str) -> Result<()> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(Error::new(format!(
            "`emela pome {verb}` takes no arguments"
        )))
    }
}

fn pome_usage() -> Error {
    Error::new(
        "usage: emela pome add <src>[@<req>] [--yes] \
         | emela pome remove <src> \
         | emela pome list \
         | emela pome update [<src>] \
         | emela pome install \
         | emela pome search <query>",
    )
}
