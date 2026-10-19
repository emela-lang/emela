//! Collects `.emel` files under the source root and maps them to module names (spec chapter 4).
//!
//! Each file is one module, named by its path from the root: `src/http/client.emel` is
//! `Http.Client`. The file system is swappable through [`SourceFs`].

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use la_arena::{Arena, Idx};
use rustc_hash::FxHashMap;
use smol_str::SmolStr;

use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::naming::to_type_name;
use crate::{ModuleName, PRELUDE, STD};

pub const EXTENSION: &str = "emel";

/// A minimal file system that only lists directories.
pub trait SourceFs {
    /// The entries directly under `dir`, in any order (the caller sorts them).
    fn read_dir(&self, dir: &Path) -> io::Result<Vec<DirEntry>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: OsString,
    pub is_dir: bool,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct OsFs;

impl SourceFs for OsFs {
    fn read_dir(&self, dir: &Path) -> io::Result<Vec<DirEntry>> {
        std::fs::read_dir(dir)?
            .map(|entry| {
                let entry = entry?;
                // Symlinks are classified by what they point to.
                let is_dir = std::fs::metadata(entry.path())?.is_dir();
                Ok(DirEntry {
                    name: entry.file_name(),
                    is_dir,
                })
            })
            .collect()
    }
}

/// An in-memory file list, for tests and unsaved LSP buffers.
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
            // Paths compare segment by segment, so entries of the same directory are adjacent.
            if entries.last() != Some(&entry) {
                entries.push(entry);
            }
        }
        if found {
            Ok(entries)
        } else {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("`{}` does not exist", dir.display()),
            ))
        }
    }
}

pub type ModuleId = Idx<ModuleData>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleData {
    pub name: ModuleName,
    /// The file path, including the root.
    pub file: PathBuf,
}

#[derive(Debug, Default, Clone)]
pub struct ModuleMap {
    modules: Arena<ModuleData>,
    /// By the internal name: a dependency's modules are under a root name unique in the map.
    by_name: FxHashMap<ModuleName, ModuleId>,
    /// Core modules bundled with the compiler ([`ModuleMap::add_core`]).
    core: Vec<ModuleId>,
    /// Pomes the project depends on, directly or not ([`ModuleMap::add_package`]).
    packages: Vec<Package>,
    package_of: FxHashMap<ModuleId, PackageId>,
    /// The root names the project's `[dependencies]` give their Pomes.
    project_dependencies: FxHashMap<SmolStr, PackageId>,
    /// The root names the project's `[dev-dependencies]` give their Pomes. Only the test root
    /// sees them (spec 4.7).
    project_dev_dependencies: FxHashMap<SmolStr, PackageId>,
    /// The project's modules under the test root ([`ModuleMap::add_test_root`]).
    tests: Vec<ModuleId>,
}

/// Index of a dependency in [`ModuleMap`].
pub type PackageId = usize;

/// A Pome the project depends on (spec 4.7). Its modules import each other by their names inside
/// the Pome (`Router`), and import their own dependencies by the root names they chose.
#[derive(Debug, Clone)]
pub struct Package {
    /// The source path, for diagnostics.
    pub name: SmolStr,
    /// The root of the internal names of its modules (`Quince`, `Quince.Router`).
    pub root: SmolStr,
    /// Module names inside the Pome (`Router`, `Lib`) → modules.
    local: FxHashMap<ModuleName, ModuleId>,
    /// `src/lib.emel`: the module the root name itself names.
    lib: Option<ModuleId>,
    dependencies: FxHashMap<SmolStr, PackageId>,
    /// Whether the project allows its `@external` (`[external] allow`, spec 4.7).
    pub external_allowed: bool,
}

impl Package {
    /// The names of its modules inside the Pome (`Router`, `Lib`).
    pub fn local_names(&self) -> impl Iterator<Item = &ModuleName> {
        self.local.keys()
    }
}

/// The library root of a Pome (`src/lib.emel`) and its entry (`src/main.emel`), by their names
/// inside the Pome.
const LIB: &str = "Lib";
const MAIN: &str = "Main";

impl ModuleMap {
    /// Looks up a module by its internal name. Import paths go through [`ModuleMap::lookup_from`].
    pub fn lookup(&self, name: &ModuleName) -> Option<ModuleId> {
        self.by_name.get(name).copied()
    }

