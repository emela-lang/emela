//! The project a command works on (spec 4.1): the source root and entry decided from the path on
//! the command line, the Pomes it depends on (spec 4.7), and its default target
//! (`[build].target` in `Pome.toml`).

use std::path::{Path, PathBuf};

use crate::code;
use crate::diagnostic::{Diagnostic, Location};
use crate::experimental::Features;
use crate::source::FileSystem;
use crate::target::Target;

/// The file marking a project (spec 4.1). In 0.20 only its presence matters.
pub const PROJECT_FILE: &str = "Pome.toml";
/// The source root inside a project.
pub const SOURCE_DIR: &str = "src";
/// The test root inside a project: modules only the tests use (spec 4.1, 13.1).
pub const TEST_DIR: &str = "test";
/// The project entry, relative to the source root.
pub const ENTRY_FILE: &str = "main.emel";
/// The default JS output directory, relative to the project (or to the file's directory for a
/// single file).
pub const JS_OUT_DIR: &str = "target/emela/js";
/// The default output directory of the native target (`--target native`, `[target.native.output]`), relative like [`JS_OUT_DIR`].
pub const NATIVE_OUT_DIR: &str = "target/emela/native";
/// The default WASM output directory (`--target wasm`), relative like [`JS_OUT_DIR`].
pub const WASM_OUT_DIR: &str = "target/emela/wasm";
/// The default output directory of `emela doc`, relative like [`JS_OUT_DIR`].
pub const DOC_OUT_DIR: &str = "target/emela/doc";

/// The source root and entry decided from the command line path (spec 4.1). All paths are
/// absolute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    /// Module names are paths relative to this.
    pub root: PathBuf,
    /// `None` when a directory is passed and it has neither `src/main.emel` nor `src/lib.emel`.
    pub entry: Option<PathBuf>,
    /// A directory-selected project with `src/lib.emel` but no `src/main.emel`.
    /// Build only checks it; run rejects it even if the library defines a fn named `main`.
    /// Explicit file inputs are never library-only.
    pub library_only: bool,
    /// The project directory, or the file's directory for a single file. `target/` goes here,
    /// and diagnostic paths are relative to it.
    pub base: PathBuf,
    /// A single file without `Pome.toml`. Only the `.emel` files directly under the root are
    /// modules.
    pub single_file: bool,
    /// The Pomes the project depends on, resolved from `Pome.lock` by the caller (spec 4.7).
    pub dependencies: Dependencies,
    /// The test root to read (`<project>/test`), set by [`Input::with_tests`] for check, test and
    /// the language server. build and run leave it out (spec 13.1).
    pub tests: Option<PathBuf>,
    /// The names given by the flags `--experimental-<name>` of the command, without the prefix
    /// (`[cmd.build]`). They add to the `[experimental]` of the project's `Pome.toml`, and do not
    /// apply to dependencies. The language server has none (`[lsp.project]`).
    pub experimental: Vec<String>,
}

/// The Pomes a project depends on, directly or not (spec 4.7). The caller (emela pome) resolves
/// them from `Pome.lock` and gets their files; the pipeline adds their modules under root names.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Dependencies {
    pub pomes: Vec<DependencyPome>,
    /// The project's `[dependencies]`.
    pub project: Vec<DependencyRef>,
    /// The project's `[dev-dependencies]`, for the test root only. Empty unless the tests are
    /// read.
    pub dev: Vec<DependencyRef>,
    /// The project's `Pome.toml`, where root name errors of the project are reported.
    pub manifest: PathBuf,
    /// `[external] allow` of the project: the source paths of the Pomes whose `@external` it
    /// allows.
    pub allow_external: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyPome {
    /// The source path (`github.com/emela-lang/quince`).
    pub name: String,
    /// The directory with its `Pome.toml`; its modules are under `src`.
    pub dir: PathBuf,
    /// The last element of the source path without `/vN` (`quince`). The default root name.
    pub last_element: String,
    /// `[pome].module`: the root name its author chose.
    pub module: Option<String>,
    /// `[pome].emela`: the oldest compiler it needs.
    pub emela: Option<String>,
    /// `[pome].targets`: the targets it runs on; `None` for all of them.
    pub targets: Option<Vec<String>>,
    /// The locked version (`v0.2.0`), or `replace` for a `[replace]` directory. Names its
    /// directory in the output (spec 12.1).
    pub version: String,
    /// Its `[dependencies]`.
    pub dependencies: Vec<DependencyRef>,
    /// The experimental features its own `[experimental]` enables (`[module.pome]`).
    pub experimental: Features,
}

