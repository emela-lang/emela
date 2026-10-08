//! 各段で共通の診断．字句解析，名前解決，型検査の診断をここへ写してから表示する．
//!
//! 元の診断の型はそれぞれのクレートに残し，ここでは変換だけを持つ．

use std::path::{Path, PathBuf};

use emela_resolve::{DiagnosticKind, NameError};
use emela_types::{Ty, TyCons, TypeError, TypeErrorKind};
use line_index::TextRange;

use crate::code;
use crate::source::{FileId, SourceDb};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Error,
    Warning,
}

/// ファイルの中の範囲．
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub file: FileId,
    pub range: TextRange,
}

impl Span {
    pub fn new(file: FileId, range: TextRange) -> Self {
        Span { file, range }
    }
}

/// 診断の主な位置．
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    /// ファイルの中の範囲．
    Span(Span),
    /// ファイル全体（ファイル名の誤りなど）．
    File(FileId),
    /// ソースの表にないパス（ディレクトリ，読めなかったファイル）．
    Path(PathBuf),
}

/// 主な位置とは別の場所に付ける説明．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub span: Span,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    /// エラーコード（[`crate::code`]）．まだ振っていない段（字句解析）の診断は `None`．
    pub code: Option<&'static str>,
    pub message: String,
    /// 位置がない診断（node が見つからないなど）は `None`．
    pub location: Option<Location>,
    pub labels: Vec<Label>,
    pub notes: Vec<String>,
}

impl Diagnostic {
    pub fn error(message: impl Into<String>) -> Self {
        Diagnostic::new(Severity::Error, message)
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Diagnostic::new(Severity::Warning, message)
    }

    fn new(severity: Severity, message: impl Into<String>) -> Self {
        Diagnostic {
            severity,
            code: None,
            message: message.into(),
            location: None,
            labels: Vec::new(),
            notes: Vec::new(),
        }
    }

    pub fn with_code(mut self, code: &'static str) -> Self {
        self.code = Some(code);
        self
    }

    pub fn at(mut self, location: Location) -> Self {
        self.location = Some(location);
        self
    }

    pub fn with_span(self, span: Span) -> Self {
        self.at(Location::Span(span))
    }

    pub fn with_label(mut self, span: Span, message: impl Into<String>) -> Self {
        self.labels.push(Label {
            span,
            message: message.into(),
        });
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// 字句解析と構文解析の診断．コード（E01xx）と英語の文面は emela-syntax が持つ．
    pub fn from_syntax(file: FileId, diagnostic: &emela_syntax::Diagnostic) -> Self {
        Diagnostic::error(&diagnostic.message)
            .with_code(diagnostic.code.as_str())
            .with_span(Span::new(file, diagnostic.range))
    }

    /// 名前解決の診断．パスがソースの表にあれば ID で指し，なければパスのまま持つ．
    pub fn from_resolve(diagnostic: &emela_resolve::Diagnostic, sources: &SourceDb) -> Self {
        let location = match (sources.file_id(&diagnostic.file), diagnostic.range) {
            (Some(file), Some(range)) => Location::Span(Span::new(file, range)),
            (Some(file), None) => Location::File(file),
            (None, _) => Location::Path(diagnostic.file.clone()),
        };
        let display = |path: &Path| format!("`{}`", sources.display_path(path).display());
        let (code, message) = match &diagnostic.kind {
            DiagnosticKind::InvalidFileName { name, error } => (
                code::INVALID_FILE_NAME,
                format!(
                    "file name `{name}` cannot be a module name: {}",
                    name_error(error)
                ),
            ),
            DiagnosticKind::InvalidDirName { name, error } => (
                code::INVALID_DIR_NAME,
                format!(
                    "directory name `{name}` cannot be a module name: {}",
                    name_error(error)
                ),
            ),
            DiagnosticKind::ReservedModuleName { name } => (
                code::RESERVED_MODULE_NAME,
                format!("module name `{name}` is reserved"),
            ),
            // 文面のパスも，診断の位置と同じく起点からの相対パスにする．
            DiagnosticKind::DuplicateModule { name, first } => (
                code::DUPLICATE_MODULE,
                format!(
                    "duplicate module `{name}` (already defined by {})",
                    display(first)
                ),
            ),
            DiagnosticKind::UndefinedModule { name } => {
                (code::UNDEFINED_MODULE, format!("undefined module `{name}`"))
            }
            DiagnosticKind::ImportCycle { modules } => {
                let message = match modules.as_slice() {
                    [only] => format!("module `{only}` imports itself"),
                    _ => {
                        let mut cycle: Vec<String> =
                            modules.iter().map(ToString::to_string).collect();
                        cycle.push(modules[0].to_string());
                        format!("import cycle: {}", cycle.join(" → "))
                    }
                };
                (code::IMPORT_CYCLE, message)
            }
            DiagnosticKind::Io { message } => (
                code::CANNOT_READ,
                format!("cannot read directory: {message}"),
            ),
        };
        Diagnostic::error(message).with_code(code).at(location)
    }

    /// 型検査の診断．`TypeError` は位置を持たないので，呼び出し側が `span` を渡す．
    pub fn from_type_error(error: &TypeError, cons: &TyCons, span: Span) -> Self {
        let show = |ty: &Ty| format!("`{}`", ty.display(cons));
        let diagnostic = match &*error.kind {
            TypeErrorKind::Mismatch { expected, actual } => {
                let diagnostic = Diagnostic::error(format!(
                    "type mismatch: expected {}, found {}",
                    show(&error.expected),
                    show(&error.actual)
                ));
                if *expected == error.expected && *actual == error.actual {
                    diagnostic
                } else {
                    diagnostic.with_note(format!(
                        "the mismatch is between {} and {}",
                        show(expected),
                        show(actual)
                    ))
                }
            }
            TypeErrorKind::InfiniteType { var, ty } => Diagnostic::error(format!(
                "infinite type: {} occurs in {}",
                show(&Ty::Var(*var)),
                show(ty)
            ))
            .with_note(format!(
                "expected {}, found {}",
                show(&error.expected),
                show(&error.actual)
            )),
        };
        let code = match &*error.kind {
            TypeErrorKind::Mismatch { .. } => code::TYPE_MISMATCH,
            TypeErrorKind::InfiniteType { .. } => code::INFINITE_TYPE,
        };
        diagnostic.with_code(code).with_span(span)
    }
}

/// 名前の誤りの理由（emela-resolve の `NameError` の英語の文面）．
fn name_error(error: &NameError) -> String {
    match error {
        NameError::NotSnakeCase => "not lower snake_case".to_owned(),
        NameError::NotTypeName { converted } => {
            format!("the converted name `{converted}` is not a valid type name")
        }
    }
}

/// エラーの件数．
pub fn error_count(diagnostics: &[Diagnostic]) -> usize {
    diagnostics.iter().filter(|d| d.is_error()).count()
}
