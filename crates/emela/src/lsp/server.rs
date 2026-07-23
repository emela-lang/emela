//! The server loop (spec 0033): reads JSON-RPC messages from stdin
//! sequentially, dispatches them, and writes responses and diagnostics to
//! stdout. Checks are fast enough (whole-pipeline, small files) that a single
//! thread keeps the protocol simple.

use std::collections::{HashMap, HashSet};
use std::io::{self, BufReader, Write};
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::lsp::analysis::{self, Snapshot};
use crate::lsp::code_action;
use crate::lsp::completion;
use crate::lsp::documents::DocumentStore;
use crate::lsp::hover;
use crate::lsp::protocol::{
    CodeActionParams, CompletionParams, DidChangeParams, DidCloseParams, DidOpenParams,
    HoverParams, PublishDiagnosticsParams,
};
use crate::lsp::rpc::{self, Message};

const INVALID_PARAMS: i64 = -32602;

struct Server {
    package_paths: Vec<PathBuf>,
    platform_registry: Vec<emela_codegen::PlatformFn>,
    documents: DocumentStore,
    /// The last usable completion scope per document (spec 0033).
    snapshots: HashMap<String, Snapshot>,
    /// The URIs each entry document published diagnostics to last round, so
    /// the ones that went clean get an explicit empty publish.
    published: HashMap<String, HashSet<String>>,
    initialized: bool,
    shutdown_requested: bool,
}

/// Runs until the client sends `exit`; the return value is the process exit
/// code (0 after an orderly `shutdown`, 1 otherwise — also when the client
/// hangs up without one).
pub(crate) fn run(
    package_paths: Vec<PathBuf>,
    platform_registry: Vec<emela_codegen::PlatformFn>,
) -> Result<i32> {
    let mut server = Server {
        package_paths,
        platform_registry,
        documents: DocumentStore::default(),
        snapshots: HashMap::new(),
        published: HashMap::new(),
        initialized: false,
        shutdown_requested: false,
    };
    let stdin = io::stdin();
    let mut input = BufReader::new(stdin.lock());
    let stdout = io::stdout();
    let mut output = stdout.lock();
    loop {
        let message = rpc::read_message(&mut input)
            .map_err(|err| Error::new(format!("lsp: failed to read message: {err}")))?;
        let Some(message) = message else {
            return Ok(1);
        };
        let outcome = server
            .handle(&mut output, message)
            .map_err(|err| Error::new(format!("lsp: failed to write message: {err}")))?;
        if let Some(code) = outcome {
            return Ok(code);
        }
    }
}