    /// Looks up the module an import path in `importer` names (spec 4.7): a module of the
    /// importer's own Pome, else a module of one of its dependencies under the root name it gave
    /// them, else a bundled core module. Dependencies of dependencies are not visible.
    ///
    /// The project's test root modules and `[dev-dependencies]` are visible only from the test
    /// root (spec 13.1); [`ModuleMap::is_test_only`] tells such an import apart.
    pub fn lookup_from(&self, importer: ModuleId, name: &ModuleName) -> Option<ModuleId> {
        let package = self.package_of.get(&importer).map(|&p| &self.packages[p]);
        let from_tests = self.is_test(importer);
        let own = match package {
            Some(p) => p.local.get(name).copied(),
            None => self
                .lookup(name)
                .filter(|&id| !self.package_of.contains_key(&id))
                .filter(|&id| from_tests || !self.is_test(id)),
        };
        if own.is_some() {
            return own;
        }
        let (first, rest) = name.segments().split_first()?;
        let dependency = match package {
            Some(p) => p.dependencies.get(first),
            None => self.project_dependencies.get(first).or_else(|| {
                from_tests
                    .then(|| self.project_dev_dependencies.get(first))
                    .flatten()
            }),
        };
        if let Some(&dependency) = dependency {
            let dependency = &self.packages[dependency];
            return match rest {
                [] => dependency.lib,
                [only] if only == LIB || only == MAIN => None,
                _ => dependency
                    .local
                    .get(&ModuleName::new(rest.iter().cloned()))
                    .copied(),
            };
        }
        self.lookup(name).filter(|&id| self.is_core(id))
    }

    /// Whether an import that [`ModuleMap::lookup_from`] does not find names a module only the
    /// test root may import: a module of the test root, or a `[dev-dependencies]` Pome (E0239).
    pub fn is_test_only(&self, importer: ModuleId, name: &ModuleName) -> bool {
        if self.package_of.contains_key(&importer) || self.is_test(importer) {
            return false;
        }
        self.lookup(name).is_some_and(|id| self.is_test(id))
            || name
                .segments()
                .first()
                .is_some_and(|first| self.project_dev_dependencies.contains_key(first))
    }

    /// Whether a module is one of the project's modules under the test root.
    pub fn is_test(&self, id: ModuleId) -> bool {
        self.tests.contains(&id)
    }

    /// Adds the project's modules under the test root `root` (spec 4.1). They share the module
    /// names with the source root: a name in both is a duplicate, and `main.emel` and `lib.emel`
    /// are reserved at the top.
    pub fn add_test_root(&mut self, fs: &dyn SourceFs, root: &Path) -> Vec<Diagnostic> {
        let mut collector = Collector {
            fs,
            map: std::mem::take(self),
            diagnostics: Vec::new(),
            by_lowercase: FxHashMap::default(),
            test_root: true,
            inserted: Vec::new(),
        };
        collector.walk(root, &mut Vec::new());
        *self = collector.map;
        self.tests.extend(collector.inserted);
        collector.diagnostics
    }

    /// The dependency a module belongs to; `None` for the project's and core modules.
    pub fn package_of(&self, id: ModuleId) -> Option<&Package> {
        self.package_of.get(&id).map(|&p| &self.packages[p])
    }

    pub fn package(&self, id: PackageId) -> &Package {
        &self.packages[id]
    }

    /// Allows the `@external` of a dependency (`[external] allow`, spec 4.7).
    pub fn allow_external(&mut self, id: PackageId) {
        self.packages[id].external_allowed = true;
    }

    /// Whether a module belongs to a dependency, not to the project or core.
    pub fn is_dependency(&self, id: ModuleId) -> bool {
        self.package_of.contains_key(&id)
    }

