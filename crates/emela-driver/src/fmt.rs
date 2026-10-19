//! `emela fmt`: decides what to format, formats it with emela-fmt, and collects syntax
//! diagnostics.
//!
//! Targets are decided as in spec 4.1 ([`Input::resolve`]). Writing files is passed in by the
//! caller.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use emela_fmt::FormatError;
use emela_resolve::EXTENSION;

use crate::code;
use crate::diagnostic::{Diagnostic, Location};
use crate::pipeline::{Input, TEST_DIR};
use crate::render::{render_one, summary};
use crate::source::{FileSystem, SourceDb};

/// The path passed to read from stdin.
pub const STDIN_PATH: &str = "-";
/// The path shown in diagnostics for stdin source.
pub const STDIN_DISPLAY: &str = "<stdin>";

/// Writes formatted contents; the arguments are the file's absolute path and contents.
pub type WriteFile<'a> = &'a mut dyn FnMut(&Path, &str) -> io::Result<()>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FmtTarget {
    pub path: PathBuf,
    /// The path shown in diagnostics and listings, relative to the project (or to the file's
    /// directory for a single file).
    pub display: PathBuf,
}

/// Diagnostics of a file that could not be formatted. Paths are relative to a per-file base, so
/// each is paired with its own source table.
#[derive(Debug, Default)]
pub struct FmtFailure {
    pub sources: SourceDb,
    pub diagnostics: Vec<Diagnostic>,
}

impl FmtFailure {
    fn new(diagnostic: Diagnostic) -> Self {
        FmtFailure {
            sources: SourceDb::new(),
            diagnostics: vec![diagnostic],
        }
    }
}

#[derive(Debug, Default)]
pub struct FmtReport {
    /// Files whose contents change (display paths). Files that failed to write are excluded.
    pub changed: Vec<PathBuf>,
    pub failures: Vec<FmtFailure>,
}

impl FmtReport {
    pub fn has_errors(&self) -> bool {
        self.failures
            .iter()
            .any(|failure| failure.diagnostics.iter().any(Diagnostic::is_error))
    }

    /// Lists all diagnostics and summarizes the counts, like [`crate::render`].
    pub fn render(&self, color: bool) -> String {
        let mut out = String::new();
        let mut all = Vec::new();
        for failure in &self.failures {
            for diagnostic in &failure.diagnostics {
                if !all.is_empty() {
                    out.push('\n');
                }
                out.push_str(&render_one(diagnostic, &failure.sources, color));
                all.push(diagnostic.clone());
            }
        }
        if let Some(summary) = summary(&all) {
            out.push('\n');
            out.push_str(&summary);
            out.push('\n');
        }
        out
    }
}

/// Decides the `.emel` files to format from the given paths. The diagnostics are for paths that
/// could not be resolved.
///
/// - Files follow the same rules as [`Input::resolve`] (E0208 outside the project's source
///   root; a single file without `Pome.toml`). A single file formats only itself and does not
///   follow imports.
/// - Directories need `Pome.toml` (E0207). The project directory means everything under the
///   source root, and a directory inside the source root means everything under it. Elsewhere is
///   E0208.
pub fn fmt_targets(fs: &dyn FileSystem, paths: &[PathBuf]) -> (Vec<FmtTarget>, Vec<FmtFailure>) {
    let mut targets = BTreeSet::new();
    let mut failures = Vec::new();
    for path in paths {
        let input = match Input::resolve(fs, path) {
            Ok(input) => input,
            Err(diagnostic) => {
                failures.push(FmtFailure::new(diagnostic));
                continue;
            }
        };
        let target = |file: PathBuf| FmtTarget {
            display: display_path(&input.base, &file),
            path: file,
        };
        let absolute = fs
            .absolute(path)
            .expect("Input::resolve made the path absolute");
        if fs.is_file(&absolute) {
            targets.insert(target(absolute));
            continue;
        }
        let dir = absolute;
        // `Input::resolve` has rejected directories outside the source and test roots, so `dir`
        // is under one of them or the project directory, which means both (the test root only
        // if it exists).
        let tests = input.base.join(TEST_DIR);
        let walk_from = if dir.starts_with(&input.root) || dir.starts_with(&tests) {
            vec![dir]
        } else if fs.is_dir(&tests) {
            vec![input.root.clone(), tests]
        } else {
            vec![input.root.clone()]
        };
        for from in walk_from {
            let mut files = Vec::new();
            if let Err((dir, err)) = walk(fs, &from, &mut files) {
                failures.push(FmtFailure::new(
                    Diagnostic::error(format!("cannot read directory: {err}"))
                        .with_code(code::CANNOT_READ)
                        .at(Location::Path(display_path(&input.base, &dir))),
                ));
            }
            targets.extend(files.into_iter().map(target));
        }
    }
    (targets.into_iter().collect(), failures)
}

