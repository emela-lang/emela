//! The source table: file IDs, paths, text, and line/column conversion.
//!
//! File reading goes through [`FileSystem`]. Tests and unsaved LSP buffers use
//! [`MemoryFiles`].

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::{Component, Path, PathBuf};

use emela_resolve::{DirEntry, OsFs, SourceFs};
use line_index::{LineCol, LineIndex, TextSize};

/// Assigned in insertion order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileId(u32);

impl FileId {
    pub fn index(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone)]
pub struct SourceFile {
    path: PathBuf,
    text: String,
    line_index: LineIndex,
}

impl SourceFile {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn line_index(&self) -> &LineIndex {
        &self.line_index
    }

    /// Converts a byte offset to a line and column (both 0-based; the column counts UTF-8
    /// bytes).
    pub fn line_col(&self, offset: TextSize) -> LineCol {
        self.line_index.line_col(offset)
    }
}

#[derive(Debug, Clone, Default)]
pub struct SourceDb {
    files: Vec<SourceFile>,
    by_path: HashMap<PathBuf, FileId>,
    /// The base for paths shown in diagnostics (the project directory).
    base: Option<PathBuf>,
}

impl SourceDb {
    pub fn new() -> Self {
        SourceDb::default()
    }

    /// If the path is already present, replaces its text and returns the same ID.
    pub fn add(&mut self, path: impl Into<PathBuf>, text: impl Into<String>) -> FileId {
        let path = path.into();
        let text = text.into();
        let line_index = LineIndex::new(&text);
        if let Some(&id) = self.by_path.get(&path) {
            let file = &mut self.files[id.0 as usize];
            file.text = text;
            file.line_index = line_index;
            return id;
        }
        let id = FileId(u32::try_from(self.files.len()).expect("too many files"));
        self.by_path.insert(path.clone(), id);
        self.files.push(SourceFile {
            path,
            text,
            line_index,
        });
        id
    }

    /// Sets the base for paths shown in diagnostics.
    pub fn set_base(&mut self, base: impl Into<PathBuf>) {
        self.base = Some(base.into());
    }

    /// Relative to the base if under it; otherwise unchanged.
    pub fn display_path<'a>(&self, path: &'a Path) -> &'a Path {
        match &self.base {
            Some(base) => match path.strip_prefix(base) {
                Ok(relative) if !relative.as_os_str().is_empty() => relative,
                _ => path,
            },
            None => path,
        }
    }

    pub fn file_id(&self, path: &Path) -> Option<FileId> {
        self.by_path.get(path).copied()
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (FileId, &SourceFile)> {
        self.files
            .iter()
            .enumerate()
            .map(|(i, file)| (FileId(i as u32), file))
    }
}

impl std::ops::Index<FileId> for SourceDb {
    type Output = SourceFile;

    fn index(&self, id: FileId) -> &SourceFile {
        &self.files[id.0 as usize]
    }
}

/// A directory listing ([`SourceFs`]) plus file reading and kind checks.
pub trait FileSystem: SourceFs {
    /// Makes the path absolute; an error if it does not exist.
    fn absolute(&self, path: &Path) -> io::Result<PathBuf>;
    fn read_to_string(&self, path: &Path) -> io::Result<String>;
    fn is_file(&self, path: &Path) -> bool;
    fn is_dir(&self, path: &Path) -> bool;
}

impl FileSystem for OsFs {
    fn absolute(&self, path: &Path) -> io::Result<PathBuf> {
        std::fs::canonicalize(path)
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }
}

/// In-memory files, built from path and text pairs.
///
/// Rooted at `/`. Relative paths are taken from `/`, and `.` and `..` segments are collapsed
/// textually (`./src/main.emel` is the same as `/src/main.emel`).
#[derive(Debug, Default, Clone)]
pub struct MemoryFiles {
    files: BTreeMap<PathBuf, String>,
}

impl MemoryFiles {
    pub fn new<P: AsRef<Path>, T: Into<String>>(files: impl IntoIterator<Item = (P, T)>) -> Self {
        let files = files
            .into_iter()
            .map(|(path, text)| (clean(path.as_ref()), text.into()))
            .collect();
        MemoryFiles { files }
    }
}

/// Makes the path absolute from `/` and collapses `.` and `..`.
fn clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::from("/");
    for component in path.components() {
        match component {
            Component::Normal(name) => out.push(name),
            Component::ParentDir => {
                out.pop();
            }
            Component::RootDir | Component::CurDir | Component::Prefix(_) => {}
        }
    }
    out
}