    /// Adds the modules of a dependency whose source root is `root` (its `src`). Their internal
    /// names are under `preferred`, or under `preferred` with a number when another module already
    /// starts with that name. `src/main.emel` is not used (spec 4.7).
    pub fn add_package(
        &mut self,
        fs: &dyn SourceFs,
        name: SmolStr,
        preferred: SmolStr,
        root: &Path,
    ) -> (PackageId, Vec<Diagnostic>) {
        let (collected, diagnostics) = collect_modules(fs, root);
        let taken = |map: &ModuleMap, r: &str| map.by_name.keys().any(|n| n.segments()[0] == r);
        let mut internal = preferred.clone();
        let mut n = 2;
        while taken(self, &internal) {
            internal = SmolStr::new(format!("{preferred}{n}"));
            n += 1;
        }
        let id = self.packages.len();
        let mut package = Package {
            name,
            root: internal.clone(),
            local: FxHashMap::default(),
            lib: None,
            dependencies: FxHashMap::default(),
            external_allowed: false,
        };
        for (_, data) in collected.modules.iter() {
            let local = data.name.clone();
            let is_lib = local.segments() == [SmolStr::new(LIB)];
            if local.segments() == [SmolStr::new(MAIN)] {
                continue;
            }
            let internal_name = if is_lib {
                ModuleName::new([internal.clone()])
            } else {
                ModuleName::new(
                    std::iter::once(internal.clone()).chain(local.segments().iter().cloned()),
                )
            };
            let module = self.insert(ModuleData {
                name: internal_name,
                file: data.file.clone(),
            });
            self.package_of.insert(module, id);
            package.local.insert(local, module);
            if is_lib {
                package.lib = Some(module);
            }
        }
        self.packages.push(package);
        (id, diagnostics)
    }

    /// The root names a Pome (`None` for the project) gives its dependencies.
    pub fn set_dependencies(
        &mut self,
        package: Option<PackageId>,
        dependencies: impl IntoIterator<Item = (SmolStr, PackageId)>,
    ) {
        let table = match package {
            Some(p) => &mut self.packages[p].dependencies,
            None => &mut self.project_dependencies,
        };
        table.extend(dependencies);
    }

    /// The root names the project's `[dev-dependencies]` give their Pomes.
    pub fn set_dev_dependencies(
        &mut self,
        dependencies: impl IntoIterator<Item = (SmolStr, PackageId)>,
    ) {
        self.project_dev_dependencies.extend(dependencies);
    }

