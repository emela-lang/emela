//! The dependencies of a project for check, build, run and test, from `Pome.lock` (spec 4.7).

use std::collections::HashMap;
use std::path::Path;

use emela_pome::fetch::Fetcher;
use emela_pome::fetch::transport::Transport;
use emela_pome::lock::LOCK_FILE;
use emela_pome::project::PomeError;
use emela_pome::{Lock, Manifest, SourcePath};

use crate::pipeline::{Dependencies, DependencyPome, DependencyRef, PROJECT_FILE};
use crate::source::FileSystem;

/// The dependencies of the project at `base` for the build: every Pome of the lock (and of
/// `[replace]`) with its directory, fetched into the cache if missing and checked against the
/// lock. The lock is not rewritten; a dependency of `Pome.toml` missing from it is an error.
/// A fetcher without sources (the language server) only finds what is in the cache.
///
/// With `tests` (check, test and the language server), `[dev-dependencies]` and what they bring
/// in are read too; otherwise the Pomes the lock marks `dev` are not got (spec 4.7).
pub fn project_dependencies<T: Transport>(
    fs: &dyn FileSystem,
    base: &Path,
    fetcher: &Fetcher<T>,
    tests: bool,
) -> Result<Dependencies, PomeError> {
    let manifest_file = base.join(PROJECT_FILE);
    let text = fs
        .read_to_string(&manifest_file)
        .map_err(|e| PomeError::Io(format!("{}: {e}", manifest_file.display())))?;
    // A project without dependencies needs neither a lock nor a full `[pome]` (it may be empty).
    let table: toml::Table = text
        .parse()
        .map_err(|e| PomeError::Manifest(format!("{}: {e}", manifest_file.display())))?;
    let declares = |key: &str| {
        table
            .get(key)
            .and_then(toml::Value::as_table)
            .is_some_and(|t| !t.is_empty())
    };
    // `[external] allow` is read even without dependencies, to warn about unused entries.
    let allow_external: Vec<String> = table
        .get("external")
        .and_then(|e| e.get("allow"))
        .and_then(toml::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if !(declares("dependencies") || tests && declares("dev-dependencies")) {
        return Ok(Dependencies {
            manifest: manifest_file,
            allow_external,
            ..Dependencies::default()
        });
    }
    let manifest = Manifest::parse(&text)
        .map_err(|e| PomeError::Manifest(format!("{}: {e}", manifest_file.display())))?;
    let lock_file = base.join(LOCK_FILE);
    let lock_text = fs
        .read_to_string(&lock_file)
        .map_err(|_| PomeError::Lock(format!("no {LOCK_FILE}; run `emela pome install`")))?;
    let lock = Lock::parse(&lock_text).map_err(|e| PomeError::Lock(e.to_string()))?;

    let mut builder = Builder {
        fs,
        base,
        manifest: &manifest,
        lock: &lock,
        fetcher,
        index: HashMap::new(),
        pomes: Vec::new(),
    };
    let mut project = Vec::new();
    let mut dev = Vec::new();
    let dev_dependencies = manifest
        .dev_dependencies
        .iter()
        .filter(|_| tests)
        .map(|entry| (entry, true));
    for ((path, requirement), is_dev) in manifest
        .dependencies
        .iter()
        .map(|entry| (entry, false))
        .chain(dev_dependencies)
    {
        if !manifest.replace.contains_key(path) {
            match lock.get(path) {
                Some(locked) if locked.version >= requirement.version => {}
                _ => {
                    return Err(PomeError::Lock(format!(
                        "`{path}` {} is in Pome.toml but not in {LOCK_FILE}; run `emela pome install`",
                        requirement.version
                    )));
                }
            }
        }
        let reference = DependencyRef {
            pome: builder.pome(path)?,
            module: requirement.module.clone(),
        };
        if is_dev {
            dev.push(reference);
        } else {
            project.push(reference);
        }
    }
    Ok(Dependencies {
        pomes: builder.pomes,
        project,
        dev,
        manifest: manifest_file,
        allow_external,
    })
}

struct Builder<'a, T: Transport> {
    fs: &'a dyn FileSystem,
    base: &'a Path,
    manifest: &'a Manifest,
    lock: &'a Lock,
    fetcher: &'a Fetcher<T>,
    index: HashMap<SourcePath, usize>,
    pomes: Vec<DependencyPome>,
}

impl<T: Transport> Builder<'_, T> {
    // The index of a Pome, adding it and its dependencies on first sight.
    fn pome(&mut self, path: &SourcePath) -> Result<usize, PomeError> {
        if let Some(&i) = self.index.get(path) {
            return Ok(i);
        }
        let (dir, version) = match self.manifest.replace.get(path) {
            Some(dir) => (self.base.join(dir), "replace".to_string()),
            None => {
                let locked = self.lock.get(path).ok_or_else(|| {
                    PomeError::Lock(format!(
                        "`{path}` is not in {LOCK_FILE}; run `emela pome install`"
                    ))
                })?;
                (self.fetcher.ensure(locked)?, locked.version.to_string())
            }
        };
        let file = dir.join(PROJECT_FILE);
        let text = self
            .fs
            .read_to_string(&file)
            .map_err(|e| PomeError::Io(format!("{}: {e}", file.display())))?;
        let manifest = Manifest::parse(&text)
            .map_err(|e| PomeError::Manifest(format!("{}: {e}", file.display())))?;
        if &manifest.name != path {
            return Err(PomeError::Manifest(format!(
                "{}: names `{}`, not `{path}`",
                file.display(),
                manifest.name
            )));
        }
        let i = self.pomes.len();
        self.index.insert(path.clone(), i);
        self.pomes.push(DependencyPome {
            name: path.to_string(),
            dir,
            last_element: path.last_element().to_string(),
            module: manifest.module.clone(),
            emela: Some(manifest.emela.clone()),
            targets: manifest.targets.clone(),
            version,
            dependencies: Vec::new(),
        });
        let mut refs = Vec::new();
        for (dep, requirement) in &manifest.dependencies {
            refs.push(DependencyRef {
                pome: self.pome(dep)?,
                module: requirement.module.clone(),
            });
        }
        self.pomes[i].dependencies = refs;
        Ok(i)
    }
}
