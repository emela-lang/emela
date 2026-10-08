//! JS を出す段の出力と，その書き出し．

use std::path::{Component, Path, PathBuf};

use crate::diagnostic::{Diagnostic, Location};

/// 出力する1ファイル．`path` は出力先からの相対パス．
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

/// JS を出す段が返すもの．`entry` は `files` のどれか（node で実行する `.mjs`）．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsOutput {
    pub files: Vec<OutputFile>,
    pub entry: PathBuf,
}

/// `out_dir` の下に書き出し，エントリのパスを返す．前の出力は消さずに上書きする．
pub fn write_output(out_dir: &Path, output: &JsOutput) -> Result<PathBuf, Diagnostic> {
    for file in &output.files {
        if !is_plain_relative(&file.path) {
            return Err(Diagnostic::error(format!(
                "出力のパス `{}` が出力先の外を指している",
                file.path.display()
            )));
        }
    }
    if !output.files.iter().any(|file| file.path == output.entry) {
        return Err(Diagnostic::error(format!(
            "エントリ `{}` が出力に含まれていない",
            output.entry.display()
        )));
    }
    for file in &output.files {
        let path = out_dir.join(&file.path);
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, &file.contents)
        };
        write().map_err(|err| {
            Diagnostic::error(format!("出力を書けない: {err}")).at(Location::Path(path.clone()))
        })?;
    }
    Ok(out_dir.join(&output.entry))
}

/// `..` や絶対パスを含まない相対パスか．
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
        let output = JsOutput {
            files: vec![OutputFile::new("lib.mjs", "")],
            entry: "main.mjs".into(),
        };
        let err = write_output(Path::new("/nonexistent"), &output).unwrap_err();
        assert_eq!(err.message, "エントリ `main.mjs` が出力に含まれていない");
    }
}
