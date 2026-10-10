//! エディタで開いているファイルの未保存の内容を，ファイルシステムに重ねる．
//!
//! driver の `check` はファイルを [`FileSystem`] から読むので，ここで差し替えれば
//! 未保存の内容がそのまま解析にのる．ディスクにまだないファイルも一覧に足す．

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use emela_driver::FileSystem;
use emela_resolve::{DirEntry, SourceFs};

/// 下のファイルシステムに，開いているファイルの内容を重ねたもの．パスは絶対パス．
#[derive(Debug)]
pub struct Overlay<Fs> {
    inner: Fs,
    files: BTreeMap<PathBuf, String>,
}

impl<Fs: FileSystem> Overlay<Fs> {
    pub fn new(inner: Fs) -> Self {
        Overlay {
            inner,
            files: BTreeMap::new(),
        }
    }

    /// 開いたか書き換えたファイルの内容を重ねる．同じパスなら差し替える．
    pub fn set(&mut self, path: PathBuf, text: String) {
        self.files.insert(path, text);
    }

    /// 閉じたファイルをディスクの内容に戻す．
    pub fn remove(&mut self, path: &Path) {
        self.files.remove(path);
    }

    /// エディタから来たパスを，driver が使う絶対パスにそろえる．
    ///
    /// 下のファイルシステムで絶対パスにできればそれを使う（`OsFs` ならシンボリックリンクも解く）．
    /// ディスクにまだないファイルは，親のディレクトリだけを直して名前をつなぐ．
    pub fn normalize(&self, path: &Path) -> PathBuf {
        if let Ok(absolute) = self.inner.absolute(path) {
            return absolute;
        }
        match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) => match self.inner.absolute(parent) {
                Ok(parent) => parent.join(name),
                Err(_) => path.to_owned(),
            },
            _ => path.to_owned(),
        }
    }
}

impl<Fs: FileSystem> SourceFs for Overlay<Fs> {
    fn read_dir(&self, dir: &Path) -> io::Result<Vec<DirEntry>> {
        let inner = self.inner.read_dir(dir);
        let mut extra = Vec::new();
        for path in self.files.keys() {
            let Ok(rest) = path.strip_prefix(dir) else {
                continue;
            };
            let mut components = rest.components();
            let Some(first) = components.next() else {
                continue;
            };
            extra.push(DirEntry {
                name: first.as_os_str().to_owned(),
                is_dir: components.next().is_some(),
            });
        }
        let mut entries = match inner {
            Ok(entries) => entries,
            Err(err) if extra.is_empty() => return Err(err),
            Err(_) => Vec::new(),
        };
        for entry in extra {
            if !entries.iter().any(|e| e.name == entry.name) {
                entries.push(entry);
            }
        }
        Ok(entries)
    }
}

impl<Fs: FileSystem> FileSystem for Overlay<Fs> {
    fn absolute(&self, path: &Path) -> io::Result<PathBuf> {
        if self.files.contains_key(path) {
            return Ok(path.to_owned());
        }
        self.inner.absolute(path).or_else(|err| {
            if self.is_dir(path) {
                Ok(path.to_owned())
            } else {
                Err(err)
            }
        })
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        match self.files.get(path) {
            Some(text) => Ok(text.clone()),
            None => self.inner.read_to_string(path),
        }
    }

    fn is_file(&self, path: &Path) -> bool {
        self.files.contains_key(path) || self.inner.is_file(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.inner.is_dir(path)
            || self
                .files
                .keys()
                .any(|file| file != path && file.starts_with(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emela_driver::MemoryFiles;

    #[test]
    fn overlays_unsaved_and_new_files() {
        let mut fs = Overlay::new(MemoryFiles::new([("/p/src/a.emel", "disk")]));
        fs.set("/p/src/a.emel".into(), "buffer".into());
        fs.set("/p/src/new/b.emel".into(), "new".into());
        assert_eq!(
            fs.read_to_string(Path::new("/p/src/a.emel")).unwrap(),
            "buffer"
        );
        assert!(fs.is_file(Path::new("/p/src/new/b.emel")));
        assert!(fs.is_dir(Path::new("/p/src/new")));
        let mut names: Vec<_> = fs
            .read_dir(Path::new("/p/src"))
            .unwrap()
            .into_iter()
            .map(|e| (e.name.into_string().unwrap(), e.is_dir))
            .collect();
        names.sort();
        assert_eq!(names, [("a.emel".into(), false), ("new".into(), true)]);
        assert_eq!(
            fs.normalize(Path::new("/p/src/new/b.emel")),
            Path::new("/p/src/new/b.emel")
        );
        fs.remove(Path::new("/p/src/a.emel"));
        assert_eq!(
            fs.read_to_string(Path::new("/p/src/a.emel")).unwrap(),
            "disk"
        );
    }
}
