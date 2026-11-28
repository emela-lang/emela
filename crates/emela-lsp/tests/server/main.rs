//! Runs the server over an in-memory connection and checks the messages it sends.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use emela_driver::{FileSystem, Frontend, MemoryFiles};
use emela_resolve::{DirEntry, SourceFs};
use lsp_server::{Connection, Message, Notification, Request, RequestId};
use lsp_types::notification::{
    DidChangeTextDocument, DidChangeWatchedFiles, DidCloseTextDocument, DidOpenTextDocument,
    DidSaveTextDocument, Initialized, Notification as _, PublishDiagnostics,
};
use lsp_types::request::{
    CodeActionRequest, Completion, DocumentSymbolRequest, Formatting, GotoDefinition, HoverRequest,
    Initialize, InlayHintRequest, PrepareRenameRequest, References, Rename, Request as _,
    SemanticTokensFullRequest, SemanticTokensRangeRequest, Shutdown, SignatureHelpRequest,
};
use lsp_types::{self as lsp, Uri};
use serde_json::json;

mod actions;
mod completion;
mod docs;
mod documents;
mod hints;
mod hover;
mod navigation;

/// A semantic token as absolute (line, column, length, type, modifiers).
type Token = (u32, u32, u32, u32, u32);

/// A code action's title and edits (range, inserted text).
type Action = (String, Vec<(lsp::Range, String)>);

const TIMEOUT: Duration = Duration::from_secs(10);

fn uri(path: &str) -> Uri {
    Uri::from_str(&format!("file://{path}")).unwrap()
}

/// The editor side. The server runs on another thread.
struct Client {
    connection: Connection,
    server: Option<JoinHandle<()>>,
    next_id: i32,
}

impl Client {
    /// Starting is split out into [`Client::start`] so messages can be queued before the server
    /// starts.
    fn new() -> (Client, Connection) {
        let (server, client) = Connection::memory();
        let client = Client {
            connection: client,
            server: None,
            next_id: 1,
        };
        (client, server)
    }

