//! The operations of `emela pome` (spec appendix B) on a project: they keep `Pome.toml` and
//! `Pome.lock` consistent.
//!
//! Each operation computes the new `Pome.toml` and the new lock first and writes them only when
//! everything resolved, so a failed `add` leaves both files as they were.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::edit;
use crate::fetch::transport::Transport;
use crate::fetch::{FetchError, Fetcher};
use crate::hash::MANIFEST_FILE;
use crate::lock::{LOCK_FILE, Lock, Locked};
use crate::manifest::Manifest;
use crate::mvs::{self, Requirements};
use crate::source_path::SourcePath;
use crate::version::Version;

#[derive(Debug)]
pub enum PomeError {
    /// No `Pome.toml` in the directory or its ancestors.
    NoProject(PathBuf),
    Manifest(String),
    Lock(String),
    Fetch(FetchError),
    /// A wrong argument (`remove` of something not depended on, a malformed source path...).
    Usage(String),
    /// Dependencies use `@external` without `[external] allow` (spec 4.7).
    External(String),
    Io(String),
}

impl fmt::Display for PomeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PomeError::NoProject(dir) => {
                write!(
                    f,
                    "no `{MANIFEST_FILE}` found in `{}` or its parent directories",
                    dir.display()
                )
            }
            PomeError::Manifest(m)
            | PomeError::Lock(m)
            | PomeError::Usage(m)
            | PomeError::External(m)
            | PomeError::Io(m) => f.write_str(m),
            PomeError::Fetch(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for PomeError {}

impl From<FetchError> for PomeError {
    fn from(e: FetchError) -> Self {
        PomeError::Fetch(e)
    }
}

pub struct Project {
    pub root: PathBuf,
    text: String,
    pub manifest: Manifest,
    pub lock: Option<Lock>,
}

/// A change of a requirement or of the build list, for reporting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub path: SourcePath,
    pub from: Option<Version>,
    pub to: Option<Version>,
    /// In `[dev-dependencies]`, or in the lock only for the tests (spec 4.7).
    pub dev: bool,
}

impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.from, &self.to) {
            (None, Some(to)) => write!(f, "+ {} {to}", self.path),
            (Some(from), None) => write!(f, "- {} {from}", self.path),
            (Some(from), Some(to)) => write!(f, "  {} {from} -> {to}", self.path),
            (None, None) => write!(f, "  {}", self.path),
        }?;
        if self.dev {
            f.write_str(" (dev)")?;
        }
        Ok(())
    }
}

/// What an operation changed: the requirements in `Pome.toml` and the versions in the lock.
#[derive(Debug, Default)]
pub struct Outcome {
    pub requirements: Vec<Change>,
    pub build_list: Vec<Change>,
    /// Pomes added to `[external] allow` (`add --allow-external`).
    pub allowed: Vec<SourcePath>,
    /// Changes of the `@external` files of locked Pomes (spec appendix B).
    pub external: Vec<ExternalChange>,
}

/// The `.emel` files with `@external` a Pome gained or lost between the old and the new lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalChange {
    pub path: SourcePath,
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

impl Project {
    /// The project whose `Pome.toml` is in `start` or its nearest ancestor.
    pub fn find(start: &Path) -> Result<Project, PomeError> {
        let start = start
            .canonicalize()
            .map_err(|e| PomeError::Io(format!("{}: {e}", start.display())))?;
        let root = start
            .ancestors()
            .find(|d| d.join(MANIFEST_FILE).is_file())
            .ok_or_else(|| PomeError::NoProject(start.clone()))?;
        Project::open(root)
    }

    pub fn open(root: &Path) -> Result<Project, PomeError> {
        let text = read(&root.join(MANIFEST_FILE))?;
        let manifest = parse_manifest(&text, &root.join(MANIFEST_FILE))?;
        let lock_file = root.join(LOCK_FILE);
        let lock = match lock_file.is_file() {
            true => {
                Some(Lock::parse(&read(&lock_file)?).map_err(|e| PomeError::Lock(e.to_string()))?)
            }
            false => None,
        };
        Ok(Project {
            root: root.to_path_buf(),
            text,
            manifest,
            lock,
        })
    }