/// An entry of `[dependencies]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyRef {
    /// Index into [`Dependencies::pomes`].
    pub pome: usize,
    /// The root name the depending side gives it (`module` of the entry).
    pub module: Option<String>,
}

impl Input {
    /// Makes the given path absolute and walks up its ancestors; the first `Pome.toml` decides
    /// the project.
    ///
    /// - A directory without `Pome.toml` is an error. The entry is `<project>/src/main.emel`,
    ///   falling back to `src/lib.emel` for a library-only project.
    /// - A file without `Pome.toml` is a single file, rooted at the file's directory.
    /// - With `Pome.toml`, a file outside `<project>/src` and `<project>/test`, or a directory
    ///   that is neither the project directory nor under one of them, is an error (E2006).
    pub fn resolve(fs: &dyn FileSystem, path: &Path) -> Result<Input, Diagnostic> {
        let not_found = || {
            Diagnostic::error(format!("`{}` not found", path.display()))
                .with_code(code::PATH_NOT_FOUND)
        };
        let absolute = fs.absolute(path).map_err(|_| not_found())?;
        let is_file = fs.is_file(&absolute);
        if !is_file && !fs.is_dir(&absolute) {
            return Err(not_found());
        }
        let start = if is_file {
            absolute.parent().expect("a file has a parent")
        } else {
            &absolute
        };
        let project = start
            .ancestors()
            .find(|dir| fs.is_file(&dir.join(PROJECT_FILE)))
            .map(Path::to_owned);

        match (project, is_file) {
            (Some(project), _) => {
                let root = project.join(SOURCE_DIR);
                // The project directory itself means the whole project. Any other directory
                // outside the source and test roots would be ignored, so it is rejected rather
                // than silently checking `src` instead (spec appendix B).
                let inside = absolute.starts_with(&root)
                    || absolute.starts_with(project.join(TEST_DIR))
                    || (!is_file && absolute == project);
                if !inside {
                    let kind = if is_file { "file" } else { "directory" };
                    return Err(Diagnostic::error(format!(
                        "{kind} `{}` is outside the source root `{SOURCE_DIR}`",
                        path.display()
                    ))
                    .with_code(code::OUTSIDE_SOURCE_ROOT)
                    .with_note(format!(
                        "source files of a project (the directory with {PROJECT_FILE}) go under {SOURCE_DIR}/, and those only the tests use under {TEST_DIR}/; pass the project directory, or a file or directory under one of them"
                    )));
                }
                let entry = if is_file {
                    Some(absolute)
                } else {
                    let entry = root.join(ENTRY_FILE);
                    if fs.is_file(&entry) {
                        Some(entry)
                    } else {
                        // A library-only project still has an entry module for checking and
                        // test discovery. It is not an executable entry, even if it defines
                        // a fn named `main`; `run` will report E2009.
                        let library = root.join("lib.emel");
                        fs.is_file(&library).then_some(library)
                    }
                };
                let library_only = !is_file && entry.as_deref() == Some(&root.join("lib.emel"));
                Ok(Input {
                    root,
                    entry,
                    library_only,
                    base: project,
                    single_file: false,
                    dependencies: Dependencies::default(),
                    tests: None,
                    experimental: Vec::new(),
                })
            }
            (None, true) => {
                let dir = start.to_owned();
                Ok(Input {
                    root: dir.clone(),
                    entry: Some(absolute),
                    library_only: false,
                    base: dir,
                    single_file: true,
                    dependencies: Dependencies::default(),
                    tests: None,
                    experimental: Vec::new(),
                })
            }
            (None, false) => Err(Diagnostic::error(format!(
                "no {PROJECT_FILE} found in `{}` or its parent directories",
                path.display()
            ))
            .with_code(code::NO_PROJECT_FILE)
            .with_note(format!(
                "put a {PROJECT_FILE} (it may be empty) in the project directory, or pass a single file"
            ))),
        }
    }

    /// Reads the test root too, if the project has one (check, test and the language server).
    pub fn with_tests(mut self, fs: &dyn FileSystem) -> Input {
        let dir = self.base.join(TEST_DIR);
        if !self.single_file && fs.is_dir(&dir) {
            self.tests = Some(dir);
        }
        self
    }