impl Server {
    /// Dispatches one message; `Some(code)` means `exit` was received.
    fn handle(&mut self, out: &mut impl Write, message: Message) -> io::Result<Option<i32>> {
        let Some(method) = message.method else {
            // No method and no id: not a JSON-RPC message at all. (With an id
            // it would be a response, but this server sends no requests.)
            if message.id.is_none() {
                rpc::write_error(out, &Value::Null, rpc::PARSE_ERROR, "malformed message")?;
            }
            return Ok(None);
        };
        match (method.as_str(), message.id) {
            ("initialize", Some(id)) => {
                self.initialized = true;
                rpc::write_response(out, &id, capabilities())?;
            }
            ("shutdown", Some(id)) => {
                self.shutdown_requested = true;
                rpc::write_response(out, &id, Value::Null)?;
            }
            ("exit", _) => {
                return Ok(Some(if self.shutdown_requested { 0 } else { 1 }));
            }
            (_, Some(id)) if !self.initialized => {
                rpc::write_error(
                    out,
                    &id,
                    rpc::SERVER_NOT_INITIALIZED,
                    "server not initialized",
                )?;
            }
            (_, None) if !self.initialized => {}
            ("initialized", _) | ("$/cancelRequest", _) => {}
            ("textDocument/didOpen", _) => {
                if let Ok(params) = serde_json::from_value::<DidOpenParams>(message.params) {
                    self.documents.open(
                        params.text_document.uri,
                        params.text_document.version,
                        params.text_document.text,
                    );
                    self.check_open_documents(out)?;
                }
            }
            ("textDocument/didChange", _) => {
                if let Ok(params) = serde_json::from_value::<DidChangeParams>(message.params)
                    && let Some(change) = params.content_changes.into_iter().last()
                {
                    self.documents.change(
                        &params.text_document.uri,
                        params.text_document.version,
                        change.text,
                    );
                    self.check_open_documents(out)?;
                }
            }
            ("textDocument/didSave", _) => {
                self.check_open_documents(out)?;
            }
            ("textDocument/didClose", _) => {
                if let Ok(params) = serde_json::from_value::<DidCloseParams>(message.params) {
                    let uri = params.text_document.uri;
                    self.documents.close(&uri);
                    self.snapshots.remove(&uri);
                    // Clear this document's diagnostics and anything it
                    // published for imported files.
                    let mut stale = self.published.remove(&uri).unwrap_or_default();
                    stale.insert(uri);
                    for target in stale {
                        publish(out, target, Vec::new())?;
                    }
                }
            }
            ("textDocument/completion", Some(id)) => {
                let Ok(params) = serde_json::from_value::<CompletionParams>(message.params) else {
                    return rpc::write_error(out, &id, INVALID_PARAMS, "invalid params")
                        .map(|()| None);
                };
                let items = match self.documents.get(&params.text_document.uri) {
                    Some(doc) => {
                        let empty = Snapshot::default();
                        let snapshot = self
                            .snapshots
                            .get(&params.text_document.uri)
                            .unwrap_or(&empty);
                        completion::complete(doc, &params.position, snapshot, &self.package_paths)
                    }
                    None => Vec::new(),
                };
                rpc::write_response(out, &id, serde_json::to_value(items)?)?;
            }
            ("textDocument/hover", Some(id)) => {
                let Ok(params) = serde_json::from_value::<HoverParams>(message.params) else {
                    return rpc::write_error(out, &id, INVALID_PARAMS, "invalid params")
                        .map(|()| None);
                };
                let result = match (
                    self.documents.get(&params.text_document.uri),
                    self.snapshots.get(&params.text_document.uri),
                ) {
                    (Some(doc), Some(snapshot)) => hover::hover(doc, &params.position, snapshot),
                    _ => None,
                };
                // "Nothing to show" is a `null` result, not an error.
                let value = match result {
                    Some(hover) => serde_json::to_value(hover)?,
                    None => Value::Null,
                };
                rpc::write_response(out, &id, value)?;
            }
            ("textDocument/codeAction", Some(id)) => {
                let Ok(params) = serde_json::from_value::<CodeActionParams>(message.params) else {
                    return rpc::write_error(out, &id, INVALID_PARAMS, "invalid params")
                        .map(|()| None);
                };
                let actions = match (
                    self.documents.get(&params.text_document.uri),
                    self.snapshots.get(&params.text_document.uri),
                ) {
                    (Some(doc), Some(snapshot)) => {
                        code_action::actions(doc, &params.range, snapshot)
                    }
                    _ => Vec::new(),
                };
                rpc::write_response(out, &id, serde_json::to_value(actions)?)?;
            }
            (_, Some(id)) => {
                rpc::write_error(out, &id, rpc::METHOD_NOT_FOUND, "method not found")?;
            }
            (_, None) => {}
        }
        Ok(None)
    }

    /// Re-checks every open document (spec 0033) — an edit in one buffer can
    /// change the diagnostics of any importer — and publishes the results,
    /// clearing URIs that went clean since the previous round.
    fn check_open_documents(&mut self, out: &mut impl Write) -> io::Result<()> {
        for uri in self.documents.uris() {
            let Some(doc) = self.documents.get(&uri) else {
                continue;
            };
            let outcome = analysis::check_document(
                doc,
                &self.documents,
                &self.package_paths,
                &self.platform_registry,
            );
            if !outcome.snapshot.is_empty() || !self.snapshots.contains_key(&uri) {
                self.snapshots.insert(uri.clone(), outcome.snapshot);
            }
            let mut current = HashSet::new();
            for (target, diagnostics) in outcome.diagnostics {
                current.insert(target.clone());
                publish(out, target, diagnostics)?;
            }
            let previous = self
                .published
                .insert(uri.clone(), current.clone())
                .unwrap_or_default();
            for stale in previous.difference(&current) {
                publish(out, stale.clone(), Vec::new())?;
            }
        }
        Ok(())
    }
}

fn publish(
    out: &mut impl Write,
    uri: String,
    diagnostics: Vec<crate::lsp::protocol::Diagnostic>,
) -> io::Result<()> {
    let params = PublishDiagnosticsParams { uri, diagnostics };
    rpc::write_notification(
        out,
        "textDocument/publishDiagnostics",
        serde_json::to_value(params)?,
    )
}

fn capabilities() -> Value {
    json!({
        "capabilities": {
            "positionEncoding": "utf-16",
            "textDocumentSync": {
                "openClose": true,
                // TextDocumentSyncKind.Full (spec 0033).
                "change": 1,
                "save": true,
            },
            "completionProvider": {
                "triggerCharacters": [".", ":", "{"],
            },
            "hoverProvider": true,
            "codeActionProvider": {
                "codeActionKinds": ["quickfix"],
            },
        },
        "serverInfo": {
            "name": "emela-lsp",
            "version": option_env!("EMELA_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
        },
    })
}
