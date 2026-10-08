//! ソースの表．ファイル ID，パス，テキスト，行・列の変換を持つ．
//!
//! ファイルの読み込みは [`FileSystem`] で差し替える．テストと LSP の未保存バッファは
//! [`MemoryFiles`] を使う．

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::{Component, Path, PathBuf};

use emela_resolve::{DirEntry, OsFs, SourceFs};
use line_index::{LineCol, LineIndex, TextSize};

/// ソースの表の中のファイル．追加した順に振る．
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

    /// バイト位置を行・列（どちらも 0 始まり，列は UTF-8 のバイト数）にする．
    pub fn line_col(&self, offset: TextSize) -> LineCol {
        self.line_index.line_col(offset)
    }
}

/// ソースの表．
#[derive(Debug, Clone, Default)]
pub struct SourceDb {
    files: Vec<SourceFile>,
    by_path: HashMap<PathBuf, FileId>,
}

impl SourceDb {
    pub fn new() -> Self {
        SourceDb::default()
    }

    /// ファイルを足す．同じパスがすでにあれば，テキストを差し替えて同じ ID を返す．
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
        let id = FileId(u32::try_from(self.files.len()).expect("ファイルが多すぎる"));
        self.by_path.insert(path.clone(), id);
        self.files.push(SourceFile {
            path,
            text,
            line_index,
        });
        id
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

/// ディレクトリの一覧（[`SourceFs`]）に，ファイルの読み込みと種類の判定を足したもの．
pub trait FileSystem: SourceFs {
    fn read_to_string(&self, path: &Path) -> io::Result<String>;
    fn is_file(&self, path: &Path) -> bool;
    fn is_dir(&self, path: &Path) -> bool;
}

impl FileSystem for OsFs {
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

/// メモリ上のファイル．パスとテキストの組で組み立てる．
///
/// パスの `.` の区切りは無視する（`./src/main.emel` と `src/main.emel` は同じ）．
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

/// `.` の区切りを落とす．`.` だけなら空のパスになる．
fn clean(path: &Path) -> PathBuf {
    path.components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect()
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
            // パスは区切りごとに比べて並ぶので，同じディレクトリの中身は隣り合う．
            if entries.last() != Some(&entry) {
                entries.push(entry);
            }
        }
        if entries.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("`{}` がない", dir.display()),
            ));
        }
        Ok(entries)
    }
}

impl FileSystem for MemoryFiles {
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        self.files.get(&clean(path)).cloned().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("`{}` がない", path.display()),
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
        // 列はバイト数で数える（`あ` は3バイト）．
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
    }
}