    pub fn default_out_dir(&self) -> PathBuf {
        self.base.join(JS_OUT_DIR)
    }

    /// The default output directory of build and run for the target.
    pub fn default_out_dir_for(&self, target: Target) -> PathBuf {
        match target {
            Target::Js => self.base.join(JS_OUT_DIR),
            Target::Native => self.base.join(NATIVE_OUT_DIR),
            Target::Wasm => self.base.join(WASM_OUT_DIR),
        }
    }

    pub fn default_doc_out_dir(&self) -> PathBuf {
        self.base.join(DOC_OUT_DIR)
    }
}

/// The project's default target, `[build].target` of its `Pome.toml`; `None` if it has none (or
/// for a single file). An unknown target is E2302. Other contents of the file are not read here.
pub fn project_target(fs: &dyn FileSystem, input: &Input) -> Result<Option<Target>, Diagnostic> {
    if input.single_file {
        return Ok(None);
    }
    let path = input.base.join(PROJECT_FILE);
    let Ok(text) = fs.read_to_string(&path) else {
        return Ok(None);
    };
    target_in(&text, &path)
}

/// `[build].target` in the text of a `Pome.toml` at `path`. A file that is not TOML, or whose
/// `target` is not a string, has no target as far as this is concerned.
fn target_in(text: &str, path: &Path) -> Result<Option<Target>, Diagnostic> {
    let Ok(table) = text.parse::<toml::Table>() else {
        return Ok(None);
    };
    let Some(name) = table
        .get("build")
        .and_then(|build| build.get("target"))
        .and_then(|target| target.as_str())
    else {
        return Ok(None);
    };
    match Target::parse(name) {
        Some(target) => Ok(Some(target)),
        None => Err(
            Diagnostic::error(format!("unknown target `{name}` in `[build].target`"))
                .with_code(code::UNKNOWN_BUILD_TARGET)
                .with_note(format!("expected one of {}", quoted(&Target::NAMES)))
                .at(Location::Path(path.to_owned())),
        ),
    }
}

/// The build target of the project that contains `path` (`[cmd.doctor]`): `[build].target` of
/// the nearest `Pome.toml` above it, else `js`. `None` outside a project.
///
/// Unlike [`Input::resolve`], any path in the project is accepted, not only the project directory
/// and the source and test roots, and no dependency is resolved, so nothing is fetched.
pub fn enclosing_project_target(
    fs: &dyn FileSystem,
    path: &Path,
) -> Result<Option<Target>, Diagnostic> {
    let not_found = || {
        Diagnostic::error(format!("`{}` not found", path.display())).with_code(code::PATH_NOT_FOUND)
    };
    let absolute = fs.absolute(path).map_err(|_| not_found())?;
    if !fs.is_file(&absolute) && !fs.is_dir(&absolute) {
        return Err(not_found());
    }
    let Some(project) = absolute
        .ancestors()
        .find(|dir| fs.is_file(&dir.join(PROJECT_FILE)))
    else {
        return Ok(None);
    };

    let manifest = project.join(PROJECT_FILE);
    let Ok(text) = fs.read_to_string(&manifest) else {
        return Ok(Some(Target::default()));
    };
    Ok(Some(target_in(&text, &manifest)?.unwrap_or_default()))
}