    /// `emela pome install`: makes the lock match `Pome.toml` and gets every dependency.
    pub fn install<T: Transport>(&mut self, fetcher: &Fetcher<T>) -> Result<Outcome, PomeError> {
        let text = self.text.clone();
        self.apply(fetcher, text, Vec::new(), None)
    }

    /// `emela pome add [--allow-external] [--dev] <src>[@<version>]`: the latest version when none
    /// is given. With `allow_external`, the added Pome and the Pomes it newly brings in that use
    /// `@external` are added to `[external] allow`. With `dev`, it goes to `[dev-dependencies]`;
    /// without, a Pome in `[dev-dependencies]` moves to `[dependencies]`.
    pub fn add<T: Transport>(
        &mut self,
        fetcher: &Fetcher<T>,
        src: &str,
        allow_external: bool,
        dev: bool,
    ) -> Result<Outcome, PomeError> {
        let (path, version) = parse_src(src)?;
        if path == self.manifest.name {
            return Err(PomeError::Usage(format!("`{path}` is this Pome itself")));
        }
        let version = match version {
            Some(v) => v,
            None => fetcher.latest(&path)?,
        };
        if dev && self.manifest.dependencies.contains_key(&path) {
            return Err(PomeError::Usage(format!(
                "`{path}` is in [dependencies]; the tests can use it as it is"
            )));
        }
        if !path.is_version_of(&version) {
            return Err(PomeError::Usage(major_mismatch(&path, &version)));
        }
        let manifest_error = |e: crate::manifest::ManifestError| PomeError::Manifest(e.0);
        let moved = match dev {
            true => None,
            false => edit::move_dependency(
                &self.text,
                &path,
                edit::DEV_DEPENDENCIES,
                edit::DEPENDENCIES,
            )
            .map_err(manifest_error)?,
        };
        let table = if dev {
            edit::DEV_DEPENDENCIES
        } else {
            edit::DEPENDENCIES
        };
        let text = edit::set_dependency(
            moved.as_deref().unwrap_or(&self.text),
            table,
            &path,
            &version,
        )
        .map_err(manifest_error)?;
        let from = self
            .manifest
            .dependencies
            .get(&path)
            .or_else(|| self.manifest.dev_dependencies.get(&path))
            .map(|r| r.version.clone());
        let change = Change {
            path,
            from,
            to: Some(version),
            dev,
        };
        let allow = allow_external.then(|| change.path.clone());
        let changes = if change.from == change.to && moved.is_none() {
            Vec::new()
        } else {
            vec![change]
        };
        self.apply(fetcher, text, changes, allow)
    }

    /// `emela pome remove <src>`.
    pub fn remove<T: Transport>(
        &mut self,
        fetcher: &Fetcher<T>,
        src: &str,
    ) -> Result<Outcome, PomeError> {
        let path = SourcePath::from_input(src).map_err(|e| PomeError::Usage(e.to_string()))?;
        let dev = !self.manifest.dependencies.contains_key(&path);
        let (table, requirements) = match dev {
            false => (edit::DEPENDENCIES, &self.manifest.dependencies),
            true => (edit::DEV_DEPENDENCIES, &self.manifest.dev_dependencies),
        };
        let from = requirements.get(&path).map(|r| r.version.clone());
        let text = edit::remove_dependency(&self.text, table, &path)
            .map_err(|e| PomeError::Manifest(e.0))?
            .ok_or_else(|| {
                PomeError::Usage(format!(
                    "`{path}` is not in [dependencies] or [dev-dependencies]"
                ))
            })?;
        self.apply(
            fetcher,
            text,
            vec![Change {
                path,
                from,
                to: None,
                dev,
            }],
            None,
        )
    }

