//! The subset of LSP 3.17 types this server speaks (spec 0033), hand-defined
//! so the protocol layer stays serde-only. Field names follow the wire format
//! (camelCase) via serde renames; unknown incoming fields are ignored.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Position {
    /// Zero-based line.
    pub(crate) line: u32,
    /// Zero-based UTF-16 code-unit column.
    pub(crate) character: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Range {
    pub(crate) start: Position,
    pub(crate) end: Position,
}

/// DiagnosticSeverity.Error — the compiler has no warnings (spec 0033).
pub(crate) const SEVERITY_ERROR: u8 = 1;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Diagnostic {
    pub(crate) range: Range,
    pub(crate) severity: u8,
    pub(crate) source: &'static str,
    pub(crate) message: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct PublishDiagnosticsParams {
    pub(crate) uri: String,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TextDocumentItem {
    pub(crate) uri: String,
    pub(crate) version: i64,
    pub(crate) text: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TextDocumentIdentifier {
    pub(crate) uri: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct VersionedTextDocumentIdentifier {
    pub(crate) uri: String,
    pub(crate) version: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DidOpenParams {
    pub(crate) text_document: TextDocumentItem,
}

/// Full-sync content change (spec 0033): only `text` matters; a client honoring
/// the advertised `TextDocumentSyncKind.Full` never sends `range`.
#[derive(Debug, Deserialize)]
pub(crate) struct ContentChange {
    pub(crate) text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DidChangeParams {
    pub(crate) text_document: VersionedTextDocumentIdentifier,
    pub(crate) content_changes: Vec<ContentChange>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DidCloseParams {
    pub(crate) text_document: TextDocumentIdentifier,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompletionParams {
    pub(crate) text_document: TextDocumentIdentifier,
    pub(crate) position: Position,
}

/// CompletionItemKind values used by the server.
pub(crate) mod completion_kind {
    pub(crate) const METHOD: u8 = 2;
    pub(crate) const FUNCTION: u8 = 3;
    pub(crate) const VARIABLE: u8 = 6;
    pub(crate) const CLASS: u8 = 7;
    pub(crate) const MODULE: u8 = 9;
    pub(crate) const ENUM: u8 = 13;
    pub(crate) const KEYWORD: u8 = 14;
    pub(crate) const ENUM_MEMBER: u8 = 20;
    /// Used for effect names (spec 0009) — visually distinct from values.
    pub(crate) const EVENT: u8 = 23;
}

/// InsertTextFormat.Snippet — `${1:placeholder}` tab stops.
pub(crate) const INSERT_TEXT_FORMAT_SNIPPET: u8 = 2;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompletionItem {
    pub(crate) label: String,
    pub(crate) kind: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) insert_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) insert_text_format: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) sort_text: Option<String>,
}

impl CompletionItem {
    pub(crate) fn new(label: impl Into<String>, kind: u8) -> Self {
        CompletionItem {
            label: label.into(),
            kind,
            detail: None,
            insert_text: None,
            insert_text_format: None,
            sort_text: None,
        }
    }

    pub(crate) fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub(crate) fn snippet(mut self, text: impl Into<String>) -> Self {
        self.insert_text = Some(text.into());
        self.insert_text_format = Some(INSERT_TEXT_FORMAT_SNIPPET);
        self
    }

    pub(crate) fn sort_group(mut self, group: char) -> Self {
        self.sort_text = Some(format!("{group}{}", self.label));
        self
    }
}