fn quoted(names: &[&str]) -> String {
    names
        .iter()
        .map(|n| format!("`{n}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MemoryFiles;

    fn input(files: &[&str], path: &str) -> Result<Input, Diagnostic> {
        let fs = MemoryFiles::new(files.iter().map(|f| (*f, "")));
        Input::resolve(&fs, Path::new(path))
    }

    fn project(root: &str, entry: Option<&str>, base: &str) -> Result<Input, Diagnostic> {
        Ok(Input {
            root: root.into(),
            entry: entry.map(Into::into),
            library_only: false,
            base: base.into(),
            single_file: false,
            dependencies: Dependencies::default(),
            tests: None,
            experimental: Vec::new(),
        })
    }

    fn message(result: Result<Input, Diagnostic>) -> String {
        result.unwrap_err().message
    }

    #[test]
    fn project_from_directory() {
        let files = [
            "/app/Pome.toml",
            "/app/src/main.emel",
            "/app/src/http/client.emel",
        ];
        let expected = project("/app/src", Some("/app/src/main.emel"), "/app");
        assert_eq!(input(&files, "/app"), expected);
        // Same for a relative path and one with `./` (in-memory files are rooted at `/`).
        assert_eq!(input(&files, "./app/"), expected);
        // A directory inside the project walks up to the same project.
        assert_eq!(input(&files, "/app/src/http"), expected);
        // Without an entry it is still a project (check does not require an entry).
        assert_eq!(
            input(&["/lib/Pome.toml", "/lib/src/util.emel"], "/lib"),
            project("/lib/src", None, "/lib")
        );
    }

    #[test]
    fn directory_without_pome_toml() {
        assert_eq!(
            message(input(&["/app/src/main.emel"], "/app")),
            "no Pome.toml found in `/app` or its parent directories"
        );
    }

    #[test]
    fn project_from_file() {
        let files = [
            "/app/Pome.toml",
            "/app/src/main.emel",
            "/app/src/http/client.emel",
            "/app/scripts/x.emel",
        ];
        assert_eq!(
            input(&files, "/app/src/http/client.emel"),
            project("/app/src", Some("/app/src/http/client.emel"), "/app")
        );
        assert_eq!(
            message(input(&files, "/app/scripts/x.emel")),
            "file `/app/scripts/x.emel` is outside the source root `src`"
        );
    }

    #[test]
    fn directory_outside_source_root() {
        let files = [
            "/app/Pome.toml",
            "/app/src/main.emel",
            "/app/scripts/x.emel",
        ];
        assert_eq!(
            message(input(&files, "/app/scripts")),
            "directory `/app/scripts` is outside the source root `src`"
        );
        assert_eq!(
            input(&files, "/app/scripts").unwrap_err().code,
            Some(code::OUTSIDE_SOURCE_ROOT)
        );
        // The project directory and directories under the source root still mean the whole
        // project.
        let expected = project("/app/src", Some("/app/src/main.emel"), "/app");
        assert_eq!(input(&files, "/app"), expected);
        assert_eq!(input(&files, "/app/src"), expected);
    }

    #[test]
    fn nearest_pome_toml_wins() {
        // For nested projects, the inner one wins.
        let files = [
            "/outer/Pome.toml",
            "/outer/src/inner/Pome.toml",
            "/outer/src/inner/src/main.emel",
        ];
        assert_eq!(
            input(&files, "/outer/src/inner/src/main.emel"),
            project(
                "/outer/src/inner/src",
                Some("/outer/src/inner/src/main.emel"),
                "/outer/src/inner"
            )
        );
    }

    #[test]
    fn single_file() {
        let files = ["/home/src/proj/main.emel", "/home/src/proj/util.emel"];
        // An ancestor named `src` without Pome.toml still means a single file.
        assert_eq!(
            input(&files, "/home/src/proj/main.emel"),
            Ok(Input {
                root: "/home/src/proj".into(),
                entry: Some("/home/src/proj/main.emel".into()),
                library_only: false,
                base: "/home/src/proj".into(),
                single_file: true,
                dependencies: Dependencies::default(),
                tests: None,
                experimental: Vec::new(),
            })
        );
        assert_eq!(
            message(input(&files, "/home/missing.emel")),
            "`/home/missing.emel` not found"
        );
    }

    fn target(text: &str) -> Result<Option<Target>, String> {
        target_in(text, Path::new("Pome.toml")).map_err(|d| format!("{:?} {}", d.code, d.message))
    }

    #[test]
    fn reads_build_target() {
        assert_eq!(target(""), Ok(None));
        assert_eq!(target("[pome]\nname = \"x\"\n"), Ok(None));
        assert_eq!(
            target("[build]\ntarget = \"native\"\n"),
            Ok(Some(Target::Native))
        );
        // `[target.native]`: `c` is an unknown target since 0.29.
        assert_eq!(
            target("[build]\ntarget = \"c\"\n"),
            Err("Some(\"E2302\") unknown target `c` in `[build].target`".into())
        );
        assert_eq!(target("[build]\ntarget = \"js\"\n"), Ok(Some(Target::Js)));
        assert_eq!(
            target("[build]\ntarget = \"node\"\n"),
            Err("Some(\"E2302\") unknown target `node` in `[build].target`".into())
        );
    }
}