    /// `emela pome update [<src>]`: raises requirements to the latest version (all when none is
    /// given). Never lowers one.
    pub fn update<T: Transport>(
        &mut self,
        fetcher: &Fetcher<T>,
        src: Option<&str>,
    ) -> Result<Outcome, PomeError> {
        let all: Vec<(&SourcePath, &Version, bool)> = self
            .manifest
            .dependencies
            .iter()
            .map(|(p, r)| (p, &r.version, false))
            .chain(
                self.manifest
                    .dev_dependencies
                    .iter()
                    .map(|(p, r)| (p, &r.version, true)),
            )
            .collect();
        let targets = match src {
            Some(src) => {
                let path =
                    SourcePath::from_input(src).map_err(|e| PomeError::Usage(e.to_string()))?;
                let found: Vec<_> = all.into_iter().filter(|(p, ..)| **p == path).collect();
                if found.is_empty() {
                    return Err(PomeError::Usage(format!(
                        "`{path}` is not in [dependencies] or [dev-dependencies]"
                    )));
                }
                found
            }
            None => all,
        };
        let mut text = self.text.clone();
        let mut changes = Vec::new();
        for (path, current, dev) in targets {
            let latest = fetcher.latest(path)?;
            if latest > *current {
                let table = if dev {
                    edit::DEV_DEPENDENCIES
                } else {
                    edit::DEPENDENCIES
                };
                text = edit::set_dependency(&text, table, path, &latest)
                    .map_err(|e| PomeError::Manifest(e.0))?;
                changes.push(Change {
                    path: path.clone(),
                    from: Some(current.clone()),
                    to: Some(latest),
                    dev,
                });
            }
        }
        self.apply(fetcher, text, changes, None)
    }

    /// `emela pome list`: the resolved dependency tree. Each line is the requirement, and the
    /// selected version when it is larger.
    pub fn list<T: Transport>(&self, fetcher: &Fetcher<T>) -> Result<String, PomeError> {
        let lock = self.lock.as_ref().ok_or_else(missing_lock)?;
        let selected: BTreeMap<&SourcePath, &Version> =
            lock.pomes.iter().map(|p| (&p.path, &p.version)).collect();
        let mut graph = Graph {
            fetcher,
            root: &self.root,
            replace: &self.manifest.replace,
            lock: Some(lock),
        };
        let mut out = format!("{}\n", self.manifest.name);
        let root: Vec<(SourcePath, Version)> = self
            .manifest
            .dependencies
            .iter()
            .chain(&self.manifest.dev_dependencies)
            .map(|(p, r)| (p.clone(), r.version.clone()))
            .collect();
        let dev: BTreeSet<&SourcePath> = lock
            .pomes
            .iter()
            .filter(|p| p.dev)
            .map(|p| &p.path)
            .chain(self.manifest.dev_dependencies.keys())
            .collect();
        tree(
            &mut out,
            "",
            &root,
            &selected,
            &dev,
            &mut graph,
            &mut BTreeSet::new(),
        )?;
        Ok(out)
    }

    /// `emela pome verify`: the cached dependencies whose content differs from the lock.
    pub fn verify<T: Transport>(&self, fetcher: &Fetcher<T>) -> Result<Vec<String>, PomeError> {
        let lock = self.lock.as_ref().ok_or_else(missing_lock)?;
        let mut problems = Vec::new();
        for p in &lock.pomes {
            match fetcher.rehash(&p.path, &p.version)? {
                Some(h) if h != p.hash => problems.push(format!(
                    "{} {}: {h} in the cache, {} in {LOCK_FILE}",
                    p.path, p.version, p.hash
                )),
                _ => {}
            }
        }
        Ok(problems)
    }