/// Collects every `.emel` under `dir`, skipping files and directories starting with `.`
/// (spec 4.1). Files whose names are not valid module names are formatted too, since formatting
/// ignores names.
fn walk(
    fs: &dyn FileSystem,
    dir: &Path,
    out: &mut Vec<PathBuf>,
) -> Result<(), (PathBuf, io::Error)> {
    let mut entries = fs.read_dir(dir).map_err(|err| (dir.to_owned(), err))?;
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    for entry in entries {
        if entry.name.to_string_lossy().starts_with('.') {
            continue;
        }
        let path = dir.join(&entry.name);
        if entry.is_dir {
            walk(fs, &path, out)?;
        } else if path.extension() == Some(OsStr::new(EXTENSION)) {
            out.push(path);
        }
    }
    Ok(())
}

fn display_path(base: &Path, path: &Path) -> PathBuf {
    match path.strip_prefix(base) {
        Ok(relative) if !relative.as_os_str().is_empty() => relative.to_owned(),
        _ => path.to_owned(),
    }
}

/// `Ok(None)` if the contents do not change. `display` is the path shown in diagnostics.
pub fn format_source(display: &Path, text: &str) -> Result<Option<String>, FmtFailure> {
    match emela_fmt::format(text) {
        Ok(formatted) if formatted == text => Ok(None),
        Ok(formatted) => Ok(Some(formatted)),
        Err(err) => {
            let mut sources = SourceDb::new();
            let file = sources.add(display, text);
            let diagnostics = match err {
                FormatError::Syntax(diagnostics) => diagnostics
                    .iter()
                    .map(|diagnostic| Diagnostic::from_syntax(file, diagnostic))
                    .collect(),
                // A formatter bug; leave the input untouched.
                FormatError::Internal(message) => vec![
                    Diagnostic::error(format!(
                        "internal error: cannot format this file: {message}"
                    ))
                    .at(Location::File(file))
                    .with_note("the file was left unchanged"),
                ],
            };
            Err(FmtFailure {
                sources,
                diagnostics,
            })
        }
    }
}