    pub fn len(&self) -> usize {
        self.modules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    /// Ordered by file path.
    pub fn iter(&self) -> impl Iterator<Item = (ModuleId, &ModuleData)> {
        self.modules.iter()
    }

    /// Adds a core module bundled with the compiler (`List`, `Option`, `Prelude`, ...). `file` is
    /// a placeholder path for the source table.
    ///
    /// If a source module has the same name, the name points to the core module instead, and a
    /// reserved-name diagnostic is returned for the hidden source module.
    pub fn add_core(&mut self, name: ModuleName, file: PathBuf) -> (ModuleId, Option<Diagnostic>) {
        let shadowed = self.lookup(&name).map(|id| Diagnostic {
            file: self.modules[id].file.clone(),
            range: None,
            kind: DiagnosticKind::ReservedModuleName { name: name.clone() },
        });
        let id = self.insert(ModuleData { name, file });
        self.core.push(id);
        (id, shadowed)
    }

    pub fn is_core(&self, id: ModuleId) -> bool {
        self.core.contains(&id)
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

/// Collects all diagnostics.
pub fn collect_modules(fs: &dyn SourceFs, root: &Path) -> (ModuleMap, Vec<Diagnostic>) {
    let mut collector = Collector {
        fs,
        map: ModuleMap::default(),
        diagnostics: Vec::new(),
        by_lowercase: FxHashMap::default(),
        test_root: false,
        inserted: Vec::new(),
    };
    collector.walk(root, &mut Vec::new());
    (collector.map, collector.diagnostics)
}

struct Collector<'a> {
    fs: &'a dyn SourceFs,
    map: ModuleMap,
    diagnostics: Vec<Diagnostic>,
    /// Lowercased path → the first file seen. Finds files that differ only in case.
    by_lowercase: FxHashMap<String, PathBuf>,
    /// Walking the test root: `main.emel` and `lib.emel` are reserved at the top.
    test_root: bool,
    inserted: Vec<ModuleId>,
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
        // Sort by name to fix the order of the module map and of the diagnostics.
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        for entry in entries {
            let path = dir.join(&entry.name);
            // Skip hidden files and directories (editor temp files, .git, ...).
            if entry.name.as_encoded_bytes().starts_with(b".") {
                continue;
            }
            if entry.is_dir {
                let name = entry.name.to_string_lossy();
                match to_type_name(&name) {
                    // `std/` at the root is reserved for the bundled std modules (spec 4.1, 15.2).
                    Ok(segment) if prefix.is_empty() && segment == STD => self.error(
                        &path,
                        DiagnosticKind::ReservedModuleName {
                            name: ModuleName::new([segment]),
                        },
                    ),
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
                // Also report it if it collides, up to case, with a correctly named file: on a
                // case-insensitive file system they are the same file.
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

        let reserved_in_tests = self.test_root && (name.last() == MAIN || name.last() == LIB);
        if prefix.is_empty() && (name.last() == PRELUDE || name.last() == STD || reserved_in_tests)
        {
            self.error(&path, DiagnosticKind::ReservedModuleName { name });
            return;
        }
        if let Some(first) = self.map.lookup(&name).map(|id| self.map[id].file.clone()) {
            self.error(&path, DiagnosticKind::DuplicateModule { name, first });
            return;
        }
        // A wrongly named file seen earlier differs from this one only in case.
        if let Some(first) = first_same_case {
            self.error(
                &first,
                DiagnosticKind::DuplicateModule {
                    name: name.clone(),
                    first: path.clone(),
                },
            );
        }
        let id = self.map.insert(ModuleData { name, file: path });
        self.inserted.push(id);
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
                "src/Foo.emel: file name `Foo` cannot be a module name: not lower snake_case",
                "src/HTTP: directory name `HTTP` cannot be a module name: not lower snake_case",
                "src/_x.emel: file name `_x` cannot be a module name: not lower snake_case",
                "src/a.emel: file name `a` cannot be a module name: the converted name `A` is not a valid type name",
                "src/a__b.emel: file name `a__b` cannot be a module name: not lower snake_case",
                "src/ok/a-b.emel: file name `a-b` cannot be a module name: not lower snake_case",
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
                "src/Json_Codec.emel: file name `Json_Codec` cannot be a module name: not lower snake_case",
                "src/UTIL.emel: file name `UTIL` cannot be a module name: not lower snake_case",
                "src/ab_2.emel: duplicate module `Ab2` (already defined by `src/ab2.emel`)",
                "src/Json_Codec.emel: duplicate module `JsonCodec` (already defined by `src/json_codec.emel`)",
                "src/UTIL.emel: duplicate module `Util` (already defined by `src/util.emel`)",
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
        let (map, diagnostics) = collect(&["src/prelude.emel", "src/app/prelude.emel"]);
        assert_eq!(
            diagnostics,
            ["src/prelude.emel: module name `Prelude` is reserved"]
        );
        assert_eq!(names(&map), ["App.Prelude = src/app/prelude.emel"]);
    }

    #[test]
    fn std_is_reserved() {
        // `std.emel` and `std/` at the root are reserved (spec 4.1); `Std` elsewhere is not.
        let (map, diagnostics) = collect(&[
            "src/std.emel",
            "src/std/console.emel",
            "src/app/std.emel",
            "src/console.emel",
        ]);
        assert_eq!(
            diagnostics,
            [
                "src/std: module name `Std` is reserved",
                "src/std.emel: module name `Std` is reserved",
            ]
        );
        assert_eq!(
            names(&map),
            ["App.Std = src/app/std.emel", "Console = src/console.emel"]
        );
    }

    #[test]
    fn missing_root() {
        let (map, diagnostics) = collect(&["lib/main.emel"]);
        assert!(map.is_empty());
        assert_eq!(
            diagnostics,
            ["src: cannot read directory: `src` does not exist"]
        );
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

    // The project depends on quince (as `Quince`); quince depends on http (as `Web`). Both have
    // a module named `Util`.
    fn packages() -> (ModuleMap, ModuleId, ModuleId, ModuleId) {
        let fs = MemoryFs::new([
            "src/main.emel",
            "src/util.emel",
            "q/src/lib.emel",
            "q/src/main.emel",
            "q/src/router.emel",
            "q/src/util.emel",
            "h/src/lib.emel",
            "h/src/client.emel",
        ]);
        let (mut map, _) = collect_modules(&fs, Path::new("src"));
        let (http, _) = map.add_package(
            &fs,
            "github.com/acme/http".into(),
            "Util".into(),
            Path::new("h/src"),
        );
        let (quince, _) = map.add_package(
            &fs,
            "github.com/acme/quince".into(),
            "Quince".into(),
            Path::new("q/src"),
        );
        map.set_dependencies(None, [(SmolStr::new("Quince"), quince)]);
        map.set_dependencies(Some(quince), [(SmolStr::new("Web"), http)]);
        let main = map.lookup(&ModuleName::parse("Main")).unwrap();
        let router = map.lookup(&ModuleName::parse("Quince.Router")).unwrap();
        let client = map.lookup(&ModuleName::parse("Util2.Client")).unwrap();
        (map, main, router, client)
    }

    #[test]
    fn looks_up_imports_from_the_importers_pome() {
        let (map, main, router, client) = packages();
        let file = |id: Option<ModuleId>| id.map(|id| map[id].file.display().to_string());
        let from = |importer, name| file(map.lookup_from(importer, &ModuleName::parse(name)));
        // The project sees its own modules and its dependency under the root name.
        assert_eq!(from(main, "Util").as_deref(), Some("src/util.emel"));
        assert_eq!(from(main, "Quince").as_deref(), Some("q/src/lib.emel"));
        assert_eq!(
            from(main, "Quince.Router").as_deref(),
            Some("q/src/router.emel")
        );
        // Not quince's entry, its `Lib` by name, or the dependency of the dependency.
        assert_eq!(from(main, "Quince.Main"), None);
        assert_eq!(from(main, "Quince.Lib"), None);
        assert_eq!(from(main, "Web.Client"), None);
        assert_eq!(from(main, "Util2.Client"), None);
        // Inside quince, `Util` is quince's own, and http is `Web`.
        assert_eq!(from(router, "Util").as_deref(), Some("q/src/util.emel"));
        assert_eq!(from(router, "Lib").as_deref(), Some("q/src/lib.emel"));
        assert_eq!(
            from(router, "Web.Client").as_deref(),
            Some("h/src/client.emel")
        );
        assert_eq!(from(router, "Quince"), None);
        // Inside http, its own modules.
        assert_eq!(from(client, "Lib").as_deref(), Some("h/src/lib.emel"));
        assert!(map.is_dependency(router) && !map.is_dependency(main));
        assert_eq!(map.package_of(client).unwrap().root, "Util2");
    }

    #[test]
    fn the_test_root_and_dev_dependencies_are_seen_only_from_tests() {
        let fs = MemoryFs::new([
            "src/main.emel",
            "src/sort.emel",
            "test/support.emel",
            "test/sort_test.emel",
            "test/sort.emel",
            "test/lib.emel",
            "b/src/lib.emel",
            "b/src/gen.emel",
        ]);
        let (mut map, _) = collect_modules(&fs, Path::new("src"));
        let found = map.add_test_root(&fs, Path::new("test"));
        let found: Vec<String> = found
            .iter()
            .map(|d| format!("{}: {d}", d.file.display()))
            .collect();
        assert_eq!(
            found,
            [
                "test/lib.emel: module name `Lib` is reserved",
                "test/sort.emel: duplicate module `Sort` (already defined by `src/sort.emel`)",
            ]
        );
        let (brix, _) = map.add_package(
            &fs,
            "github.com/emela-lang/brix".into(),
            "Brix".into(),
            Path::new("b/src"),
        );
        map.set_dev_dependencies([(SmolStr::new("Brix"), brix)]);
        let id = |name: &str| map.lookup(&ModuleName::parse(name)).unwrap();
        let (main, test) = (id("Main"), id("SortTest"));
        let sees = |importer, name: &str| {
            map.lookup_from(importer, &ModuleName::parse(name))
                .is_some()
        };
        assert!(sees(test, "Sort") && sees(test, "Support") && sees(test, "Brix.Gen"));
        assert!(sees(main, "Sort") && !sees(main, "Support") && !sees(main, "Brix.Gen"));
        let test_only = |name: &str| map.is_test_only(main, &ModuleName::parse(name));
        assert!(test_only("Support") && test_only("Brix") && test_only("Brix.Gen"));
        assert!(!test_only("Nothing") && !map.is_test_only(test, &ModuleName::parse("Nothing")));
    }
}
