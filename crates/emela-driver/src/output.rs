//! What a [`crate::Backend`] outputs (JS or C), and writing it out.

use std::path::{Component, Path, PathBuf};

use crate::code;
use crate::diagnostic::{Diagnostic, Location};

/// `path` is relative to the output directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputFile {
    pub path: PathBuf,
    pub contents: String,
}

impl OutputFile {
    pub fn new(path: impl Into<PathBuf>, contents: impl Into<String>) -> Self {
        OutputFile {
            path: path.into(),
            contents: contents.into(),
        }
    }
}

/// `entry` is one of `files`: the `.mjs` run on node for JS, `main.c` for C (see [`crate::Backend::finish`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildOutput {
    pub files: Vec<OutputFile>,
    pub entry: PathBuf,
}

/// Writes under `out_dir` and returns the entry's path. Earlier output is overwritten, not
/// removed.
pub fn write_output(out_dir: &Path, output: &BuildOutput) -> Result<PathBuf, Diagnostic> {
    for file in &output.files {
        if !is_plain_relative(&file.path) {
            return Err(Diagnostic::error(format!(
                "internal error: the JS backend wrote `{}` outside the output directory",
                file.path.display()
            ))
            .with_code(code::INVALID_JS_OUTPUT));
        }
    }
    if !output.files.iter().any(|file| file.path == output.entry) {
        return Err(Diagnostic::error(format!(
            "internal error: the JS backend did not output the entry `{}`",
            output.entry.display()
        ))
        .with_code(code::INVALID_JS_OUTPUT));
    }
    write_files(out_dir, &output.files)?;
    Ok(out_dir.join(&output.entry))
}

/// Writes each file under `out_dir`, making directories as needed. Earlier output is
/// overwritten, not removed. The paths must be plain relative paths.
pub fn write_files(out_dir: &Path, files: &[OutputFile]) -> Result<(), Diagnostic> {
    for file in files {
        let path = out_dir.join(&file.path);
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, &file.contents)
        };
        write().map_err(|err| {
            Diagnostic::error(format!("cannot write output: {err}"))
                .with_code(code::CANNOT_WRITE_OUTPUT)
                .at(Location::Path(path.clone()))
        })?;
    }
    Ok(())
}

/// A relative path with no `..` and not absolute.
fn is_plain_relative(path: &Path) -> bool {
    path.components().next().is_some()
        && path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_paths_outside() {
        for path in ["../x.mjs", "/x.mjs", "a/../../x.mjs", ""] {
            assert!(!is_plain_relative(Path::new(path)), "{path}");
        }
        for path in ["x.mjs", "a/b.mjs", "./c.mjs"] {
            assert!(is_plain_relative(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn entry_must_be_written() {
        let output = BuildOutput {
            files: vec![OutputFile::new("lib.mjs", "")],
            entry: "main.mjs".into(),
        };
        let err = write_output(Path::new("/nonexistent"), &output).unwrap_err();
        assert_eq!(
            err.message,
            "internal error: the JS backend did not output the entry `main.mjs`"
        );
    }
}
