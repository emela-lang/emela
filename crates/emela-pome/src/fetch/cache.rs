//! The cache (`~/.emela/pomes`, spec 4.7).
//!
//! ```text
//! <cache>/<escaped path>@<version>/        the files of a version, read-only
//! <cache>/download/<escaped path>/@v/<version>.toml   a verified Pome.toml
//! <cache>/sumdb/<key name>/latest          the largest checkpoint seen, signed
//! ```
//!
//! Every write goes to a temporary file or directory first and is renamed into place, so a
//! concurrent `emela` sees a version either whole or not at all.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::source_path::SourcePath;
use crate::version::Version;

pub struct Cache {
    root: PathBuf,
}

impl Cache {
    pub fn new(root: PathBuf) -> Cache {
        Cache { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The directory of a version's files.
    pub fn dir(&self, path: &SourcePath, version: &Version) -> PathBuf {
        self.root.join(format!("{}@{}", path.escaped(), version))
    }

    pub fn manifest_file(&self, path: &SourcePath, version: &Version) -> PathBuf {
        self.root
            .join("download")
            .join(path.escaped())
            .join("@v")
            .join(format!("{version}.toml"))
    }

    pub fn checkpoint_file(&self, key_name: &str) -> PathBuf {
        self.root.join("sumdb").join(key_name).join("latest")
    }

    /// Writes the files of a version and makes them read-only. Does nothing if it is already there.
    pub fn put_dir(
        &self,
        path: &SourcePath,
        version: &Version,
        files: &[(String, Vec<u8>)],
    ) -> Result<PathBuf, String> {
        let dir = self.dir(path, version);
        if dir.exists() {
            return Ok(dir);
        }
        let parent = dir.parent().expect("under the cache root");
        fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;
        let tmp = tempfile::Builder::new()
            .prefix(".tmp-")
            .tempdir_in(parent)
            .map_err(|e| io_err(parent, e))?;
        for (name, content) in files {
            // The callers hash the files first, which rejects these too; this keeps writes inside.
            if name.starts_with('/')
                || name
                    .split('/')
                    .any(|e| e.is_empty() || e == "." || e == "..")
            {
                return Err(format!(
                    "refusing to write `{name}` outside the version's directory"
                ));
            }
            let file = tmp.path().join(name);
            if let Some(d) = file.parent() {
                fs::create_dir_all(d).map_err(|e| io_err(d, e))?;
            }
            fs::write(&file, content).map_err(|e| io_err(&file, e))?;
        }
        make_read_only(tmp.path()).map_err(|e| io_err(tmp.path(), e))?;
        let tmp = tmp.keep();
        match fs::rename(&tmp, &dir) {
            Ok(()) => Ok(dir),
            // Someone else put the same version first.
            Err(_) if dir.exists() => {
                let _ = remove_read_only(&tmp);
                Ok(dir)
            }
            Err(e) => Err(io_err(&dir, e)),
        }
    }

    /// Writes a file atomically.
    pub fn put_file(&self, file: &Path, content: &[u8]) -> Result<(), String> {
        let parent = file.parent().expect("under the cache root");
        fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;
        let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(|e| io_err(parent, e))?;
        tmp.write_all(content).map_err(|e| io_err(file, e))?;
        tmp.persist(file).map_err(|e| io_err(file, e.error))?;
        Ok(())
    }

    pub fn read_file(&self, file: &Path) -> Option<Vec<u8>> {
        fs::read(file).ok()
    }
}

/// The files of a directory, as (relative `/`-separated path, content), for hashing it again.
pub fn read_dir_files(dir: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d).map_err(|e| io_err(&d, e))? {
            let entry = entry.map_err(|e| io_err(&d, e))?;
            let path = entry.path();
            let kind = entry.file_type().map_err(|e| io_err(&path, e))?;
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file() {
                let rel = path.strip_prefix(dir).expect("under dir");
                let rel = rel
                    .to_str()
                    .ok_or_else(|| format!("{}: not UTF-8", rel.display()))?;
                out.push((
                    rel.replace(std::path::MAIN_SEPARATOR, "/"),
                    fs::read(&path).map_err(|e| io_err(&path, e))?,
                ));
            } else {
                return Err(format!("{}: not a regular file", path.display()));
            }
        }
    }
    Ok(out)
}

fn make_read_only(dir: &Path) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            make_read_only(&path)?;
        }
        set_read_only(&path, true)?;
    }
    set_read_only(dir, true)
}

fn remove_read_only(dir: &Path) -> std::io::Result<()> {
    set_read_only(dir, false)?;
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            remove_read_only(&path)?;
        } else {
            set_read_only(&path, false)?;
        }
    }
    fs::remove_dir_all(dir)
}

fn set_read_only(path: &Path, read_only: bool) -> std::io::Result<()> {
    let mut perms = fs::metadata(path)?.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir = path.is_dir();
        perms.set_mode(match (read_only, dir) {
            (true, true) => 0o555,
            (true, false) => 0o444,
            (false, true) => 0o755,
            (false, false) => 0o644,
        });
    }
    #[cfg(not(unix))]
    perms.set_readonly(read_only);
    fs::set_permissions(path, perms)
}

fn io_err(path: &Path, e: std::io::Error) -> String {
    format!("{}: {e}", path.display())
}
