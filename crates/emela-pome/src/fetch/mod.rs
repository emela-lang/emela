//! Getting Pomes (spec 4.7): from the proxies and `direct` in the order of `EMELA_POMEPROXY`,
//! checked against the checksum database or `Pome.lock`, and kept in the cache.
//!
//! - [`Fetcher::download`] is for writing a version into the lock: its content is checked against
//!   the checksum database (unless the Pome is private or the database is off).
//! - [`Fetcher::ensure`] is for building from the lock: its content is checked against the hash in
//!   the lock, and the checksum database is not asked.

pub mod archive;
pub mod cache;
pub mod config;
pub mod direct;
pub mod sumdb;
pub mod transport;

use std::fmt;
use std::path::PathBuf;

use crate::hash::{H1, MANIFEST_FILE, manifest_hash, tree_hash};
use crate::lock::Locked;
use crate::manifest::Manifest;
use crate::mvs::Requirements;
use crate::source_path::SourcePath;
use crate::version::{Version, latest_candidates};
use cache::Cache;
use config::{Config, Source};
use direct::Direct;
use sumdb::SumdbClient;
use transport::Transport;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// The version does not exist, or cannot be a Pome (a proxy or the repository said so).
    NotFound(String),
    /// The content differs from the checksum database or the lock: possibly tampered with.
    Verify(String),
    /// No answer, or an unexpected one, from a proxy, the checksum database or git.
    Network(String),
    /// Reading or writing the cache failed.
    Cache(String),
    /// A dependency's `Pome.toml` is malformed.
    Manifest(String),
    /// `EMELA_POMEPROXY=off` (or only proxies for a private Pome).
    NoSource(String),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FetchError::NotFound(m)
            | FetchError::Verify(m)
            | FetchError::Network(m)
            | FetchError::Cache(m)
            | FetchError::Manifest(m)
            | FetchError::NoSource(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for FetchError {}

pub struct Fetcher<T: Transport> {
    config: Config,
    transport: T,
    cache: Cache,
    direct: Direct,
}

impl<T: Transport> Fetcher<T> {
    pub fn new(config: Config, transport: T) -> Self {
        let cache = Cache::new(config.cache_dir.clone());
        Fetcher {
            config,
            transport,
            cache,
            direct: Direct::default(),
        }
    }

    pub fn with_direct(mut self, direct: Direct) -> Self {
        self.direct = direct;
        self
    }

    pub fn cache(&self) -> &Cache {
        &self.cache
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The versions of a Pome, in ascending order.
    pub fn versions(&self, path: &SourcePath) -> Result<Vec<Version>, FetchError> {
        self.first(path, |source| match source {
            Source::Proxy(base) => {
                let body = self.proxy_get(base, &format!("{}/@v/list", path.escaped()))?;
                let mut versions: Vec<Version> = String::from_utf8_lossy(&body)
                    .lines()
                    .filter_map(|l| Version::parse(l.trim()).ok())
                    .filter(|v| path.is_version_of(v))
                    .collect();
                versions.sort();
                Ok(versions)
            }
            Source::Direct => self.direct.versions(path),
        })
    }

    /// The version `emela pome add` picks when none is given: the proxy's `@latest`, which skips
    /// versions that cannot be ingested. `direct` takes the largest release, else the largest
    /// prerelease.
    pub fn latest(&self, path: &SourcePath) -> Result<Version, FetchError> {
        self.first(path, |source| match source {
            Source::Proxy(base) => {
                let body = self.proxy_get(base, &format!("{}/@latest", path.escaped()))?;
                let info: Info = serde_json::from_slice(&body).map_err(|e| {
                    FetchError::Network(format!("{base}: bad @latest of {path}: {e}"))
                })?;
                Version::parse(&info.version)
                    .map_err(|e| FetchError::Network(format!("{base}: {e}")))
            }
            Source::Direct => latest_candidates(&self.direct.versions(path)?)
                .into_iter()
                .next()
                .ok_or_else(|| FetchError::NotFound(format!("{path}: no version tags"))),
        })
    }

    /// The `Pome.toml` of a version: checked against `locked` (its hash in the lock) when given,
    /// else against the checksum database. A cached one was checked when it was stored.
    pub fn manifest(
        &self,
        path: &SourcePath,
        version: &Version,
        locked: Option<&H1>,
    ) -> Result<Vec<u8>, FetchError> {
        let file = self.cache.manifest_file(path, version);
        let cached = self.cache.read_file(&file);
        let content = match &cached {
            Some(c) => c.clone(),
            None => self.first(path, |source| match source {
                Source::Proxy(base) => {
                    self.proxy_get(base, &format!("{}/@v/{version}.toml", path.escaped()))
                }
                Source::Direct => {
                    let fetched = self.direct.fetch(path, version)?;
                    manifest_of(path, version, &fetched.files).map(<[u8]>::to_vec)
                }
            })?,
        };
        let got = manifest_hash(&content);
        match locked {
            Some(want) if got != *want => {
                return Err(mismatch(
                    path,
                    version,
                    MANIFEST_FILE,
                    &got,
                    want,
                    crate::lock::LOCK_FILE,
                ));
            }
            Some(_) => {}
            None if cached.is_none() => {
                if let Some(record) = self.lookup(path, version)?
                    && got != record.manifest
                {
                    return Err(mismatch(
                        path,
                        version,
                        MANIFEST_FILE,
                        &got,
                        &record.manifest,
                        "the checksum database",
                    ));
                }
            }
            None => {}
        }
        if cached.is_none() {
            self.cache
                .put_file(&file, &content)
                .map_err(FetchError::Cache)?;
        }
        Ok(content)
    }

    /// The `[dependencies]` of a version, from its `Pome.toml` (see [`Fetcher::manifest`]).
    pub fn requirements_of(
        &self,
        path: &SourcePath,
        version: &Version,
        locked: Option<&H1>,
    ) -> Result<Vec<(SourcePath, Version)>, FetchError> {
        let content = self.manifest(path, version, locked)?;
        let text = String::from_utf8(content).map_err(|_| {
            FetchError::Manifest(format!("{path}@{version}: Pome.toml is not UTF-8"))
        })?;
        let manifest = Manifest::parse(&text)
            .map_err(|e| FetchError::Manifest(format!("{path}@{version}: {e}")))?;
        if &manifest.name != path {
            return Err(FetchError::Manifest(format!(
                "{path}@{version}: Pome.toml names `{}`",
                manifest.name
            )));
        }
        Ok(manifest
            .dependencies
            .into_iter()
            .map(|(p, r)| (p, r.version))
            .collect())
    }

    /// Gets a version to write it into the lock, checking it against the checksum database.
    pub fn download(&self, path: &SourcePath, version: &Version) -> Result<Locked, FetchError> {
        let (commit, files) = self.files(path, version)?;
        let (tree, manifest) = hashes(path, version, &files)?;
        if let Some(record) = self.lookup(path, version)? {
            if tree != record.tree {
                return Err(mismatch(
                    path,
                    version,
                    "content",
                    &tree,
                    &record.tree,
                    "the checksum database",
                ));
            }
            if manifest != record.manifest {
                return Err(mismatch(
                    path,
                    version,
                    MANIFEST_FILE,
                    &manifest,
                    &record.manifest,
                    "the checksum database",
                ));
            }
        }
        self.cache
            .put_dir(path, version, &files)
            .map_err(FetchError::Cache)?;
        let content = manifest_of(path, version, &files)?;
        self.cache
            .put_file(&self.cache.manifest_file(path, version), content)
            .map_err(FetchError::Cache)?;
        let external =
            crate::lock::external_files(files.iter().map(|(p, c)| (p.as_str(), c.as_slice())));
        Ok(Locked {
            path: path.clone(),
            version: version.clone(),
            commit,
            hash: tree,
            manifest,
            external,
            dev: false,
        })
    }

    /// The directory of a locked version: from the cache, or fetched and checked against the lock.
    pub fn ensure(&self, locked: &Locked) -> Result<PathBuf, FetchError> {
        let dir = self.cache.dir(&locked.path, &locked.version);
        if dir.exists() {
            return Ok(dir);
        }
        let (_, files) = self.files(&locked.path, &locked.version)?;
        let (tree, _) = hashes(&locked.path, &locked.version, &files)?;
        if tree != locked.hash {
            return Err(mismatch(
                &locked.path,
                &locked.version,
                "content",
                &tree,
                &locked.hash,
                crate::lock::LOCK_FILE,
            ));
        }
        self.cache
            .put_dir(&locked.path, &locked.version, &files)
            .map_err(FetchError::Cache)
    }

    /// Hashes a cached version again (`emela pome verify`). `None` if it is not in the cache.
    pub fn rehash(&self, path: &SourcePath, version: &Version) -> Result<Option<H1>, FetchError> {
        let dir = self.cache.dir(path, version);
        if !dir.exists() {
            return Ok(None);
        }
        let files = cache::read_dir_files(&dir).map_err(FetchError::Cache)?;
        tree_hash(files.iter().map(|(p, c)| (p.as_str(), c.as_slice())))
            .map(Some)
            .map_err(|e| FetchError::Verify(format!("{path}@{version} in the cache: {e}")))
    }

    // The commit and files of a version, from the first source that has it.
    fn files(
        &self,
        path: &SourcePath,
        version: &Version,
    ) -> Result<(String, archive::Files), FetchError> {
        self.first(path, |source| match source {
            Source::Proxy(base) => {
                let info =
                    self.proxy_get(base, &format!("{}/@v/{version}.info", path.escaped()))?;
                let info: Info = serde_json::from_slice(&info).map_err(|e| {
                    FetchError::Network(format!("{base}: bad .info of {path}@{version}: {e}"))
                })?;
                if !is_commit(&info.commit) {
                    return Err(FetchError::Network(format!(
                        "{base}: bad commit in .info of {path}@{version}"
                    )));
                }
                let archive =
                    self.proxy_get(base, &format!("{}/@v/{version}.tar.gz", path.escaped()))?;
                let files = archive::read_tar_gz(&archive).map_err(|e| {
                    FetchError::Verify(format!("{path}@{version} from {base}: {e}"))
                })?;
                Ok((info.commit, files))
            }
            Source::Direct => {
                let fetched = self.direct.fetch(path, version)?;
                Ok((fetched.commit, fetched.files))
            }
        })
    }

    // Tries the sources in order. Only "not found" (404, 410) moves on to the next one.
    fn first<R>(
        &self,
        path: &SourcePath,
        mut f: impl FnMut(&Source) -> Result<R, FetchError>,
    ) -> Result<R, FetchError> {
        let sources = self.config.sources_for(path);
        if sources.is_empty() {
            return Err(FetchError::NoSource(format!(
                "{path}: nowhere to get it from (EMELA_POMEPROXY)"
            )));
        }
        let mut last = None;
        for source in &sources {
            match f(source) {
                Err(FetchError::NotFound(m)) => last = Some(m),
                other => return other,
            }
        }
        Err(FetchError::NotFound(last.unwrap_or_default()))
    }

    fn proxy_get(&self, base: &str, rest: &str) -> Result<Vec<u8>, FetchError> {
        let url = format!("{base}/{rest}");
        let res = self.transport.get(&url).map_err(FetchError::Network)?;
        match res.status {
            200 => Ok(res.body),
            404 | 410 => Err(FetchError::NotFound(res.text())),
            s => Err(FetchError::Network(format!(
                "GET {url}: {s} {}",
                res.text()
            ))),
        }
    }

    fn lookup(
        &self,
        path: &SourcePath,
        version: &Version,
    ) -> Result<Option<crate::sumdb::record::Record>, FetchError> {
        match self.config.sumdb_for(path) {
            Some(config) => SumdbClient::new(config, &self.transport, &self.cache)
                .lookup(path, version)
                .map(Some),
            None => Ok(None),
        }
    }
}

/// Walking the dependency graph reads each version's `Pome.toml`.
impl<T: Transport> Requirements for &Fetcher<T> {
    type Error = FetchError;
    fn requirements(
        &mut self,
        path: &SourcePath,
        version: &Version,
    ) -> Result<Vec<(SourcePath, Version)>, FetchError> {
        self.requirements_of(path, version, None)
    }
}

#[derive(serde::Deserialize)]
struct Info {
    version: String,
    #[serde(default)]
    commit: String,
}

fn manifest_of<'a>(
    path: &SourcePath,
    version: &Version,
    files: &'a archive::Files,
) -> Result<&'a [u8], FetchError> {
    files
        .iter()
        .find(|(p, _)| p == MANIFEST_FILE)
        .map(|(_, c)| c.as_slice())
        .ok_or_else(|| FetchError::NotFound(format!("{path}@{version}: no {MANIFEST_FILE}")))
}

fn hashes(
    path: &SourcePath,
    version: &Version,
    files: &archive::Files,
) -> Result<(H1, H1), FetchError> {
    let tree = tree_hash(files.iter().map(|(p, c)| (p.as_str(), c.as_slice())))
        .map_err(|e| FetchError::Verify(format!("{path}@{version}: {e}")))?;
    Ok((tree, manifest_hash(manifest_of(path, version, files)?)))
}

fn mismatch(
    path: &SourcePath,
    version: &Version,
    what: &str,
    got: &H1,
    want: &H1,
    by: &str,
) -> FetchError {
    FetchError::Verify(format!(
        "{path}@{version}: the {what} hash {got} differs from {want} in {by}; the download may have been tampered with"
    ))
}

fn is_commit(s: &str) -> bool {
    s.len() == 40
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