    // Resolves with the new `Pome.toml` text and writes both files. With `allow`, the Pomes using
    // `@external` that this change brings in (and `allow` itself) are added to `[external] allow`;
    // any other Pome using `@external` without being allowed stops the change.
    fn apply<T: Transport>(
        &mut self,
        fetcher: &Fetcher<T>,
        text: String,
        requirements: Vec<Change>,
        allow: Option<SourcePath>,
    ) -> Result<Outcome, PomeError> {
        let mut text = text;
        let mut manifest = parse_manifest(&text, &self.root.join(MANIFEST_FILE))?;
        let lock = resolve(fetcher, &self.root, &manifest, self.lock.as_ref())?;
        let build_list = diff(self.lock.as_ref(), &lock);

        let unallowed: Vec<&Locked> = lock
            .pomes
            .iter()
            .filter(|p| !p.external.is_empty() && !manifest.external.contains(&p.path))
            .collect();
        let (allowed, refused): (Vec<&Locked>, Vec<&Locked>) =
            unallowed.into_iter().partition(|p| {
                allow.as_ref().is_some_and(|a| {
                    &p.path == a
                        || self
                            .lock
                            .as_ref()
                            .is_none_or(|old| old.get(&p.path).is_none())
                })
            });
        if !refused.is_empty() {
            return Err(external_error(&refused));
        }
        let allowed: Vec<SourcePath> = allowed.into_iter().map(|p| p.path.clone()).collect();
        if !allowed.is_empty() {
            text = edit::allow_external(&text, &allowed).map_err(|e| PomeError::Manifest(e.0))?;
            manifest = parse_manifest(&text, &self.root.join(MANIFEST_FILE))?;
        }
        let external = external_changes(self.lock.as_ref(), &lock);
        if text != self.text {
            write(&self.root.join(MANIFEST_FILE), &text)?;
        }
        if self.lock.as_ref() != Some(&lock) {
            write(&self.root.join(LOCK_FILE), &lock.to_toml())?;
        }
        (self.text, self.manifest, self.lock) = (text, manifest, Some(lock));
        Ok(Outcome {
            requirements,
            build_list,
            allowed,
            external,
        })
    }
}

/// The lock of a project: the build list of minimal version selection over `[dependencies]` and
/// `[dev-dependencies]` together, without the root and the `[replace]`d Pomes, each downloaded
/// (or, if already locked at that version, fetched and checked against the old lock). The Pomes
/// that only `[dev-dependencies]` reach are marked `dev` (spec 4.7).
pub fn resolve<T: Transport>(
    fetcher: &Fetcher<T>,
    root: &Path,
    manifest: &Manifest,
    old: Option<&Lock>,
) -> Result<Lock, PomeError> {
    let mut graph = Graph {
        fetcher,
        root,
        replace: &manifest.replace,
        lock: old,
    };
    let pairs = |deps: &BTreeMap<SourcePath, crate::manifest::Requirement>| {
        deps.iter()
            .map(|(p, r)| (p.clone(), r.version.clone()))
            .collect::<Vec<_>>()
    };
    let requirements = pairs(&manifest.dependencies);
    let all: Vec<(SourcePath, Version)> = requirements
        .iter()
        .cloned()
        .chain(pairs(&manifest.dev_dependencies))
        .collect();
    let list = mvs::build_list(&all, &mut graph)?;
    let built = mvs::reached(&requirements, &list, &mut graph)?;
    let mut pomes = Vec::new();
    for (path, version) in list {
        if path == manifest.name || manifest.replace.contains_key(&path) {
            continue;
        }
        let locked = match old
            .and_then(|l| l.get(&path))
            .filter(|l| l.version == version)
        {
            // Kept as locked, with the `@external` files counted again from the content (a lock
            // written before the field existed has none).
            Some(locked) => {
                let dir = fetcher.ensure(locked)?;
                let files = crate::fetch::cache::read_dir_files(&dir).map_err(PomeError::Io)?;
                let external = crate::lock::external_files(
                    files.iter().map(|(p, c)| (p.as_str(), c.as_slice())),
                );
                Locked {
                    external,
                    ..locked.clone()
                }
            }
            None => fetcher.download(&path, &version)?,
        };
        pomes.push(Locked {
            dev: !built.contains(&path),
            ..locked
        });
    }
    Ok(Lock::new(pomes))
}