    fn start<F: Frontend + 'static>(
        &mut self,
        connection: Connection,
        files: &[(&str, &str)],
        make_frontend: fn() -> F,
    ) {
        let fs = MemoryFiles::new(files.iter().copied());
        self.start_on(connection, fs, make_frontend);
    }

    fn start_on<Fs: FileSystem + Send + 'static, F: Frontend + 'static>(
        &mut self,
        connection: Connection,
        fs: Fs,
        make_frontend: fn() -> F,
    ) {
        self.server = Some(std::thread::spawn(move || {
            emela_lsp::serve(&connection, fs, make_frontend).unwrap();
        }));
    }

    /// Does not read the response yet.
    fn send_initialize(&mut self, root: Option<&str>) {
        self.send_initialize_with(root, json!({}));
    }

    fn send_initialize_with(&mut self, root: Option<&str>, capabilities: serde_json::Value) {
        let params = json!({
            "capabilities": capabilities,
            "rootUri": root.map(|r| format!("file://{r}")),
        });
        self.request(Initialize::METHOD, params);
        self.notify(Initialized::METHOD, json!({}));
    }

    fn initialize<F: Frontend + 'static>(
        files: &[(&str, &str)],
        root: Option<&str>,
        make_frontend: fn() -> F,
    ) -> Client {
        let fs = MemoryFiles::new(files.iter().copied());
        Client::initialize_on(fs, root, json!({}), make_frontend)
    }

    /// Initializes with the given client capabilities, on a file system the test can change.
    fn initialize_on<Fs: FileSystem + Send + 'static, F: Frontend + 'static>(
        fs: Fs,
        root: Option<&str>,
        capabilities: serde_json::Value,
        make_frontend: fn() -> F,
    ) -> Client {
        let (mut client, server) = Client::new();
        client.send_initialize_with(root, capabilities);
        client.start_on(server, fs, make_frontend);
        let response = client.recv();
        let Message::Response(response) = response else {
            panic!("no initialize response: {response:?}");
        };
        let result: lsp::InitializeResult =
            serde_json::from_value(response.response_result.unwrap()).expect("InitializeResult");
        assert_eq!(
            result.capabilities.text_document_sync,
            emela_lsp::capabilities().text_document_sync
        );
        client
    }

    fn request(&mut self, method: &str, params: serde_json::Value) -> RequestId {
        let id = RequestId::from(self.next_id);
        self.next_id += 1;
        self.send(Request::new(id.clone(), method.to_owned(), params).into());
        id
    }

    fn notify(&self, method: &str, params: serde_json::Value) {
        self.send(Notification::new(method.to_owned(), params).into());
    }

    fn send(&self, message: Message) {
        self.connection.sender.send(message).unwrap();
    }

    fn recv(&self) -> Message {
        self.connection
            .receiver
            .recv_timeout(TIMEOUT)
            .expect("no response from the server")
    }

    fn open(&self, path: &str, version: i32, text: &str) {
        self.notify(
            DidOpenTextDocument::METHOD,
            json!({ "textDocument": {
                "uri": uri(path), "languageId": "emela", "version": version, "text": text,
            }}),
        );
    }

    fn change(&self, path: &str, version: i32, text: &str) {
        self.notify(
            DidChangeTextDocument::METHOD,
            json!({
                "textDocument": { "uri": uri(path), "version": version },
                "contentChanges": [{ "text": text }],
            }),
        );
    }

    fn save(&self, path: &str) {
        self.notify(
            DidSaveTextDocument::METHOD,
            json!({ "textDocument": { "uri": uri(path) } }),
        );
    }

    /// `workspace/didChangeWatchedFiles` with (path, change type) pairs.
    fn watched(&self, changes: &[(&str, lsp::FileChangeType)]) {
        let changes: Vec<_> = changes
            .iter()
            .map(|(path, kind)| json!({ "uri": uri(path), "type": kind }))
            .collect();
        self.notify(DidChangeWatchedFiles::METHOD, json!({ "changes": changes }));
    }

    fn close(&self, path: &str) {
        self.notify(
            DidCloseTextDocument::METHOD,
            json!({ "textDocument": { "uri": uri(path) } }),
        );
    }

    /// Skips publishDiagnostics that arrive first; analysis and requests are not ordered.
    fn response(&self, id: &RequestId) -> lsp_server::Response {
        loop {
            match self.recv() {
                Message::Response(response) if response.id == *id => return response,
                Message::Notification(n) if n.method == PublishDiagnostics::METHOD => {}
                other => panic!("arrived before the response: {other:?}"),
            }
        }
    }

    /// `None` for `null`.
    fn format(&mut self, path: &str) -> Option<Vec<lsp::TextEdit>> {
        let id = self.request(
            Formatting::METHOD,
            json!({
                "textDocument": { "uri": uri(path) },
                "options": { "tabSize": 4, "insertSpaces": true },
            }),
        );
        serde_json::from_value(self.response(&id).response_result.unwrap()).unwrap()
    }

    /// `None` for `null`.
    fn hover(&mut self, path: &str, line: u32, character: u32) -> Option<lsp::Hover> {
        let id = self.request(
            HoverRequest::METHOD,
            json!({
                "textDocument": { "uri": uri(path) },
                "position": { "line": line, "character": character },
            }),
        );
        serde_json::from_value(self.response(&id).response_result.unwrap()).unwrap()
    }

    /// `None` for `null`.
    fn definition(&mut self, path: &str, line: u32, character: u32) -> Option<lsp::Location> {
        let id = self.request(
            GotoDefinition::METHOD,
            json!({
                "textDocument": { "uri": uri(path) },
                "position": { "line": line, "character": character },
            }),
        );
        let result: Option<lsp::GotoDefinitionResponse> =
            serde_json::from_value(self.response(&id).response_result.unwrap()).unwrap();
        match result? {
            lsp::GotoDefinitionResponse::Scalar(location) => Some(location),
            other => panic!("not exactly one location: {other:?}"),
        }
    }

    /// Returns (path, range) sorted by path and position; `None` for `null`.
    fn references(
        &mut self,
        path: &str,
        line: u32,
        character: u32,
        declaration: bool,
    ) -> Option<Vec<(String, lsp::Range)>> {
        let id = self.request(
            References::METHOD,
            json!({
                "textDocument": { "uri": uri(path) },
                "position": { "line": line, "character": character },
                "context": { "includeDeclaration": declaration },
            }),
        );
        let result: Option<Vec<lsp::Location>> =
            serde_json::from_value(self.response(&id).response_result.unwrap()).unwrap();
        let mut found: Vec<(String, lsp::Range)> = result?
            .into_iter()
            .map(|l| {
                let path = l.uri.as_str().strip_prefix("file://").unwrap().to_owned();
                (path, l.range)
            })
            .collect();
        found.sort_by_key(|(path, r)| (path.clone(), r.start.line, r.start.character));
        Some(found)
    }

    /// `None` for `null`.
    fn document_symbols(&mut self, path: &str) -> Option<Vec<lsp::DocumentSymbol>> {
        let id = self.request(
            DocumentSymbolRequest::METHOD,
            json!({ "textDocument": { "uri": uri(path) } }),
        );
        let result: Option<lsp::DocumentSymbolResponse> =
            serde_json::from_value(self.response(&id).response_result.unwrap()).unwrap();
        match result? {
            lsp::DocumentSymbolResponse::Nested(symbols) => Some(symbols),
            other => panic!("not nested: {other:?}"),
        }
    }

    /// Returns (name, kind, detail); `None` for `null`.
    fn complete(
        &mut self,
        path: &str,
        line: u32,
        character: u32,
    ) -> Option<Vec<(String, lsp::CompletionItemKind, Option<String>)>> {
        let id = self.request(
            Completion::METHOD,
            json!({
                "textDocument": { "uri": uri(path) },
                "position": { "line": line, "character": character },
            }),
        );
        let result: Option<lsp::CompletionResponse> =
            serde_json::from_value(self.response(&id).response_result.unwrap()).unwrap();
        let lsp::CompletionResponse::Array(mut items) = result? else {
            panic!("not an array");
        };
        items.sort_by(|a, b| a.sort_text.cmp(&b.sort_text));
        Some(
            items
                .into_iter()
                .map(|i| (i.label, i.kind.unwrap(), i.detail))
                .collect(),
        )
    }

    fn position(path: &str, line: u32, character: u32) -> serde_json::Value {
        json!({
            "textDocument": { "uri": uri(path) },
            "position": { "line": line, "character": character },
        })
    }

    /// Returns (label, parameter label strings, active parameter).
    fn signature(
        &mut self,
        path: &str,
        line: u32,
        character: u32,
    ) -> Option<(String, Vec<String>, Option<u32>)> {
        let id = self.request(
            SignatureHelpRequest::METHOD,
            Client::position(path, line, character),
        );
        let result: Option<lsp::SignatureHelp> =
            serde_json::from_value(self.response(&id).response_result.unwrap()).unwrap();
        let help = result?;
        let sig = &help.signatures[0];
        let utf16: Vec<u16> = sig.label.encode_utf16().collect();
        let params = sig
            .parameters
            .iter()
            .flatten()
            .map(|p| match p.label {
                lsp::ParameterLabel::LabelOffsets([a, b]) => {
                    String::from_utf16(&utf16[a as usize..b as usize]).unwrap()
                }
                _ => panic!("not an offset pair"),
            })
            .collect();
        Some((sig.label.clone(), params, help.active_parameter))
    }

    fn prepare_rename(&mut self, path: &str, line: u32, character: u32) -> Option<lsp::Range> {
        let id = self.request(
            PrepareRenameRequest::METHOD,
            Client::position(path, line, character),
        );
        let result: Option<lsp::PrepareRenameResponse> =
            serde_json::from_value(self.response(&id).response_result.unwrap()).unwrap();
        match result? {
            lsp::PrepareRenameResponse::Range(range) => Some(range),
            other => panic!("not a range: {other:?}"),
        }
    }

    /// Returns (path, range, new text) sorted by path and position, or the message of an error
    /// response.
    fn rename(
        &mut self,
        path: &str,
        line: u32,
        character: u32,
        new_name: &str,
    ) -> Result<Vec<(String, lsp::Range, String)>, String> {
        let mut params = Client::position(path, line, character);
        params["newName"] = json!(new_name);
        let id = self.request(Rename::METHOD, params);
        let edit: lsp::WorkspaceEdit = match self.response(&id).response_result {
            Ok(value) => serde_json::from_value(value).unwrap(),
            Err(err) => return Err(err.message),
        };
        let mut found: Vec<(String, lsp::Range, String)> = edit
            .changes
            .unwrap()
            .into_iter()
            .flat_map(|(uri, edits)| {
                let path = uri.as_str().strip_prefix("file://").unwrap().to_owned();
                edits
                    .into_iter()
                    .map(move |e| (path.clone(), e.range, e.new_text))
            })
            .collect();
        found.sort_by_key(|(path, r, _)| (path.clone(), r.start.line, r.start.character));
        Ok(found)
    }

    /// Returns (title, edits) where each edit is (range, inserted text).
    fn code_actions(
        &mut self,
        path: &str,
        start: (u32, u32),
        end: (u32, u32),
    ) -> Option<Vec<Action>> {
        let id = self.request(
            CodeActionRequest::METHOD,
            json!({
                "textDocument": { "uri": uri(path) },
                "range": range(start, end),
                "context": { "diagnostics": [] },
            }),
        );
        let result: Option<Vec<lsp::CodeActionOrCommand>> =
            serde_json::from_value(self.response(&id).response_result.unwrap()).unwrap();
        Some(
            result?
                .into_iter()
                .map(|a| match a {
                    lsp::CodeActionOrCommand::CodeAction(a) => {
                        assert_eq!(a.kind, Some(lsp::CodeActionKind::REFACTOR_REWRITE));
                        assert!(a.diagnostics.is_none());
                        let edits = a
                            .edit
                            .unwrap()
                            .changes
                            .unwrap()
                            .into_iter()
                            .flat_map(|(u, edits)| {
                                assert_eq!(u, uri(path));
                                edits
                            })
                            .map(|e| (e.range, e.new_text))
                            .collect();
                        (a.title, edits)
                    }
                    other => panic!("a command, not an action: {other:?}"),
                })
                .collect(),
        )
    }

    fn quickfixes(
        &mut self,
        path: &str,
        start: (u32, u32),
        end: (u32, u32),
    ) -> Vec<lsp::CodeAction> {
        let id = self.request(
            CodeActionRequest::METHOD,
            json!({
                "textDocument": { "uri": uri(path) },
                "range": range(start, end),
                "context": { "diagnostics": [], "only": ["quickfix"] },
            }),
        );
        let actions: Vec<lsp::CodeAction> =
            serde_json::from_value(self.response(&id).response_result.unwrap()).unwrap();
        for action in &actions {
            assert_eq!(action.kind, Some(lsp::CodeActionKind::QUICKFIX));
            assert_eq!(action.diagnostics.as_ref().unwrap().len(), 1);
            assert!(action.diagnostics.as_ref().unwrap()[0].code.is_some());
            assert_eq!(
                action
                    .edit
                    .as_ref()
                    .unwrap()
                    .changes
                    .as_ref()
                    .unwrap()
                    .len(),
                1
            );
            assert!(
                action
                    .edit
                    .as_ref()
                    .unwrap()
                    .changes
                    .as_ref()
                    .unwrap()
                    .contains_key(&uri(path))
            );
        }
        actions
    }

    /// Returns semantic tokens in absolute positions. `range` asks for `semanticTokens/range`
    /// instead of `full`.
    fn semantic_tokens(&mut self, path: &str, range: Option<lsp::Range>) -> Option<Vec<Token>> {
        let document = json!({ "uri": uri(path) });
        let id = match range {
            None => self.request(
                SemanticTokensFullRequest::METHOD,
                json!({ "textDocument": document }),
            ),
            Some(range) => self.request(
                SemanticTokensRangeRequest::METHOD,
                json!({ "textDocument": document, "range": range }),
            ),
        };
        let result: Option<lsp::SemanticTokens> =
            serde_json::from_value(self.response(&id).response_result.unwrap()).unwrap();
        let (mut line, mut col) = (0, 0);
        Some(
            result?
                .data
                .into_iter()
                .map(|t| {
                    if t.delta_line > 0 {
                        col = 0;
                    }
                    line += t.delta_line;
                    col += t.delta_start;
                    (line, col, t.length, t.token_type, t.token_modifiers_bitset)
                })
                .collect(),
        )
    }

    /// Returns hints for the whole file as (line, column, label, left padding).
    fn inlay_hints(&mut self, path: &str) -> Option<Vec<(u32, u32, String, bool)>> {
        let id = self.request(
            InlayHintRequest::METHOD,
            json!({
                "textDocument": { "uri": uri(path) },
                "range": range((0, 0), (10_000, 0)),
            }),
        );
        let result: Option<Vec<lsp::InlayHint>> =
            serde_json::from_value(self.response(&id).response_result.unwrap()).unwrap();
        Some(
            result?
                .into_iter()
                .map(|h| {
                    let lsp::InlayHintLabel::String(label) = h.label else {
                        panic!("not a string");
                    };
                    (
                        h.position.line,
                        h.position.character,
                        label,
                        h.padding_left == Some(true),
                    )
                })
                .collect(),
        )
    }

    fn diagnostics(&self) -> lsp::PublishDiagnosticsParams {
        match self.recv() {
            Message::Notification(n) if n.method == PublishDiagnostics::METHOD => {
                serde_json::from_value(n.params).unwrap()
            }
            other => panic!("not publishDiagnostics: {other:?}"),
        }
    }

    /// The next `n` publishDiagnostics. One analysis publishes in URI order.
    fn diagnostics_n(&self, n: usize) -> Vec<lsp::PublishDiagnosticsParams> {
        (0..n).map(|_| self.diagnostics()).collect()
    }

    /// Skips publishDiagnostics that arrive before the shutdown response. The server answers
    /// requests between analyses, so diagnostics of the last edit come before or after the last
    /// response depending on how long the analysis takes. Checks that nothing else arrived.
    fn shutdown(self) {
        self.shutdown_with(|_| {});
    }

    /// Also checks that no publishDiagnostics arrived before shutting down.
    fn shutdown_with_nothing_published(self) {
        self.shutdown_with(|n| panic!("arrived before the shutdown response: {n:?}"));
    }

    fn shutdown_with(mut self, mut on_diagnostics: impl FnMut(Notification)) {
        let id = self.request(Shutdown::METHOD, serde_json::Value::Null);
        loop {
            match self.recv() {
                Message::Response(response) if response.id == id => break,
                Message::Notification(n) if n.method == PublishDiagnostics::METHOD => {
                    on_diagnostics(n);
                }
                other => panic!("arrived before the shutdown response: {other:?}"),
            }
        }
        self.notify("exit", serde_json::Value::Null);
        self.server.take().unwrap().join().unwrap();
    }
}

