//! 各段で共通の診断．字句解析，名前解決，型検査の診断をここへ写してから表示する．
//!
//! 元の診断の型はそれぞれのクレートに残し，ここでは変換だけを持つ．

use std::path::PathBuf;

use emela_types::{Ty, TyCons, TypeError, TypeErrorKind};
use line_index::TextRange;

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
            message: message.into(),
            location: None,
            labels: Vec::new(),
            notes: Vec::new(),
        }
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

    /// 字句解析の診断．
    pub fn from_syntax(file: FileId, diagnostic: &emela_syntax::Diagnostic) -> Self {
        Diagnostic::error(&diagnostic.message).with_span(Span::new(file, diagnostic.range))
    }

    /// 名前解決の診断．パスがソースの表にあれば ID で指し，なければパスのまま持つ．
    pub fn from_resolve(diagnostic: &emela_resolve::Diagnostic, sources: &SourceDb) -> Self {
        let location = match (sources.file_id(&diagnostic.file), diagnostic.range) {
            (Some(file), Some(range)) => Location::Span(Span::new(file, range)),
            (Some(file), None) => Location::File(file),
            (None, _) => Location::Path(diagnostic.file.clone()),
        };
        Diagnostic::error(diagnostic.to_string()).at(location)
    }

    /// 型検査の診断．`TypeError` は位置を持たないので，呼び出し側が `span` を渡す．
    pub fn from_type_error(error: &TypeError, cons: &TyCons, span: Span) -> Self {
        let show = |ty: &Ty| format!("`{}`", ty.display(cons));
        let diagnostic = match &*error.kind {
            TypeErrorKind::Mismatch { expected, actual } => {
                let diagnostic = Diagnostic::error(format!(
                    "型が合わない: {} を期待したが {} だった",
                    show(&error.expected),
                    show(&error.actual)
                ));
                if *expected == error.expected && *actual == error.actual {
                    diagnostic
                } else {
                    diagnostic.with_note(format!(
                        "食い違った部分: {} と {}",
                        show(expected),
                        show(actual)
                    ))
                }
            }
            TypeErrorKind::InfiniteType { var, ty } => Diagnostic::error(format!(
                "無限型になる: {} が {} の中に現れる",
                show(&Ty::Var(*var)),
                show(ty)
            ))
            .with_note(format!(
                "期待した型 {}，実際の型 {}",
                show(&error.expected),
                show(&error.actual)
            )),
        };
        diagnostic.with_span(span)
    }
}

/// エラーの件数．
pub fn error_count(diagnostics: &[Diagnostic]) -> usize {
    diagnostics.iter().filter(|d| d.is_error()).count()
}