/// The requirements of a version: from the `[replace]` directory if replaced, else fetched and
/// checked against the lock if the lock has that version, else against the checksum database.
struct Graph<'a, T: Transport> {
    fetcher: &'a Fetcher<T>,
    root: &'a Path,
    replace: &'a BTreeMap<SourcePath, PathBuf>,
    lock: Option<&'a Lock>,
}

impl<T: Transport> Requirements for Graph<'_, T> {
    type Error = PomeError;
    fn requirements(
        &mut self,
        path: &SourcePath,
        version: &Version,
    ) -> Result<Vec<(SourcePath, Version)>, PomeError> {
        let Some(dir) = self.replace.get(path) else {
            let locked = self
                .lock
                .and_then(|l| l.get(path))
                .filter(|l| &l.version == version);
            return Ok(self
                .fetcher
                .requirements_of(path, version, locked.map(|l| &l.manifest))?);
        };
        let file = self.root.join(dir).join(MANIFEST_FILE);
        let manifest = parse_manifest(&read(&file)?, &file)?;
        if &manifest.name != path {
            return Err(PomeError::Manifest(format!(
                "{}: names `{}`, but it replaces `{path}`",
                file.display(),
                manifest.name
            )));
        }
        Ok(manifest
            .dependencies
            .into_iter()
            .map(|(p, r)| (p, r.version))
            .collect())
    }
}

fn tree<T: Transport>(
    out: &mut String,
    indent: &str,
    requirements: &[(SourcePath, Version)],
    selected: &BTreeMap<&SourcePath, &Version>,
    dev: &BTreeSet<&SourcePath>,
    graph: &mut Graph<'_, T>,
    on_path: &mut BTreeSet<SourcePath>,
) -> Result<(), PomeError> {
    for (i, (path, required)) in requirements.iter().enumerate() {
        let last = i + 1 == requirements.len();
        let (branch, next) = if last {
            ("└── ", "    ")
        } else {
            ("├── ", "│   ")
        };
        let chosen = selected.get(path).copied();
        let note = match chosen {
            _ if graph.replace.contains_key(path) => {
                format!(" => {}", graph.replace[path].display())
            }
            Some(v) if v != required => format!(" -> {v}"),
            Some(_) => String::new(),
            None => " (not in the lock)".to_string(),
        };
        let note = if dev.contains(path) {
            format!("{note} (dev)")
        } else {
            note
        };
        out.push_str(&format!("{indent}{branch}{path} {required}{note}\n"));
        let version = chosen.cloned().unwrap_or_else(|| required.clone());
        if on_path.insert(path.clone()) {
            let children = graph.requirements(path, &version)?;
            tree(
                out,
                &format!("{indent}{next}"),
                &children,
                selected,
                dev,
                graph,
                on_path,
            )?;
            on_path.remove(path);
        }
    }
    Ok(())
}

fn diff(old: Option<&Lock>, new: &Lock) -> Vec<Change> {
    let before: BTreeMap<&SourcePath, &Locked> = old
        .map(|l| l.pomes.iter().map(|p| (&p.path, p)).collect())
        .unwrap_or_default();
    let after: BTreeMap<&SourcePath, &Locked> = new.pomes.iter().map(|p| (&p.path, p)).collect();
    let paths: BTreeSet<&SourcePath> = before.keys().chain(after.keys()).copied().collect();
    paths
        .into_iter()
        .filter(|p| {
            before.get(p).map(|l| (&l.version, l.dev)) != after.get(p).map(|l| (&l.version, l.dev))
        })
        .map(|p| Change {
            path: p.clone(),
            from: before.get(p).map(|l| l.version.clone()),
            to: after.get(p).map(|l| l.version.clone()),
            dev: after.get(p).or(before.get(p)).is_some_and(|l| l.dev),
        })
        .collect()
}