/// Files on disk that a test changes while the server runs, without the editor's buffers.
#[derive(Clone, Default)]
struct Disk(Arc<Mutex<BTreeMap<PathBuf, String>>>);

impl Disk {
    fn new(files: &[(&str, &str)]) -> Disk {
        let disk = Disk::default();
        for (path, text) in files {
            disk.write(path, text);
        }
        disk
    }

    fn write(&self, path: &str, text: &str) {
        self.0.lock().unwrap().insert(path.into(), text.to_owned());
    }

    fn delete(&self, path: &str) {
        self.0.lock().unwrap().remove(Path::new(path));
    }

    fn now(&self) -> MemoryFiles {
        MemoryFiles::new(self.0.lock().unwrap().clone())
    }
}

impl SourceFs for Disk {
    fn read_dir(&self, dir: &Path) -> std::io::Result<Vec<DirEntry>> {
        self.now().read_dir(dir)
    }
}

impl FileSystem for Disk {
    fn absolute(&self, path: &Path) -> std::io::Result<PathBuf> {
        self.now().absolute(path)
    }

    fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
        self.now().read_to_string(path)
    }

    fn is_file(&self, path: &Path) -> bool {
        self.now().is_file(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.now().is_dir(path)
    }
}

fn codes(params: &lsp::PublishDiagnosticsParams) -> Vec<String> {
    params
        .diagnostics
        .iter()
        .map(|d| match &d.code {
            Some(lsp::NumberOrString::String(code)) => code.clone(),
            other => panic!("no code: {other:?}"),
        })
        .collect()
}

