//! ソースのルートから `.emel` ファイルを集め，モジュール名との対応表を作る（4章）．
//!
//! 1つのファイルが1つのモジュールで，名前はルートからのパスで決まる．
//! `src/http/client.emel` は `Http.Client`．ファイルシステムは [`SourceFs`] で差し替える．

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use la_arena::{Arena, Idx};
use rustc_hash::FxHashMap;
use smol_str::SmolStr;

use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::naming::to_type_name;
use crate::{ModuleName, PRELUDE};

/// ソースファイルの拡張子．
pub const EXTENSION: &str = "emel";

/// ディレクトリの中身を読むだけの小さなファイルシステム．
pub trait SourceFs {
    /// `dir` の直下の項目を返す．順序は問わない（呼び出し側で並べる）．
    fn read_dir(&self, dir: &Path) -> io::Result<Vec<DirEntry>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: OsString,
    pub is_dir: bool,
}

/// 実際のファイルシステム．
#[derive(Debug, Default, Clone, Copy)]
pub struct OsFs;

impl SourceFs for OsFs {
    fn read_dir(&self, dir: &Path) -> io::Result<Vec<DirEntry>> {
        std::fs::read_dir(dir)?
            .map(|entry| {
                let entry = entry?;
                // シンボリックリンクはたどった先の種類で判断する．
                let is_dir = std::fs::metadata(entry.path())?.is_dir();
                Ok(DirEntry {
                    name: entry.file_name(),
                    is_dir,
                })
            })
            .collect()
    }
}

/// メモリ上のファイル一覧．テストと，LSP の未保存バッファ向け．
#[derive(Debug, Default, Clone)]
pub struct MemoryFs {
    files: BTreeSet<PathBuf>,
}

impl MemoryFs {
    pub fn new(files: impl IntoIterator<Item = impl Into<PathBuf>>) -> Self {
        MemoryFs {
            files: files.into_iter().map(Into::into).collect(),
        }
    }
}

impl SourceFs for MemoryFs {
    fn read_dir(&self, dir: &Path) -> io::Result<Vec<DirEntry>> {
        let mut entries = Vec::new();
        let mut found = false;
        for file in &self.files {
            let Ok(rest) = file.strip_prefix(dir) else {
                continue;
            };
            let mut components = rest.components();
            let Some(first) = components.next() else {
                continue;
            };
            found = true;
            let entry = DirEntry {
                name: first.as_os_str().to_owned(),
                is_dir: components.next().is_some(),
            };
            // パスは区切りごとに比べて並ぶので，同じディレクトリの中身は隣り合う．
            if entries.last() != Some(&entry) {
                entries.push(entry);
            }
        }
        if found {
            Ok(entries)
        } else {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("`{}` がない", dir.display()),
            ))
        }
    }
}

pub type ModuleId = Idx<ModuleData>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleData {
    pub name: ModuleName,
    /// ルートを付けたファイルのパス．
    pub file: PathBuf,
}

/// モジュール名とファイルの対応表．
#[derive(Debug, Default, Clone)]
pub struct ModuleMap {
    modules: Arena<ModuleData>,
    by_name: FxHashMap<ModuleName, ModuleId>,
}

impl ModuleMap {
    /// import のパスからモジュールを引く．
    pub fn lookup(&self, name: &ModuleName) -> Option<ModuleId> {
        self.by_name.get(name).copied()
    }

    pub fn len(&self) -> usize {
        self.modules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    /// ファイルのパス順に並ぶ．
    pub fn iter(&self) -> impl Iterator<Item = (ModuleId, &ModuleData)> {
        self.modules.iter()
    }

    fn insert(&mut self, data: ModuleData) -> ModuleId {
        let name = data.name.clone();
        let id = self.modules.alloc(data);
        self.by_name.insert(name, id);
        id
    }
}

impl std::ops::Index<ModuleId> for ModuleMap {
    type Output = ModuleData;

    fn index(&self, id: ModuleId) -> &ModuleData {
        &self.modules[id]
    }
}

/// `root` の下の `.emel` ファイルを集め，対応表を作る．診断は全部集める．
pub fn collect_modules(fs: &dyn SourceFs, root: &Path) -> (ModuleMap, Vec<Diagnostic>) {
    let mut collector = Collector {
        fs,
        map: ModuleMap::default(),
        diagnostics: Vec::new(),
        by_lowercase: FxHashMap::default(),
    };
    collector.walk(root, &mut Vec::new());
    (collector.map, collector.diagnostics)
}

struct Collector<'a> {
    fs: &'a dyn SourceFs,
    map: ModuleMap,
    diagnostics: Vec<Diagnostic>,
    /// 小文字にしたパス → 最初に見たファイル．大文字小文字の違いだけのファイルを見つける．
    by_lowercase: FxHashMap<String, PathBuf>,
}