/// Formats the `.emel` files of the given paths. With `write` set to `None`, nothing is written
/// (`--check`).
///
/// `write` is called only for files whose contents change. Files with syntax errors are skipped
/// after collecting their diagnostics.
pub fn fmt(fs: &dyn FileSystem, paths: &[PathBuf], mut write: Option<WriteFile<'_>>) -> FmtReport {
    let (targets, mut failures) = fmt_targets(fs, paths);
    let mut changed = Vec::new();
    for target in targets {
        let text = match fs.read_to_string(&target.path) {
            Ok(text) => text,
            Err(err) => {
                failures.push(FmtFailure::new(
                    Diagnostic::error(format!("cannot read file: {err}"))
                        .with_code(code::CANNOT_READ)
                        .at(Location::Path(target.display)),
                ));
                continue;
            }
        };
        let formatted = match format_source(&target.display, &text) {
            Ok(Some(formatted)) => formatted,
            Ok(None) => continue,
            Err(failure) => {
                failures.push(failure);
                continue;
            }
        };
        if let Some(write) = write.as_mut()
            && let Err(err) = write(&target.path, &formatted)
        {
            failures.push(FmtFailure::new(
                Diagnostic::error(format!("cannot write file: {err}"))
                    .with_code(code::CANNOT_WRITE_OUTPUT)
                    .at(Location::Path(target.display)),
            ));
            continue;
        }
        changed.push(target.display);
    }
    FmtReport { changed, failures }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MemoryFiles;

    const MESSY: &str = "fn main( ) {1}\n";
    const CLEAN: &str = "fn main() { 1 }\n";

    fn targets(files: &[(&str, &str)], paths: &[&str]) -> (Vec<String>, Vec<String>) {
        let fs = MemoryFiles::new(files.iter().copied());
        let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        let (targets, failures) = fmt_targets(&fs, &paths);
        (
            targets
                .iter()
                .map(|t| t.display.display().to_string())
                .collect(),
            failures
                .iter()
                .flat_map(|f| &f.diagnostics)
                .map(|d| format!("{}: {}", d.code.unwrap_or("-"), d.message))
                .collect(),
        )
    }

    #[test]
    fn project_directory_formats_source_root() {
        let files = [
            ("/app/Pome.toml", ""),
            ("/app/src/main.emel", ""),
            ("/app/src/http/client.emel", ""),
            ("/app/src/Bad.emel", ""),
            ("/app/src/.hidden.emel", ""),
            ("/app/src/.git/x.emel", ""),
            ("/app/src/notes.txt", ""),
            ("/app/scripts/x.emel", ""),
        ];
        let all = vec![
            "src/Bad.emel".to_owned(),
            "src/http/client.emel".to_owned(),
            "src/main.emel".to_owned(),
        ];
        assert_eq!(targets(&files, &["/app"]), (all.clone(), vec![]));
        let with_tests: Vec<(&str, &str)> = files
            .iter()
            .copied()
            .chain([("/app/test/sort_test.emel", "")])
            .collect();
        let mut both = all.clone();
        both.push("test/sort_test.emel".to_owned());
        assert_eq!(targets(&with_tests, &["/app"]), (both, vec![]));
        assert_eq!(
            targets(&with_tests, &["/app/test"]),
            (vec!["test/sort_test.emel".to_owned()], vec![])
        );
        assert_eq!(targets(&files, &["/app/src"]), (all, vec![]));
        assert_eq!(
            targets(&files, &["/app/src/http"]),
            (vec!["src/http/client.emel".to_owned()], vec![])
        );
        // Overlapping paths count once.
        assert_eq!(
            targets(&files, &["/app/src/http", "/app/src/http/client.emel"]).0,
            vec!["src/http/client.emel".to_owned()]
        );
    }

    #[test]
    fn outside_source_root() {
        let files = [
            ("/app/Pome.toml", ""),
            ("/app/src/main.emel", ""),
            ("/app/scripts/x.emel", ""),
        ];
        let (found, errors) = targets(&files, &["/app/scripts", "/app/scripts/x.emel"]);
        assert!(found.is_empty());
        assert_eq!(errors.len(), 2);
        assert!(
            errors.iter().all(|e| e.starts_with("E0208: ")),
            "{errors:?}"
        );
    }

    #[test]
    fn single_file_and_missing_paths() {
        let files = [("/tmp/a/main.emel", ""), ("/tmp/a/util.emel", "")];
        // A single file formats only itself; display paths are relative to its directory.
        assert_eq!(
            targets(&files, &["/tmp/a/main.emel"]),
            (vec!["main.emel".to_owned()], vec![])
        );
        let (found, errors) = targets(&files, &["/tmp/a", "/nowhere"]);
        assert!(found.is_empty());
        assert_eq!(errors.len(), 2);
        assert!(errors[0].starts_with("E0207: "), "{errors:?}");
        assert!(errors[1].starts_with("E0901: "), "{errors:?}");
    }

    #[test]
    fn writes_only_changed_files_and_keeps_going_after_syntax_errors() {
        let fs = MemoryFiles::new([
            ("/app/Pome.toml", ""),
            ("/app/src/a.emel", MESSY),
            ("/app/src/b.emel", "fn broken( {\n"),
            ("/app/src/c.emel", CLEAN),
            ("/app/src/d.emel", MESSY),
        ]);
        let mut written = Vec::new();
        let mut write = |path: &Path, text: &str| {
            written.push((path.to_owned(), text.to_owned()));
            Ok(())
        };
        let report = fmt(&fs, &[PathBuf::from("/app")], Some(&mut write));
        assert_eq!(
            written,
            vec![
                (PathBuf::from("/app/src/a.emel"), CLEAN.to_owned()),
                (PathBuf::from("/app/src/d.emel"), CLEAN.to_owned()),
            ]
        );
        assert_eq!(
            report.changed,
            vec![PathBuf::from("src/a.emel"), PathBuf::from("src/d.emel")]
        );
        assert!(report.has_errors());
        let rendered = report.render(false);
        assert!(rendered.starts_with("error[E0"), "{rendered}");
        assert!(rendered.contains("src/b.emel"), "{rendered}");
        assert!(
            rendered.ends_with(" error\n") || rendered.ends_with(" errors\n"),
            "{rendered}"
        );

        // --check writes nothing and returns the same list.
        let report = fmt(&fs, &[PathBuf::from("/app")], None);
        assert_eq!(report.changed.len(), 2);
    }

    #[test]
    fn write_errors_are_reported() {
        let fs = MemoryFiles::new([("/app/Pome.toml", ""), ("/app/src/a.emel", MESSY)]);
        let mut write = |_: &Path, _: &str| Err(io::Error::other("read-only"));
        let report = fmt(&fs, &[PathBuf::from("/app")], Some(&mut write));
        assert!(report.changed.is_empty());
        assert_eq!(
            report.render(false),
            "error[E0903]: cannot write file: read-only\n   ─[ src/a.emel ]\n\n1 error\n"
        );
    }

    #[test]
    fn format_source_reports_unchanged() {
        assert_eq!(format_source(Path::new("x"), CLEAN).unwrap(), None);
        assert_eq!(
            format_source(Path::new("x"), MESSY).unwrap(),
            Some(CLEAN.to_owned())
        );
        let failure = format_source(Path::new(STDIN_DISPLAY), "const X = $\n").unwrap_err();
        assert_eq!(failure.diagnostics[0].code, Some("E0105"));
    }
}