fn range(start: (u32, u32), end: (u32, u32)) -> lsp::Range {
    lsp::Range {
        start: lsp::Position::new(start.0, start.1),
        end: lsp::Position::new(end.0, end.1),
    }
}

/// The project for go-to-definition, find-references and document symbols.
const NAV_MAIN: &str = "import Util.Log
import Shapes.{Shape, area}

effect Logger {
  fn info(msg: String) -> ()
}

handler Quiet: Logger {
  fn info(msg) { () }
}

fn greet(name: String) {
  log = use Logger
  log.info(Log.prefix(name))
}

fn main() {
  s = Shape(w: 1, h: 2)
  with Quiet { greet(\"a\") }
  area(s)
}
";

const NAV_SHAPES: &str =
    "pub type Shape(w: Int, h: Int)\n\npub fn area(s: Shape) -> Int {\n  s.w * s.h\n}\n";

const NAV_LOG: &str = "pub fn prefix(s: String) -> String {\n  s\n}\n";

fn nav_files() -> [(&'static str, &'static str); 4] {
    [
        ("/app/Pome.toml", ""),
        ("/app/src/main.emel", NAV_MAIN),
        ("/app/src/shapes.emel", NAV_SHAPES),
        ("/app/src/util/log.emel", NAV_LOG),
    ]
}

fn location(path: &str, start: (u32, u32), end: (u32, u32)) -> Option<lsp::Location> {
    Some(lsp::Location {
        uri: uri(path),
        range: range(start, end),
    })
}

const MAIN: &str = "/app/src/main.emel";

fn labels(items: &[(String, lsp::CompletionItemKind, Option<String>)]) -> Vec<&str> {
    items.iter().map(|(l, _, _)| l.as_str()).collect()
}