// The Pomes that use `@external` without `[external] allow`, with their files and how to allow
// them (spec appendix B).
fn external_error(pomes: &[&Locked]) -> PomeError {
    let mut message = String::new();
    for p in pomes {
        message.push_str(&format!("{} {} uses @external:\n", p.path, p.version));
        for f in &p.external {
            message.push_str(&format!("  {f}\n"));
        }
    }
    let paths: Vec<&str> = pomes.iter().map(|p| p.path.as_str()).collect();
    message.push_str(&format!(
        "  = help: it runs host code; allow it with `emela pome add --allow-external {}`, or add it to `[external] allow` in Pome.toml",
        paths[0]
    ));
    if paths.len() > 1 {
        message.push_str(&format!(" (also {})", paths[1..].join(", ")));
    }
    PomeError::External(message)
}

fn external_changes(old: Option<&Lock>, new: &Lock) -> Vec<ExternalChange> {
    new.pomes
        .iter()
        .filter_map(|p| {
            let before: &[String] = old
                .and_then(|l| l.get(&p.path))
                .map_or(&[], |o| &o.external);
            let added: Vec<String> = p
                .external
                .iter()
                .filter(|f| !before.contains(f))
                .cloned()
                .collect();
            let removed: Vec<String> = before
                .iter()
                .filter(|f| !p.external.contains(f))
                .cloned()
                .collect();
            (!added.is_empty() || !removed.is_empty()).then(|| ExternalChange {
                path: p.path.clone(),
                added,
                removed,
            })
        })
        .collect()
}

fn parse_src(src: &str) -> Result<(SourcePath, Option<Version>), PomeError> {
    let (path, version) = match src.rsplit_once('@') {
        Some((p, v)) => (p, Some(v)),
        None => (src, None),
    };
    let path = SourcePath::from_input(path).map_err(|e| PomeError::Usage(e.to_string()))?;
    let version = match version {
        None | Some("latest") => None,
        Some(v) => Some(Version::parse(v).map_err(|e| PomeError::Usage(e.to_string()))?),
    };
    Ok((path, version))
}

fn major_mismatch(path: &SourcePath, version: &Version) -> String {
    match path.major() {
        Some(n) => format!("{version} is not a version of `{path}`: its major must be {n}"),
        None => format!(
            "{version} is not a version of `{path}`: v2 and above are `{path}/v{}`",
            version.major
        ),
    }
}

fn missing_lock() -> PomeError {
    PomeError::Lock(format!("no {LOCK_FILE}; run `emela pome install`"))
}

fn parse_manifest(text: &str, file: &Path) -> Result<Manifest, PomeError> {
    Manifest::parse(text).map_err(|e| PomeError::Manifest(format!("{}: {e}", file.display())))
}

fn read(file: &Path) -> Result<String, PomeError> {
    fs::read_to_string(file).map_err(|e| PomeError::Io(format!("{}: {e}", file.display())))
}

// Writes next to the file and renames, so a reader never sees half a file.
fn write(file: &Path, text: &str) -> Result<(), PomeError> {
    let dir = file.parent().expect("a file in the project");
    let io = |e: std::io::Error| PomeError::Io(format!("{}: {e}", file.display()));
    let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(io)?;
    std::io::Write::write_all(&mut tmp, text.as_bytes()).map_err(io)?;
    // A temporary file is private (0600); keep the permissions of the file it replaces.
    let perms = match fs::metadata(file) {
        Ok(m) => m.permissions(),
        Err(_) => fs::metadata(dir).map_err(io)?.permissions(),
    };
    let mut perms = perms;
    #[cfg(unix)]
    if !file.exists() {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o644);
    }
    tmp.as_file().set_permissions(perms).map_err(io)?;
    tmp.persist(file).map_err(|e| io(e.error))?;
    Ok(())
}