impl SourceFs for MemoryFiles {
    fn read_dir(&self, dir: &Path) -> io::Result<Vec<DirEntry>> {
        let dir = clean(dir);
        let mut entries: Vec<DirEntry> = Vec::new();
        for file in self.files.keys() {
            let Ok(rest) = file.strip_prefix(&dir) else {
                continue;
            };
            let mut components = rest.components();
            let Some(first) = components.next() else {
                continue;
            };
            let entry = DirEntry {
                name: first.as_os_str().to_owned(),
                is_dir: components.next().is_some(),
            };
            // Paths sort segment by segment, so a directory's entries are adjacent.
            if entries.last() != Some(&entry) {
                entries.push(entry);
            }
        }
        if entries.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("`{}` not found", dir.display()),
            ));
        }
        Ok(entries)
    }
}

impl FileSystem for MemoryFiles {
    fn absolute(&self, path: &Path) -> io::Result<PathBuf> {
        let path = clean(path);
        if self.is_file(&path) || self.is_dir(&path) {
            Ok(path)
        } else {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("`{}` not found", path.display()),
            ))
        }
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        self.files.get(&clean(path)).cloned().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("`{}` not found", path.display()),
            )
        })
    }

    fn is_file(&self, path: &Path) -> bool {
        self.files.contains_key(&clean(path))
    }

    fn is_dir(&self, path: &Path) -> bool {
        let path = clean(path);
        self.files
            .keys()
            .any(|file| *file != path && file.starts_with(&path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_offsets_to_line_col() {
        let mut db = SourceDb::new();
        let id = db.add("src/main.emel", "let x = 1\nlet あ = 2\n");
        let file = &db[id];
        assert_eq!(file.line_col(0.into()), LineCol { line: 0, col: 0 });
        assert_eq!(file.line_col(14.into()), LineCol { line: 1, col: 4 });
        // Columns count bytes (`あ` is 3 bytes).
        assert_eq!(file.line_col(18.into()), LineCol { line: 1, col: 8 });
    }

    #[test]
    fn same_path_keeps_id() {
        let mut db = SourceDb::new();
        let a = db.add("a.emel", "1");
        let b = db.add("b.emel", "2");
        assert_ne!(a, b);
        assert_eq!(db.add("a.emel", "3\n4"), a);
        assert_eq!(db[a].text(), "3\n4");
        assert_eq!(db[a].line_col(2.into()), LineCol { line: 1, col: 0 });
        assert_eq!(db.file_id(Path::new("b.emel")), Some(b));
        assert_eq!(db.len(), 2);
    }

    #[test]
    fn memory_files() {
        let fs = MemoryFiles::new([("p/src/main.emel", "main"), ("p/src/a/b.emel", "b")]);
        assert!(fs.is_dir(Path::new("p")));
        assert!(fs.is_dir(Path::new("p/src/a")));
        assert!(!fs.is_dir(Path::new("p/src/main.emel")));
        assert!(fs.is_file(Path::new("p/src/main.emel")));
        assert!(!fs.is_file(Path::new("p/src")));
        assert_eq!(fs.read_to_string(Path::new("p/src/a/b.emel")).unwrap(), "b");
        assert!(fs.read_to_string(Path::new("p/x.emel")).is_err());
        assert!(fs.is_file(Path::new("./p/src/main.emel")));
        assert!(fs.is_dir(Path::new(".")));
        let names = |dir: &str| -> Vec<String> {
            fs.read_dir(Path::new(dir))
                .unwrap()
                .into_iter()
                .map(|e| {
                    format!(
                        "{}{}",
                        e.name.to_string_lossy(),
                        if e.is_dir { "/" } else { "" }
                    )
                })
                .collect()
        };
        assert_eq!(names("."), ["p/"]);
        assert_eq!(names("p/src"), ["a/", "main.emel"]);
        assert!(fs.read_dir(Path::new("q")).is_err());
        assert_eq!(
            fs.absolute(Path::new("./p/src/../src/a")).unwrap(),
            Path::new("/p/src/a")
        );
        assert!(fs.absolute(Path::new("p/missing")).is_err());
    }

    #[test]
    fn display_path_is_relative_to_base() {
        let mut db = SourceDb::new();
        assert_eq!(
            db.display_path(Path::new("/p/src/a.emel")),
            Path::new("/p/src/a.emel")
        );
        db.set_base("/p");
        assert_eq!(
            db.display_path(Path::new("/p/src/a.emel")),
            Path::new("src/a.emel")
        );
        assert_eq!(db.display_path(Path::new("/p")), Path::new("/p"));
        assert_eq!(
            db.display_path(Path::new("/q/a.emel")),
            Path::new("/q/a.emel")
        );
    }
}