impl Collector<'_> {
    fn walk(&mut self, dir: &Path, prefix: &mut Vec<SmolStr>) {
        let mut entries = match self.fs.read_dir(dir) {
            Ok(entries) => entries,
            Err(err) => {
                self.error(
                    dir,
                    DiagnosticKind::Io {
                        message: SmolStr::new(err.to_string()),
                    },
                );
                return;
            }
        };
        // 対応表の順序と診断の順序を決めるために名前で並べる．
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        for entry in entries {
            let path = dir.join(&entry.name);
            // 隠しファイルと隠しディレクトリ（エディタの一時ファイル，.git など）は見ない．
            if entry.name.as_encoded_bytes().starts_with(b".") {
                continue;
            }
            if entry.is_dir {
                let name = entry.name.to_string_lossy();
                match to_type_name(&name) {
                    Ok(segment) => {
                        prefix.push(segment);
                        self.walk(&path, prefix);
                        prefix.pop();
                    }
                    Err(error) => self.error(
                        &path,
                        DiagnosticKind::InvalidDirName {
                            name: name.into_owned(),
                            error,
                        },
                    ),
                }
            } else if path.extension().is_some_and(|ext| ext == EXTENSION) {
                self.file(path, prefix);
            }
        }
    }

    fn file(&mut self, path: PathBuf, prefix: &[SmolStr]) {
        let stem = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let lowercase = path.to_string_lossy().to_lowercase();
        let converted = to_type_name(&stem);
        let first_same_case = self.by_lowercase.get(&lowercase).cloned();
        if first_same_case.is_none() {
            self.by_lowercase.insert(lowercase, path.clone());
        }

        let segment = match converted {
            Ok(segment) => segment,
            Err(error) => {
                self.error(
                    &path,
                    DiagnosticKind::InvalidFileName {
                        name: stem.clone(),
                        error,
                    },
                );
                // 大文字小文字の違いだけで正しい名前のファイルと重なるなら，それも報告する．
                // 大文字小文字を区別しないファイルシステムでは同じファイルになる．
                if let Some(first) = first_same_case
                    && let Ok(segment) = to_type_name(&stem.to_lowercase())
                {
                    let name = ModuleName::new(prefix.iter().cloned().chain([segment]));
                    self.error(&path, DiagnosticKind::DuplicateModule { name, first });
                }
                return;
            }
        };
        let name = ModuleName::new(prefix.iter().cloned().chain([segment]));

        if prefix.is_empty() && name.last() == PRELUDE {
            self.error(&path, DiagnosticKind::ReservedModuleName { name });
            return;
        }
        if let Some(first) = self.map.lookup(&name).map(|id| self.map[id].file.clone()) {
            self.error(&path, DiagnosticKind::DuplicateModule { name, first });
            return;
        }
        // 先に正しくない名前のファイルを見ていて，こちらと大文字小文字だけ違う．
        if let Some(first) = first_same_case {
            self.error(
                &first,
                DiagnosticKind::DuplicateModule {
                    name: name.clone(),
                    first: path.clone(),
                },
            );
        }
        self.map.insert(ModuleData { name, file: path });
    }

    fn error(&mut self, file: &Path, kind: DiagnosticKind) {
        self.diagnostics.push(Diagnostic {
            file: file.to_owned(),
            range: None,
            kind,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(files: &[&str]) -> (ModuleMap, Vec<String>) {
        let fs = MemoryFs::new(files.iter().copied());
        let (map, diagnostics) = collect_modules(&fs, Path::new("src"));
        let diagnostics = diagnostics
            .iter()
            .map(|d| format!("{}: {d}", d.file.display()))
            .collect();
        (map, diagnostics)
    }

    fn names(map: &ModuleMap) -> Vec<String> {
        map.iter()
            .map(|(_, m)| format!("{} = {}", m.name, m.file.display()))
            .collect()
    }

    #[test]
    fn maps_paths_to_module_names() {
        let (map, diagnostics) = collect(&[
            "src/main.emel",
            "src/http/client.emel",
            "src/http/server.emel",
            "src/json_codec.emel",
            "src/http2/frame.emel",
            "src/readme.md",
            "src/.hidden.emel",
            "src/.git/objects.emel",
        ]);
        assert_eq!(diagnostics, Vec::<String>::new());
        assert_eq!(
            names(&map),
            [
                "Http.Client = src/http/client.emel",
                "Http.Server = src/http/server.emel",
                "Http2.Frame = src/http2/frame.emel",
                "JsonCodec = src/json_codec.emel",
                "Main = src/main.emel",
            ]
        );
        let id = map.lookup(&ModuleName::parse("Http.Client")).unwrap();
        assert_eq!(map[id].file, Path::new("src/http/client.emel"));
        assert_eq!(map.lookup(&ModuleName::parse("Http")), None);
    }

    #[test]
    fn rejects_invalid_names() {
        let (map, diagnostics) = collect(&[
            "src/_x.emel",
            "src/Foo.emel",
            "src/a__b.emel",
            "src/a.emel",
            "src/HTTP/client.emel",
            "src/ok/a-b.emel",
            "src/ok/fine.emel",
        ]);
        assert_eq!(
            diagnostics,
            [
                "src/Foo.emel: ファイル名 `Foo` をモジュール名にできない: 小文字の snake_case ではない",
                "src/HTTP: ディレクトリ名 `HTTP` をモジュール名にできない: 小文字の snake_case ではない",
                "src/_x.emel: ファイル名 `_x` をモジュール名にできない: 小文字の snake_case ではない",
                "src/a.emel: ファイル名 `a` をモジュール名にできない: 変換後の `A` が型名にならない",
                "src/a__b.emel: ファイル名 `a__b` をモジュール名にできない: 小文字の snake_case ではない",
                "src/ok/a-b.emel: ファイル名 `a-b` をモジュール名にできない: 小文字の snake_case ではない",
            ]
        );
        assert_eq!(names(&map), ["Ok.Fine = src/ok/fine.emel"]);
    }

    #[test]
    fn reports_duplicates() {
        let (map, diagnostics) = collect(&[
            "src/ab2.emel",
            "src/ab_2.emel",
            "src/Json_Codec.emel",
            "src/json_codec.emel",
            "src/util.emel",
            "src/UTIL.emel",
        ]);
        assert_eq!(
            diagnostics,
            [
                "src/Json_Codec.emel: ファイル名 `Json_Codec` をモジュール名にできない: 小文字の snake_case ではない",
                "src/UTIL.emel: ファイル名 `UTIL` をモジュール名にできない: 小文字の snake_case ではない",
                "src/ab_2.emel: モジュール `Ab2` が重複している（先に `src/ab2.emel` がある）",
                "src/Json_Codec.emel: モジュール `JsonCodec` が重複している（先に `src/json_codec.emel` がある）",
                "src/UTIL.emel: モジュール `Util` が重複している（先に `src/util.emel` がある）",
            ]
        );
        assert_eq!(
            names(&map),
            [
                "Ab2 = src/ab2.emel",
                "JsonCodec = src/json_codec.emel",
                "Util = src/util.emel",
            ]
        );
    }

    #[test]
    fn prelude_is_reserved() {
        let (map, diagnostics) = collect(&["src/prelude.emel", "src/std/prelude.emel"]);
        assert_eq!(
            diagnostics,
            ["src/prelude.emel: モジュール名 `Prelude` は予約されている"]
        );
        assert_eq!(names(&map), ["Std.Prelude = src/std/prelude.emel"]);
    }

    #[test]
    fn missing_root() {
        let (map, diagnostics) = collect(&["lib/main.emel"]);
        assert!(map.is_empty());
        assert_eq!(diagnostics, ["src: ディレクトリを読めない: `src` がない"]);
    }

    #[test]
    fn os_fs_reads_real_directory() {
        let root = std::env::temp_dir().join(format!("emela-resolve-{}", std::process::id()));
        std::fs::create_dir_all(root.join("http")).unwrap();
        std::fs::write(root.join("main.emel"), "").unwrap();
        std::fs::write(root.join("http/client.emel"), "").unwrap();
        let (map, diagnostics) = collect_modules(&OsFs, &root);
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(diagnostics, []);
        let names: Vec<_> = map.iter().map(|(_, m)| m.name.to_string()).collect();
        assert_eq!(names, ["Http.Client", "Main"]);
    }
}
