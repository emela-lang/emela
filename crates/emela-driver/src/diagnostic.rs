//! Diagnostics shared by all stages. Lexer, resolver and type checker diagnostics are converted
//! into these before rendering.
//!
//! The original diagnostic types stay in their crates; only the conversions live here.

use std::path::PathBuf;

use line_index::TextRange;

use crate::source::FileId;

mod resolve;
mod types;

/// The prefix of notes that are help.
pub const HELP_PREFIX: &str = "help: ";

/// The part of the helps of W4401 and E4404 that suggests a bound item. The help is left out
/// where a bound item would change meaning ([`emela_types::ProgramTypes::bound_item_redirects`]).
pub(crate) const NAMED_ITEM_SUGGESTION: &str = "a named item `with x = H`";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Error,
    Warning,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    Span(Span),
    /// The whole file (a bad file name, ...).
    File(FileId),
    /// A path not in the source table (a directory, a file that could not be read).
    Path(PathBuf),
}

/// An explanation attached somewhere other than the primary location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub span: Span,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    /// The error code ([`crate::code`]); `None` for stages that do not assign one yet (lexing).
    pub code: Option<&'static str>,
    pub message: String,
    /// `None` for diagnostics without a location (node not found, ...).
    pub location: Option<Location>,
    pub labels: Vec<Label>,
    /// Notes starting with [`HELP_PREFIX`] are printed as help (`= help: …`).
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

    /// Marks a diagnostic produced by the experimental feature `name` with the note
    /// ``experimental feature `name` `` (spec `[intro.experimental]`).
    pub fn from_experimental_feature(self, name: &str) -> Self {
        self.with_note(format!("experimental feature `{name}`"))
    }

    /// Printed as `= help: …`. Stored as a note rather than a new field, to keep the error side
    /// of `Result` small.
    pub fn with_help(self, help: impl AsRef<str>) -> Self {
        self.with_note(format!("{HELP_PREFIX}{}", help.as_ref()))
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// The code (E1xxx) and English message come from emela-syntax.
    pub fn from_syntax(file: FileId, diagnostic: &emela_syntax::Diagnostic) -> Self {
        let out = Diagnostic::error(&diagnostic.message)
            .with_code(diagnostic.code.as_str())
            .with_span(Span::new(file, diagnostic.range));
        match &diagnostic.help {
            Some(help) => out.with_help(help),
            None => out,
        }
    }
}

pub fn error_count(diagnostics: &[Diagnostic]) -> usize {
    diagnostics.iter().filter(|d| d.is_error()).count()
}
