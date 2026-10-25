//! Runs the server over an in-memory connection and checks the messages it sends.

use std::str::FromStr;
use std::thread::JoinHandle;
use std::time::Duration;

use emela_driver::{Frontend, MemoryFiles, ParseOnly, Typecheck};
use lsp_server::{Connection, Message, Notification, Request, RequestId};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument,
    Initialized, Notification as _, PublishDiagnostics,
};
use lsp_types::request::{
    CodeActionRequest, Completion, DocumentSymbolRequest, Formatting, GotoDefinition, HoverRequest,
    Initialize, InlayHintRequest, PrepareRenameRequest, References, Rename, Request as _,
    SemanticTokensFullRequest, SemanticTokensRangeRequest, Shutdown, SignatureHelpRequest,
};
use lsp_types::{self as lsp, Uri};
use serde_json::json;

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
        self.server = Some(std::thread::spawn(move || {
            emela_lsp::serve(&connection, fs, make_frontend).unwrap();
        }));
    }

    /// Does not read the response yet.
    fn send_initialize(&mut self, root: Option<&str>) {
        let params = json!({
            "capabilities": {},
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
        let (mut client, server) = Client::new();
        client.send_initialize(root);
        client.start(server, files, make_frontend);
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

    /// Also checks that no other notification arrived before shutting down.
    fn shutdown(mut self) {
        let id = self.request(Shutdown::METHOD, serde_json::Value::Null);
        match self.recv() {
            Message::Response(response) if response.id == id => {}
            other => panic!("arrived before the shutdown response: {other:?}"),
        }
        self.notify("exit", serde_json::Value::Null);
        self.server.take().unwrap().join().unwrap();
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

#[test]
fn lex_error_appears_and_clears() {
    let files = [
        ("/app/Pome.toml", ""),
        ("/app/src/main.emel", "const X = 1\n"),
    ];
    let client = Client::initialize(&files, Some("/app"), ParseOnly::new);
    // The contents on disk have no errors, so nothing arrives at first.
    client.open("/app/src/main.emel", 1, "const X = 1\nconst Y = \"\\q\"\n");
    let published = client.diagnostics();
    assert_eq!(published.uri, uri("/app/src/main.emel"));
    assert_eq!(published.version, Some(1));
    assert_eq!(codes(&published), ["E0107"]);
    let diagnostic = &published.diagnostics[0];
    assert_eq!(diagnostic.range, range((1, 11), (1, 13)));
    assert_eq!(diagnostic.severity, Some(lsp::DiagnosticSeverity::ERROR));
    assert_eq!(diagnostic.source.as_deref(), Some("emela"));
    assert_eq!(diagnostic.message, "unknown escape sequence `\\q`");

    client.change("/app/src/main.emel", 2, "const X = 1\nconst Y = 2\n");
    let published = client.diagnostics();
    assert_eq!(published.uri, uri("/app/src/main.emel"));
    assert_eq!(published.version, Some(2));
    assert!(published.diagnostics.is_empty());
    client.shutdown();
}

#[test]
fn columns_are_utf16() {
    // No `Pome.toml`, so this is a standalone file.
    let files = [("/tmp/x/main.emel", "")];
    let client = Client::initialize(&files, None, ParseOnly::new);
    // `あ` and `日本` take one UTF-16 unit per character and `😀` two; in UTF-8, 3 and 4 bytes.
    client.open(
        "/tmp/x/main.emel",
        1,
        "# 日本\nconst S = \"あ😀\\q日本\\q\"\n",
    );
    let published = client.diagnostics();
    assert_eq!(codes(&published), ["E0107", "E0107"]);
    assert_eq!(published.diagnostics[0].range, range((1, 14), (1, 16)));
    assert_eq!(published.diagnostics[1].range, range((1, 18), (1, 20)));
    client.shutdown();
}

#[test]
fn import_errors_go_to_the_importing_file() {
    let files = [
        ("/app/Pome.toml", ""),
        (
            "/app/src/main.emel",
            "import Http.Client\nimport Http.Clinet\n",
        ),
        ("/app/src/http/client.emel", "const GET = 1\n"),
        ("/app/src/util.emel", "const U = 1\n"),
    ];
    // Workspace projects get diagnostics before any file is opened.
    let client = Client::initialize(&files, Some("/app"), ParseOnly::new);
    let published = client.diagnostics();
    assert_eq!(published.uri, uri("/app/src/main.emel"));
    assert_eq!(published.version, None);
    assert_eq!(codes(&published), ["E0205"]);
    assert_eq!(published.diagnostics[0].range, range((1, 7), (1, 18)));
    assert_eq!(
        published.diagnostics[0].message,
        "undefined module `Http.Clinet`"
    );

    // Break the import with unsaved contents of another open file. The diagnostic goes to that
    // file and main does not change.
    client.open(
        "/app/src/util.emel",
        1,
        "import Main\nimport Nope\nconst U = 1\n",
    );
    let published = client.diagnostics_n(2);
    assert_eq!(published[0].uri, uri("/app/src/main.emel"));
    assert_eq!(codes(&published[0]), ["E0205"]);
    assert_eq!(published[1].uri, uri("/app/src/util.emel"));
    assert_eq!(published[1].version, Some(1));
    assert_eq!(codes(&published[1]), ["E0205"]);
    assert_eq!(published[1].diagnostics[0].range, range((1, 7), (1, 11)));

    // A file not yet on disk becomes a module once opened.
    client.open("/app/src/http/clinet.emel", 1, "const GET = 2\n");
    let published = client.diagnostics_n(2);
    // main's error is gone, so it is cleared with an empty array (clearing is sent first).
    assert_eq!(published[0].uri, uri("/app/src/main.emel"));
    assert!(published[0].diagnostics.is_empty());
    assert_eq!(published[1].uri, uri("/app/src/util.emel"));
    assert_eq!(codes(&published[1]), ["E0205"]);
    client.shutdown();
}

/// The server does not go to the network: dependencies it cannot use (here, no `Pome.lock`) are
/// reported on `Pome.toml` (spec 4.7).
#[test]
fn reports_dependencies_that_cannot_be_used() {
    let manifest = "[pome]\nname = \"github.com/me/app\"\nversion = \"0.1.0\"\nemela = \"0.21.0\"\n\n[dependencies]\n\"github.com/acme/x\" = \"v1.0.0\"\n";
    let files = [
        ("/app/Pome.toml", manifest),
        ("/app/src/main.emel", "const X = 1\n"),
    ];
    let client = Client::initialize(&files, Some("/app"), ParseOnly::new);
    let published = client.diagnostics();
    assert_eq!(published.uri, uri("/app/Pome.toml"));
    assert_eq!(codes(&published), ["E0236"]);
    assert!(
        published.diagnostics[0]
            .message
            .contains("emela pome install"),
        "{:?}",
        published.diagnostics
    );
    client.shutdown();
}

#[test]
fn close_reverts_to_disk() {
    let files = [
        ("/app/Pome.toml", ""),
        ("/app/src/main.emel", "const X = \"\\q\"\n"),
    ];
    let client = Client::initialize(&files, Some("/app"), ParseOnly::new);
    let published = client.diagnostics();
    assert_eq!(codes(&published), ["E0107"]);
    assert_eq!(published.version, None);

    client.open("/app/src/main.emel", 1, "const X = 1\n");
    let published = client.diagnostics();
    assert!(published.diagnostics.is_empty());
    assert_eq!(published.version, Some(1));

    // Saving does not change the diagnostics (the disk is not written).
    client.save("/app/src/main.emel");
    client.close("/app/src/main.emel");
    let published = client.diagnostics();
    assert_eq!(published.uri, uri("/app/src/main.emel"));
    assert_eq!(published.version, None);
    assert_eq!(codes(&published), ["E0107"]);
    client.shutdown();
}

/// Even if the editor spells the URI differently from the server (here, parentheses are not
/// encoded), the old spelling is cleared first, so the last diagnostics sent for the file remain.
#[test]
fn uri_spelling_changes_clear_first() {
    let files = [
        ("/w/proj (1)/Pome.toml", ""),
        ("/w/proj (1)/src/main.emel", "const X = \"\\q\"\n"),
    ];
    let main = "/w/proj%20(1)/src/main.emel";
    let client = Client::initialize(&files, Some("/w/proj%20(1)"), ParseOnly::new);
    let published = client.diagnostics();
    assert_eq!(
        published.uri.as_str(),
        "file:///w/proj%20%281%29/src/main.emel"
    );
    assert_eq!(codes(&published), ["E0107"]);

    client.open(main, 1, "const X = \"\\q\"\n");
    let cleared = client.diagnostics();
    assert_eq!(
        cleared.uri.as_str(),
        "file:///w/proj%20%281%29/src/main.emel"
    );
    assert!(cleared.diagnostics.is_empty());
    let published = client.diagnostics();
    assert_eq!(published.uri, uri(main));
    assert_eq!(codes(&published), ["E0107"]);

    client.close(main);
    let cleared = client.diagnostics();
    assert_eq!(cleared.uri, uri(main));
    assert!(cleared.diagnostics.is_empty());
    let published = client.diagnostics();
    assert_eq!(
        published.uri.as_str(),
        "file:///w/proj%20%281%29/src/main.emel"
    );
    assert_eq!(codes(&published), ["E0107"]);
    client.shutdown();
}

#[test]
fn closing_a_single_file_clears_it() {
    let files = [("/tmp/x/main.emel", "")];
    let client = Client::initialize(&files, None, ParseOnly::new);
    client.open("/tmp/x/main.emel", 1, "const X = \"\\q\"\n");
    assert_eq!(codes(&client.diagnostics()), ["E0107"]);
    client.close("/tmp/x/main.emel");
    let published = client.diagnostics();
    assert_eq!(published.uri, uri("/tmp/x/main.emel"));
    assert!(published.diagnostics.is_empty());
    client.shutdown();
}

#[test]
fn consecutive_changes_publish_only_the_latest() {
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", "")];
    let (mut client, server) = Client::new();
    // Queue everything before starting the server. Pending edits are analyzed once together.
    client.send_initialize(Some("/app"));
    client.open("/app/src/main.emel", 1, "const X = $\n");
    client.change("/app/src/main.emel", 2, "const A = 3px\n");
    client.change("/app/src/main.emel", 3, "const B = 1\nconst C = \"\n");
    client.start(server, &files, ParseOnly::new);
    assert!(matches!(client.recv(), Message::Response(_)));
    let published = client.diagnostics();
    assert_eq!(published.version, Some(3));
    assert_eq!(codes(&published), ["E0101"]);
    assert_eq!(published.diagnostics[0].range, range((1, 10), (1, 11)));
    // Diagnostics for the old version never arrive (the shutdown response comes next).
    client.shutdown();
}

#[test]
fn file_outside_the_source_root() {
    let files = [
        ("/app/Pome.toml", ""),
        ("/app/src/main.emel", "const X = 1\n"),
        ("/app/scripts/x.emel", ""),
    ];
    let client = Client::initialize(&files, Some("/app"), ParseOnly::new);
    client.open("/app/scripts/x.emel", 1, "const Y = 1\n");
    let published = client.diagnostics();
    assert_eq!(published.uri, uri("/app/scripts/x.emel"));
    assert_eq!(codes(&published), ["E0208"]);
    assert_eq!(published.diagnostics[0].range, range((0, 0), (0, 0)));
    assert!(
        published.diagnostics[0]
            .message
            .contains("note: source files of a project"),
        "{}",
        published.diagnostics[0].message
    );
    client.shutdown();
}

#[test]
fn unsupported_requests_get_an_error() {
    let mut client = Client::initialize(&[("/tmp/main.emel", "")], None, ParseOnly::new);
    let id = client.request("textDocument/codeLens", json!({}));
    match client.recv() {
        Message::Response(response) => {
            assert_eq!(response.id, id);
            assert_eq!(
                response.response_result.unwrap_err().code,
                lsp_server::ErrorCode::MethodNotFound as i32
            );
        }
        other => panic!("{other:?}"),
    }
    client.shutdown();
}

#[test]
fn formatting_replaces_the_whole_document() {
    let files = [
        ("/app/Pome.toml", ""),
        ("/app/src/main.emel", "fn main() { 1 }\n"),
        ("/app/src/util.emel", "fn f( ) {1}\n"),
    ];
    let mut client = Client::initialize(&files, Some("/app"), ParseOnly::new);
    // Formatting is announced in the capabilities.
    assert_eq!(
        emela_lsp::capabilities().document_formatting_provider,
        Some(lsp::OneOf::Left(true))
    );
    // An open document formats its unsaved contents. The range covers the whole document
    // (through the final newline).
    client.open("/app/src/main.emel", 1, "fn main( ) {\n  \"あ\"}");
    let edits = client.format("/app/src/main.emel").unwrap();
    assert_eq!(
        edits,
        vec![lsp::TextEdit {
            range: range((0, 0), (1, 6)),
            new_text: "fn main() {\n  \"あ\"\n}\n".to_owned(),
        }]
    );
    // No edits if already formatted.
    client.change("/app/src/main.emel", 2, "fn main() { 1 }\n");
    assert_eq!(client.format("/app/src/main.emel"), Some(vec![]));
    // The contents on disk if not open.
    let edits = client.format("/app/src/util.emel").unwrap();
    assert_eq!(edits[0].range, range((0, 0), (1, 0)));
    assert_eq!(edits[0].new_text, "fn f() { 1 }\n");
    // null on syntax errors.
    client.change("/app/src/main.emel", 3, "fn main( {\n");
    assert_eq!(client.format("/app/src/main.emel"), None);
    // null for unreadable files too.
    assert_eq!(client.format("/app/src/nowhere.emel"), None);
    // Read the diagnostics that have not arrived yet before finishing.
    while let Ok(message) = client
        .connection
        .receiver
        .recv_timeout(Duration::from_millis(200))
    {
        assert!(matches!(message, Message::Notification(_)), "{message:?}");
    }
    client.shutdown();
}

/// Hover shows the type and E (spec 9.3).
#[test]
fn hover_shows_types_and_errors() {
    let src = "error NotFound(id: Int)\n\nfn find(id: Int) -> Int fails NotFound {\n  if id > 0 { id } else { fail NotFound(id:) }\n}\n\nfn main() {\n  n = find(1)\n  List.map([n], fn(x) { x })\n}\n";
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    assert_eq!(
        emela_lsp::capabilities().hover_provider,
        Some(lsp::HoverProviderCapability::Simple(true))
    );
    let text = |hover: Option<lsp::Hover>| match hover.map(|h| h.contents) {
        Some(lsp::HoverContents::Markup(m)) => m.value,
        other => panic!("{other:?}"),
    };
    // A definition's name shows the declared type.
    assert_eq!(
        text(client.hover("/app/src/main.emel", 2, 4)),
        "```emela\nfind: fn(Int) -> Int fails NotFound\n```"
    );
    // Inferred E.
    assert_eq!(
        text(client.hover("/app/src/main.emel", 6, 4)),
        "```emela\nmain: fn() -> List[Int] fails NotFound\n```"
    );
    // A call expression shows its type and E; the range is the expression.
    let hover = client.hover("/app/src/main.emel", 7, 12).unwrap();
    assert_eq!(hover.range, Some(range((7, 6), (7, 13))));
    assert_eq!(text(Some(hover)), "```emela\nInt\nfails NotFound\n```");
    // A local variable.
    assert_eq!(
        text(client.hover("/app/src/main.emel", 7, 2)),
        "```emela\nn: Int\n```"
    );
    // A reference to a generic function shows the generalized type, including the invisible sets
    // of function arguments.
    assert_eq!(
        text(client.hover("/app/src/main.emel", 8, 8)),
        "```emela\nmap: ∀ A, B, E(f), R(f). fn(List[A], fn(A) -> B fails E(f) use R(f)) -> List[B] fails E(f) use R(f)\n```"
    );
    // A position with nothing there.
    assert_eq!(client.hover("/app/src/main.emel", 1, 0), None);
    // null for frontends without types.
    client.shutdown();
    let mut client = Client::initialize(&files, Some("/app"), ParseOnly::new);
    assert_eq!(client.hover("/app/src/main.emel", 2, 4), None);
    client.shutdown();
}

/// A label shows its declaration, at the declaration and at references (issue #255). Types keep
/// showing the label itself.
#[test]
fn hover_on_labels() {
    let src = "effect Log {\n  fn log(msg: String) -> ()\n}\n\neffect Debug of Log default\n\nfn b() -> () use { Debug } {\n  with Debug { (use Log).log(\"b\") }\n}\n";
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let text = |hover: Option<lsp::Hover>| match hover.map(|h| h.contents) {
        Some(lsp::HoverContents::Markup(m)) => m.value,
        other => panic!("{other:?}"),
    };
    let decl = "```emela\neffect Debug of Log default\n```";
    assert_eq!(text(client.hover("/app/src/main.emel", 4, 8)), decl);
    // In the `use` clause, and in `with`.
    assert_eq!(text(client.hover("/app/src/main.emel", 6, 20)), decl);
    assert_eq!(text(client.hover("/app/src/main.emel", 7, 8)), decl);
    assert_eq!(
        text(client.hover("/app/src/main.emel", 6, 3)),
        "```emela\nb: fn() -> () use Debug\n```"
    );
    client.shutdown();
}

/// Hover over a negative number pattern (spec 6.5) and the arm around it.
#[test]
fn hover_on_negative_number_pattern() {
    let src = "fn sign(n: Int) -> Int {\n  n match {\n    -1 -> n\n    1 -> 1\n    _ -> 0\n  }\n}\n\nfn main() {\n  sign(-1)\n}\n";
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let text = |hover: Option<lsp::Hover>| {
        hover.map(|h| match h.contents {
            lsp::HoverContents::Markup(m) => (m.value, h.range),
            other => panic!("{other:?}"),
        })
    };
    // On the `-` and on the digits, the same as a positive literal pattern (`1`).
    let positive = text(client.hover("/app/src/main.emel", 3, 4));
    assert_eq!(
        positive,
        Some(("```emela\nInt\n```".to_owned(), Some(range((1, 2), (5, 3)))))
    );
    assert_eq!(text(client.hover("/app/src/main.emel", 2, 4)), positive);
    assert_eq!(text(client.hover("/app/src/main.emel", 2, 5)), positive);
    // The arm's body after the pattern.
    assert_eq!(
        text(client.hover("/app/src/main.emel", 2, 10)),
        Some((
            "```emela\nn: Int\n```".to_owned(),
            Some(range((2, 10), (2, 11)))
        ))
    );
    client.shutdown();
}

#[test]
fn hover_shows_effects() {
    let src = "effect Logger {\n  fn info(msg: String) -> ()\n}\n\nhandler Quiet: Logger {\n  fn info(msg) { () }\n}\n\nfn greet() {\n  log = use Logger\n  log.info(\"hi\")\n}\n\nfn main() {\n  with Quiet { greet() }\n}\n";
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let text = |hover: Option<lsp::Hover>| match hover.map(|h| h.contents) {
        Some(lsp::HoverContents::Markup(m)) => m.value,
        other => panic!("{other:?}"),
    };
    // Inferred R.
    assert_eq!(
        text(client.hover("/app/src/main.emel", 8, 4)),
        "```emela\ngreet: fn() -> () use Logger\n```"
    );
    // A local variable holding a capability.
    assert_eq!(
        text(client.hover("/app/src/main.emel", 9, 2)),
        "```emela\nlog: use Logger\n```"
    );
    // An operation call expression shows its type and R.
    assert_eq!(
        text(client.hover("/app/src/main.emel", 10, 16)),
        "```emela\n()\nuse Logger\n```"
    );
    // A call inside `with` shows R; outside it does not.
    assert_eq!(
        text(client.hover("/app/src/main.emel", 14, 21)),
        "```emela\n()\nuse Logger\n```"
    );
    assert_eq!(
        text(client.hover("/app/src/main.emel", 14, 3)),
        "```emela\n()\n```"
    );
    assert_eq!(
        text(client.hover("/app/src/main.emel", 13, 4)),
        "```emela\nmain: fn() -> ()\n```"
    );
    client.shutdown();
}

/// Suspending operations and handlers that leave Async (spec 8.4, 8.7). Operation types get
/// `suspend`, and handler names show their dependencies.
#[test]
fn hover_shows_suspend_and_async() {
    let src = "effect Clock {\n  suspend fn sleep(ms: Int) -> ()\n}\n\n@external(js, \"setTimeout\")\nfn set_timeout(ms: Int, f: fn() -> ()) -> ()\n\nhandler JsClock: Clock {\n  suspend fn sleep(ms, resume) {\n    set_timeout(ms, fn() { resume(()) })\n  }\n}\n\nfn main() {\n  with JsClock { (use Clock).sleep(1) }\n}\n";
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let text = |hover: Option<lsp::Hover>| match hover.map(|h| h.contents) {
        Some(lsp::HoverContents::Markup(m)) => m.value,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        text(client.hover("/app/src/main.emel", 1, 14)),
        "```emela\nsleep: suspend fn(Int) -> ()\n```"
    );
    assert_eq!(
        text(client.hover("/app/src/main.emel", 7, 10)),
        "```emela\nhandler JsClock: Clock\nuse Async\n```"
    );
    // `resume` takes the operation's return type and returns `()`.
    assert_eq!(
        text(client.hover("/app/src/main.emel", 8, 24)),
        "```emela\nresume: fn(()) -> ()\n```"
    );
    assert_eq!(
        text(client.hover("/app/src/main.emel", 14, 31)),
        "```emela\nsuspend fn(Int) -> () use Clock\nuse Clock\n```"
    );
    // Async may remain in main's R (spec 8.3).
    assert_eq!(
        text(client.hover("/app/src/main.emel", 13, 4)),
        "```emela\nmain: fn() -> () use Async\n```"
    );
    client.shutdown();
}

/// Stateful handlers and layers (spec 8.4, 8.5). A handler's name shows its fields, the E of
/// `init` and its dependencies; a layer's name shows its handlers in initialization order and
/// the unmet dependencies; `self` shows the handler value's type.
#[test]
fn hover_shows_stateful_handlers_and_layers() {
    let src = "error Down\n\neffect Config {\n  fn url() -> String\n}\n\neffect Users {\n  fn find(id: Int) -> String\n}\n\nfn connect(url: String) -> String fails Down { url }\n\nhandler DbUsers(conn: String): Users {\n  init { DbUsers(conn: connect((use Config).url())) }\n  fn find(id) { self.conn }\n}\n\nlayer App {\n  DbUsers\n}\n";
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let text = |hover: Option<lsp::Hover>| match hover.map(|h| h.contents) {
        Some(lsp::HoverContents::Markup(m)) => m.value,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        text(client.hover("/app/src/main.emel", 12, 10)),
        "```emela\nhandler DbUsers(conn: String): Users\ninit fails Down\nuse Config\n```"
    );
    assert_eq!(
        text(client.hover("/app/src/main.emel", 14, 16)),
        "```emela\nself: DbUsers\n```"
    );
    assert_eq!(
        text(client.hover("/app/src/main.emel", 17, 7)),
        "```emela\nlayer App { DbUsers }\nuse Config\n```"
    );
    client.shutdown();
}

/// `[effect.handler]`: the hover on a handler shows the parameters of its init, a parameter of
/// init has its type, and a name in
/// the arguments of a `with` item goes to its definition.
#[test]
fn handler_init_arguments() {
    let src = "effect Kv {\n  fn get(key: String) -> String\n}\n\nconst NAME: String = \"KV\"\n\nhandler WorkerKv(binding: String): Kv {\n  init(binding: String, retries: Int) { WorkerKv(binding:) }\n  fn get(key) { self.binding }\n}\n\nfn main() {\n  with WorkerKv(binding: NAME, retries: 1) { (use Kv).get(\"a\") }\n}\n";
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let text = |hover: Option<lsp::Hover>| match hover.map(|h| h.contents) {
        Some(lsp::HoverContents::Markup(m)) => m.value,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        text(client.hover("/app/src/main.emel", 6, 10)),
        "```emela\nhandler WorkerKv(binding: String): Kv\ninit(binding: String, retries: Int)\n```"
    );
    assert_eq!(
        text(client.hover("/app/src/main.emel", 7, 8)),
        "```emela\nbinding: String\n```"
    );
    let def = client
        .definition("/app/src/main.emel", 12, 26)
        .expect("a definition");
    assert_eq!(def.range, range((4, 6), (4, 10)));
    client.shutdown();
}

/// Effects with type arguments (spec 8.1). The handler name shows `handler H: Cell[Int]`, a
/// capability shows `use Cell[Int]`, and an operation shows its type with the arguments substituted.
#[test]
fn hover_shows_effect_type_args() {
    let src = "effect Cell[S] {\n  fn get() -> S\n}\n\nhandler Zero: Cell[Int] {\n  fn get() { 0 }\n}\n\nfn read() -> Int use Cell[Int] {\n  cell = use Cell[Int]\n  cell.get()\n}\n";
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let text = |hover: Option<lsp::Hover>| match hover.map(|h| h.contents) {
        Some(lsp::HoverContents::Markup(m)) => m.value,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        text(client.hover("/app/src/main.emel", 4, 9)),
        "```emela\nhandler Zero: Cell[Int]\n```"
    );
    assert_eq!(
        text(client.hover("/app/src/main.emel", 9, 3)),
        "```emela\ncell: use Cell[Int]\n```"
    );
    assert_eq!(
        text(client.hover("/app/src/main.emel", 10, 8)),
        "```emela\nfn() -> Int use Cell[Int]\n```"
    );
    client.shutdown();
}

/// A use clause mixing effects and set type parameters (spec 8.3). Inside `with` the listed effect
/// is visible; outside it only the set type parameter remains.
#[test]
fn hover_shows_mixed_effect_sets() {
    let src = "effect Quiet {\n  fn hush() -> ()\n}\n\nhandler Silent: Quiet {\n  fn hush() { () }\n}\n\nfn run[R](f: fn() -> () use { Quiet, R }) -> () use R {\n  with Silent { f() }\n}\n";
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let text = |hover: Option<lsp::Hover>| match hover.map(|h| h.contents) {
        Some(lsp::HoverContents::Markup(m)) => m.value,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        text(client.hover("/app/src/main.emel", 8, 4)),
        "```emela\nrun: ∀ R, E(f). fn(fn() -> () fails E(f) use { Quiet, R }) -> () fails E(f) use R\n```"
    );
    // The call expression's R includes the set type parameter.
    assert_eq!(
        text(client.hover("/app/src/main.emel", 9, 17)),
        "```emela\n()\nfails E(f)\nuse { Quiet, R }\n```"
    );
    // Outside `with`, Quiet is gone.
    assert_eq!(
        text(client.hover("/app/src/main.emel", 9, 3)),
        "```emela\n()\nfails E(f)\nuse R\n```"
    );
    client.shutdown();
}

fn navigation<T: serde::de::DeserializeOwned>(
    client: &mut Client,
    method: &str,
    params: serde_json::Value,
) -> T {
    let id = client.request(method, params);
    let response = client.response(&id);
    serde_json::from_value(response.response_result.unwrap()).unwrap()
}

#[test]
fn navigation_capabilities() {
    let caps = emela_lsp::capabilities();
    assert_eq!(
        caps.document_highlight_provider,
        Some(lsp::OneOf::Left(true))
    );
    assert_eq!(caps.workspace_symbol_provider, Some(lsp::OneOf::Left(true)));
    assert_eq!(
        caps.folding_range_provider,
        Some(lsp::FoldingRangeProviderCapability::Simple(true))
    );
    assert_eq!(
        caps.selection_range_provider,
        Some(lsp::SelectionRangeProviderCapability::Simple(true))
    );
    assert_eq!(
        caps.type_definition_provider,
        Some(lsp::TypeDefinitionProviderCapability::Simple(true))
    );
    assert_eq!(
        caps.implementation_provider,
        Some(lsp::ImplementationProviderCapability::Simple(true))
    );
}

#[test]
fn highlights_are_same_file_and_include_the_binding() {
    let mut client = Client::initialize(&nav_files(), Some("/app"), Typecheck::default);
    let highlights: Vec<lsp::DocumentHighlight> = navigation(
        &mut client,
        "textDocument/documentHighlight",
        Client::position(MAIN, 17, 7),
    );
    assert_eq!(
        highlights.iter().map(|h| h.range).collect::<Vec<_>>(),
        [range((1, 15), (1, 20)), range((17, 6), (17, 11))]
    );
    assert!(
        highlights
            .iter()
            .all(|h| h.kind == Some(lsp::DocumentHighlightKind::TEXT))
    );
    let highlights: Vec<lsp::DocumentHighlight> = navigation(
        &mut client,
        "textDocument/documentHighlight",
        Client::position(MAIN, 12, 3),
    );
    assert_eq!(
        highlights.iter().map(|h| h.range).collect::<Vec<_>>(),
        [range((12, 2), (12, 5)), range((13, 2), (13, 5))]
    );
    let empty: Vec<lsp::DocumentHighlight> = navigation(
        &mut client,
        "textDocument/documentHighlight",
        Client::position(MAIN, 2, 0),
    );
    assert!(empty.is_empty());
    drain(&client);
    client.shutdown();
}

#[test]
fn workspace_symbols_search_unopened_modules_and_unsaved_contents() {
    let mut files = nav_files().to_vec();
    files.push(("/other/Pome.toml", ""));
    files.push(("/other/src/lib.emel", "pub type OtherShape(id: Int)\n"));
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let symbols: Vec<lsp::SymbolInformation> =
        navigation(&mut client, "workspace/symbol", json!({ "query": "sHaP" }));
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].name, "Shape");
    assert_eq!(
        symbols[0].location,
        location(SHAPES, (0, 9), (0, 14)).unwrap()
    );
    assert_eq!(symbols[0].container_name.as_deref(), Some("Shapes"));
    client.open(
        SHAPES,
        1,
        "pub type NewShape(id: Int)\nfn broken() { missing }\n",
    );
    let symbols: Vec<lsp::SymbolInformation> =
        navigation(&mut client, "workspace/symbol", json!({ "query": "shape" }));
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].name, "NewShape");
    let all: Vec<lsp::SymbolInformation> =
        navigation(&mut client, "workspace/symbol", json!({ "query": "" }));
    assert!(
        all.iter()
            .all(|s| s.location.uri.as_str().starts_with("file:///app/"))
    );
    let empty: Vec<lsp::SymbolInformation> = navigation(
        &mut client,
        "workspace/symbol",
        json!({ "query": "no_such_symbol" }),
    );
    assert!(empty.is_empty());
    drain(&client);
    client.shutdown();
}

#[test]
fn syntax_navigation_works_with_parse_only_and_utf16() {
    let path = "/tmp/main.emel";
    let text = "# 😀 one\r\n## two\r\nimport First\r\nimport Second\r\nfn broken() {\r\n  \"😀\"\r\n  unknown match {\r\n    _ -> 1\r\n  }\r\n}\r\nfn unfinished(\r\n";
    let mut client = Client::initialize(&[(path, "")], None, ParseOnly::new);
    client.open(path, 1, text);
    let folds: Vec<lsp::FoldingRange> = navigation(
        &mut client,
        "textDocument/foldingRange",
        json!({ "textDocument": { "uri": uri(path) } }),
    );
    assert_eq!(
        folds
            .iter()
            .map(|f| (f.start_line, f.end_line, f.kind.clone()))
            .collect::<Vec<_>>(),
        [
            (0, 1, Some(lsp::FoldingRangeKind::Comment)),
            (2, 3, Some(lsp::FoldingRangeKind::Imports)),
            (4, 9, Some(lsp::FoldingRangeKind::Region)),
            (6, 8, Some(lsp::FoldingRangeKind::Region)),
        ]
    );
    let selections: Vec<lsp::SelectionRange> = navigation(
        &mut client,
        "textDocument/selectionRange",
        json!({
            "textDocument": { "uri": uri(path) }, "positions": [
                { "line": 0, "character": 4 }, { "line": 5, "character": 3 }, { "line": 6, "character": 3 }
            ]
        }),
    );
    assert_eq!(selections.len(), 3);
    assert_eq!(selections[0].range, range((0, 0), (0, 8)));
    assert_eq!(selections[1].range, range((5, 3), (5, 5)));
    assert_eq!(selections[2].range, range((6, 2), (6, 9)));
    for selection in &selections {
        let mut current = selection;
        while let Some(parent) = current.parent.as_deref() {
            assert!(
                parent.range.start <= current.range.start && parent.range.end >= current.range.end
            );
            assert_ne!(parent.range, current.range);
            current = parent;
        }
    }
    client.change(path, 2, "fn empty() {}\n");
    let folds: Vec<lsp::FoldingRange> = navigation(
        &mut client,
        "textDocument/foldingRange",
        json!({ "textDocument": { "uri": uri(path) } }),
    );
    assert!(folds.is_empty());
    drain(&client);
    client.shutdown();
}

#[test]
fn type_and_implementation_navigation_with_type_errors() {
    let data = "pub type Boxed(value: Int)\npub enum Choice { One }\npub trait Speak {\n  fn speak(self) -> String\n}\nimpl Boxed: Speak {\n  fn speak(self) { \"boxed\" }\n}\nimpl Choice: Speak {\n  fn speak(self) { \"choice\" }\n}\n";
    let text = "import Data.{Boxed, Speak}\nfn call(item: Boxed) {\n  copy = item\n  Speak.speak(copy)\n}\nfn broken() -> Int { missing }\n";
    let mut client = Client::initialize(
        &[
            ("/app/Pome.toml", ""),
            (MAIN, text),
            ("/app/src/data.emel", data),
        ],
        Some("/app"),
        Typecheck::default,
    );
    let found: Option<lsp::Location> = navigation(
        &mut client,
        "textDocument/typeDefinition",
        Client::position(MAIN, 3, 15),
    );
    assert_eq!(found, location("/app/src/data.emel", (0, 9), (0, 14)));
    let found: Option<lsp::Location> = navigation(
        &mut client,
        "textDocument/typeDefinition",
        Client::position(MAIN, 5, 16),
    );
    assert_eq!(found, None);
    let impls: Vec<lsp::Location> = navigation(
        &mut client,
        "textDocument/implementation",
        Client::position(MAIN, 3, 4),
    );
    assert_eq!(
        impls,
        [
            location("/app/src/data.emel", (5, 0), (7, 1)).unwrap(),
            location("/app/src/data.emel", (8, 0), (10, 1)).unwrap()
        ]
    );
    let methods: Vec<lsp::Location> = navigation(
        &mut client,
        "textDocument/implementation",
        Client::position(MAIN, 3, 9),
    );
    assert_eq!(
        methods,
        [
            location("/app/src/data.emel", (6, 5), (6, 10)).unwrap(),
            location("/app/src/data.emel", (9, 5), (9, 10)).unwrap()
        ]
    );
    let empty: Vec<lsp::Location> = navigation(
        &mut client,
        "textDocument/implementation",
        Client::position(MAIN, 1, 9),
    );
    assert!(empty.is_empty());
    drain(&client);
    client.shutdown();
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
const SHAPES: &str = "/app/src/shapes.emel";
const LOG: &str = "/app/src/util/log.emel";

#[test]
fn definition_of_names() {
    let mut client = Client::initialize(&nav_files(), Some("/app"), Typecheck::default);
    assert_eq!(
        emela_lsp::capabilities().definition_provider,
        Some(lsp::OneOf::Left(true))
    );
    // An effect operation (the `info` of `log.info`).
    assert_eq!(
        client.definition(MAIN, 13, 7),
        location(MAIN, (4, 5), (4, 9))
    );
    // From a handler's operation name to the effect's operation.
    assert_eq!(
        client.definition(MAIN, 8, 6),
        location(MAIN, (4, 5), (4, 9))
    );
    // A local variable.
    assert_eq!(
        client.definition(MAIN, 13, 3),
        location(MAIN, (12, 2), (12, 5))
    );
    assert_eq!(
        client.definition(MAIN, 13, 23),
        location(MAIN, (11, 9), (11, 13))
    );
    // The module left of `.` goes to the top of its file; the function right of it to its
    // definition.
    assert_eq!(
        client.definition(MAIN, 13, 12),
        location(LOG, (0, 0), (0, 0))
    );
    assert_eq!(
        client.definition(MAIN, 13, 17),
        location(LOG, (0, 7), (0, 13))
    );
    // The import path and an imported name. `Util` is not a module, so null.
    assert_eq!(
        client.definition(MAIN, 0, 13),
        location(LOG, (0, 0), (0, 0))
    );
    assert_eq!(client.definition(MAIN, 0, 8), None);
    assert_eq!(
        client.definition(MAIN, 1, 16),
        location(SHAPES, (0, 9), (0, 14))
    );
    assert_eq!(
        client.definition(MAIN, 1, 23),
        location(SHAPES, (2, 7), (2, 11))
    );
    // Effect, handler, constructor, function. The position right after a name counts as the name.
    assert_eq!(
        client.definition(MAIN, 12, 12),
        location(MAIN, (3, 7), (3, 13))
    );
    assert_eq!(
        client.definition(MAIN, 7, 17),
        location(MAIN, (3, 7), (3, 13))
    );
    assert_eq!(
        client.definition(MAIN, 18, 9),
        location(MAIN, (7, 8), (7, 13))
    );
    assert_eq!(
        client.definition(MAIN, 17, 6),
        location(SHAPES, (0, 9), (0, 14))
    );
    assert_eq!(
        client.definition(MAIN, 18, 20),
        location(MAIN, (11, 3), (11, 8))
    );
    assert_eq!(
        client.definition(MAIN, 19, 6),
        location(SHAPES, (2, 7), (2, 11))
    );
    // A definition's name points to itself.
    assert_eq!(
        client.definition(MAIN, 11, 4),
        location(MAIN, (11, 3), (11, 8))
    );
    // A type position.
    assert_eq!(
        client.definition(SHAPES, 2, 16),
        location(SHAPES, (0, 9), (0, 14))
    );
    // A position with nothing there.
    assert_eq!(client.definition(MAIN, 2, 0), None);
    client.shutdown();
}

/// The argument of `@entry` (`[ext.entry]`) goes to the entry adapter, and its module to the top
/// of the module's file.
#[test]
fn definition_of_an_entry_adapter() {
    let main = "import Http\n\n@entry(Http.serve)\nfn app(x: Int) -> Int { x }\n\nfn main() {}\n";
    let http = "@export(js, \"default\", \"fetch\")\npub fn serve(app: fn(Int) -> Int fails {} use {}, x: Int) -> Int { app(x) }\n";
    let files = [
        ("/app/Pome.toml", ""),
        ("/app/src/main.emel", main),
        ("/app/src/http.emel", http),
    ];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let http_path = "/app/src/http.emel";
    assert_eq!(
        client.definition(MAIN, 2, 13),
        location(http_path, (1, 7), (1, 12))
    );
    assert_eq!(
        client.definition(MAIN, 2, 8),
        location(http_path, (0, 0), (0, 0))
    );
    // The fn that `@entry` makes has no source: hover and inlay hints do not show it.
    let hover = format!("{:?}", client.hover(MAIN, 2, 2));
    assert!(!hover.contains("$entry"), "{hover}");
    let hints = client.inlay_hints(MAIN).unwrap_or_default();
    assert!(hints.iter().all(|(line, ..)| *line != 2), "{hints:?}");
    client.shutdown();
}

/// Definitions in the bundled core have no location to go to.
#[test]
fn definition_of_core_is_null() {
    let src = "fn main() {\n  List.map([1], fn(x) { x })\n}\n";
    let files = [("/tmp/main.emel", src)];
    let mut client = Client::initialize(&files, None, Typecheck::default);
    assert_eq!(client.definition("/tmp/main.emel", 1, 3), None);
    assert_eq!(client.definition("/tmp/main.emel", 1, 8), None);
    // A local variable in a standalone file can be navigated.
    assert_eq!(
        client.definition("/tmp/main.emel", 1, 24),
        location("/tmp/main.emel", (1, 19), (1, 20))
    );
    // null for frontends that do not resolve names.
    client.shutdown();
    let mut client = Client::initialize(&files, None, ParseOnly::new);
    assert_eq!(client.definition("/tmp/main.emel", 1, 24), None);
    client.shutdown();
}

#[test]
fn references_in_the_project() {
    let mut client = Client::initialize(&nav_files(), Some("/app"), Typecheck::default);
    assert_eq!(
        emela_lsp::capabilities().references_provider,
        Some(lsp::OneOf::Left(true))
    );
    let main = |s, e| (MAIN.to_owned(), range(s, e));
    let shapes = |s, e| (SHAPES.to_owned(), range(s, e));
    // A constructor with the same name as its type counts as the same thing. Includes the
    // declaration.
    assert_eq!(
        client.references(SHAPES, 0, 10, true),
        Some(vec![
            main((1, 15), (1, 20)),
            main((17, 6), (17, 11)),
            shapes((0, 9), (0, 14)),
            shapes((2, 15), (2, 20)),
        ])
    );
    // Same result from a reference position. Excludes the declaration.
    assert_eq!(
        client.references(MAIN, 17, 7, false),
        Some(vec![
            main((1, 15), (1, 20)),
            main((17, 6), (17, 11)),
            shapes((2, 15), (2, 20)),
        ])
    );
    // An effect operation: the handler's implementation and calls on the capability.
    assert_eq!(
        client.references(MAIN, 4, 6, false),
        Some(vec![main((8, 5), (8, 9)), main((13, 6), (13, 10))])
    );
    // A local variable.
    assert_eq!(
        client.references(MAIN, 12, 3, true),
        Some(vec![main((12, 2), (12, 5)), main((13, 2), (13, 5))])
    );
    // A module: the import path and the left of `.`.
    assert_eq!(
        client.references(MAIN, 13, 12, true),
        Some(vec![main((0, 12), (0, 15)), main((13, 11), (13, 14))])
    );
    // A position with nothing there.
    assert_eq!(client.references(MAIN, 2, 0, true), None);
    client.shutdown();
}

#[test]
fn document_symbols_list_declarations() {
    let mut client = Client::initialize(&nav_files(), Some("/app"), ParseOnly::new);
    assert_eq!(
        emela_lsp::capabilities().document_symbol_provider,
        Some(lsp::OneOf::Left(true))
    );
    fn outline(symbols: &[lsp::DocumentSymbol]) -> Vec<String> {
        symbols
            .iter()
            .map(|s| {
                let children = outline(s.children.as_deref().unwrap_or_default());
                let kind = format!("{:?}", s.kind);
                if children.is_empty() {
                    format!("{} {kind}", s.name)
                } else {
                    format!("{} {kind} [{}]", s.name, children.join(", "))
                }
            })
            .collect()
    }
    let symbols = client.document_symbols(MAIN).unwrap();
    assert_eq!(
        outline(&symbols),
        [
            "Logger Interface [info Method]",
            "Quiet Class [info Method]",
            "greet Function",
            "main Function",
        ]
    );
    // The range is the whole declaration; the selection range is the name.
    assert_eq!(symbols[0].range, range((3, 0), (5, 1)));
    assert_eq!(symbols[0].selection_range, range((3, 7), (3, 13)));
    let symbols = client.document_symbols(SHAPES).unwrap();
    assert_eq!(
        outline(&symbols),
        ["Shape Struct [w Field, h Field]", "area Function"]
    );
    // An open document uses unsaved contents. Declarations that parsed are listed despite syntax
    // errors.
    let src = "enum Color { Red\n Green }\n\nconst MAX: Int = 3\n\ntrait Named {\n  fn name(self) -> String\n}\n\nimpl Color: Named {\n  fn name(self) { \"c\" }\n}\n\nfn broken( {\n";
    client.open("/app/src/other.emel", 1, src);
    assert_eq!(
        outline(&client.document_symbols("/app/src/other.emel").unwrap()),
        [
            "Color Enum [Red EnumMember, Green EnumMember]",
            "MAX Constant",
            "Named Interface [name Method]",
            "impl Color: Named Namespace [name Method]",
            "broken Function",
        ]
    );
    // null for unreadable files.
    assert_eq!(client.document_symbols("/app/src/nowhere.emel"), None);
    while let Ok(message) = client
        .connection
        .receiver
        .recv_timeout(Duration::from_millis(200))
    {
        assert!(matches!(message, Message::Notification(_)), "{message:?}");
    }
    client.shutdown();
}

fn labels(items: &[(String, lsp::CompletionItemKind, Option<String>)]) -> Vec<&str> {
    items.iter().map(|(l, _, _)| l.as_str()).collect()
}

#[test]
fn completion_after_dot() {
    let mut client = Client::initialize(&nav_files(), Some("/app"), Typecheck::default);
    assert_eq!(
        emela_lsp::capabilities()
            .completion_provider
            .and_then(|c| c.trigger_characters),
        Some(vec![".".to_owned()])
    );
    // Operations of a capability. Same candidates with or without a partially typed name.
    let src = NAV_MAIN.replace("  log.info(Log.prefix(name))\n", "  log.\n  log.in\n");
    client.open(MAIN, 1, &src);
    let items = client.complete(MAIN, 13, 6).unwrap();
    assert_eq!(
        items,
        [(
            "info".to_owned(),
            lsp::CompletionItemKind::METHOD,
            Some("fn(String) -> ()".to_owned())
        )]
    );
    assert_eq!(labels(&client.complete(MAIN, 14, 8).unwrap()), ["info"]);
    // A module's pub definitions.
    client.change(MAIN, 2, &NAV_MAIN.replace("Log.prefix(name)", "Log."));
    assert_eq!(labels(&client.complete(MAIN, 13, 15).unwrap()), ["prefix"]);
    // A type's fields, with the declared type as detail.
    client.change(MAIN, 3, &NAV_MAIN.replace("  area(s)\n", "  s.\n"));
    assert_eq!(
        client.complete(MAIN, 19, 4).unwrap(),
        [
            (
                "w".to_owned(),
                lsp::CompletionItemKind::FIELD,
                Some("Int".to_owned())
            ),
            (
                "h".to_owned(),
                lsp::CompletionItemKind::FIELD,
                Some("Int".to_owned())
            ),
        ]
    );
    // core modules usable without an import.
    client.change(MAIN, 4, &NAV_MAIN.replace("  area(s)\n", "  List.\n"));
    let items = client.complete(MAIN, 19, 7).unwrap();
    assert!(labels(&items).contains(&"map"), "{items:?}");
    // Nothing after an expression of unknown type.
    client.change(MAIN, 5, &NAV_MAIN.replace("  area(s)\n", "  1.\n"));
    assert_eq!(client.complete(MAIN, 19, 4), Some(vec![]));
    drain(&client);
    client.shutdown();
}

#[test]
fn completion_in_imports() {
    let mut client = Client::initialize(&nav_files(), Some("/app"), Typecheck::default);
    let src = format!("import \n{NAV_MAIN}");
    client.open(MAIN, 1, &src);
    // The first segment of a module path (the current module is excluded).
    let items = client.complete(MAIN, 0, 7).unwrap();
    assert!(labels(&items).contains(&"Shapes"), "{items:?}");
    assert!(labels(&items).contains(&"Util"), "{items:?}");
    assert!(!labels(&items).contains(&"Main"), "{items:?}");
    assert!(items.iter().all(|i| i.1 == lsp::CompletionItemKind::MODULE));
    // Later segments offer only module names (no function names).
    client.change(MAIN, 2, &format!("import Util.\n{NAV_MAIN}"));
    assert_eq!(
        client.complete(MAIN, 0, 12).unwrap(),
        [(
            "Log".to_owned(),
            lsp::CompletionItemKind::MODULE,
            Some("Util.Log".to_owned())
        )]
    );
    // Inside braces, the module's pub names.
    client.change(MAIN, 3, &format!("import Shapes.{{a}}\n{NAV_MAIN}"));
    assert_eq!(
        labels(&client.complete(MAIN, 0, 16).unwrap()),
        ["Shape", "area"]
    );
    drain(&client);
    client.shutdown();
}

#[test]
fn completion_of_names_in_scope() {
    let mut client = Client::initialize(&nav_files(), Some("/app"), Typecheck::default);
    // The body of greet: locals (innermost first), module definitions, imported names, modules,
    // keywords.
    let src = NAV_MAIN.replace(
        "  log.info(Log.prefix(name))\n",
        "  log.info(Log.prefix(name))\n  n\n",
    );
    client.open(MAIN, 1, &src);
    let items = client.complete(MAIN, 14, 3).unwrap();
    let names = labels(&items);
    assert_eq!(names[..2], ["log", "name"]);
    assert_eq!(items[0].2.as_deref(), Some("use Logger"));
    assert_eq!(items[1].2.as_deref(), Some("String"));
    assert!(!names.contains(&"Prelude"), "{names:?}");
    for name in [
        "greet", "main", "Logger", "Quiet", "Shape", "area", "Log", "List", "Some", "match",
    ] {
        assert!(names.contains(&name), "{name}: {names:?}");
    }
    // No locals of other functions and no names that were not imported.
    assert!(!names.contains(&"s"), "{names:?}");
    assert!(!names.contains(&"prefix"), "{names:?}");
    // A variable is not visible before its binding or inside its right-hand side.
    let items = client.complete(MAIN, 12, 2).unwrap();
    assert!(!labels(&items).contains(&"log"), "{items:?}");
    let items = client.complete(MAIN, 12, 12).unwrap();
    assert!(!labels(&items).contains(&"log"), "{items:?}");
    // Only effects after `use`; only handlers and layers after `with`.
    let items = client.complete(MAIN, 12, 12).unwrap();
    assert!(labels(&items).contains(&"Logger"), "{items:?}");
    assert!(
        items
            .iter()
            .all(|i| i.1 == lsp::CompletionItemKind::INTERFACE),
        "{items:?}"
    );
    let items = client.complete(MAIN, 19, 7).unwrap();
    assert_eq!(labels(&items), ["Quiet"]);
    drain(&client);
    client.shutdown();
}

fn drain(client: &Client) {
    while let Ok(message) = client
        .connection
        .receiver
        .recv_timeout(Duration::from_millis(200))
    {
        assert!(matches!(message, Message::Notification(_)), "{message:?}");
    }
}

#[test]
fn signature_help_in_calls() {
    let mut client = Client::initialize(&nav_files(), Some("/app"), Typecheck::default);
    let caps = emela_lsp::capabilities();
    assert_eq!(
        caps.signature_help_provider
            .and_then(|s| s.trigger_characters),
        Some(vec!["(".to_owned(), ",".to_owned()])
    );
    let s = |label: &str, params: &[&str], active: Option<u32>| {
        Some((
            label.to_owned(),
            params.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>(),
            active,
        ))
    };
    // The innermost call.
    assert_eq!(
        client.signature(MAIN, 13, 22),
        s("prefix(s: String) -> String", &["s: String"], Some(0))
    );
    // An effect operation.
    assert_eq!(
        client.signature(MAIN, 13, 11),
        s("info(msg: String) -> ()", &["msg: String"], Some(0))
    );
    // A constructor. The active parameter is decided by the number of commas.
    let shape = "Shape(w: Int, h: Int) -> Shape";
    assert_eq!(
        client.signature(MAIN, 17, 12),
        s(shape, &["w: Int", "h: Int"], Some(0))
    );
    assert_eq!(
        client.signature(MAIN, 17, 18),
        s(shape, &["w: Int", "h: Int"], Some(1))
    );
    // A partially written call, and a parameter decided by its label.
    client.open(
        MAIN,
        1,
        &NAV_MAIN.replace("Shape(w: 1, h: 2)", "Shape(h: 2, w:"),
    );
    assert_eq!(
        client.signature(MAIN, 17, 20),
        s(shape, &["w: Int", "h: Int"], Some(0))
    );
    // Outside a call.
    client.change(MAIN, 2, NAV_MAIN);
    assert_eq!(client.signature(MAIN, 12, 2), None);
    assert_eq!(client.signature(MAIN, 17, 23), None);
    drain(&client);
    client.shutdown();
}

const PIPE_MAIN: &str = "effect Logger {
  fn info(msg: String) -> ()
}

handler Quiet: Logger {
  fn info(msg) { () }
}

fn add(a: Int, b: Int) -> Int { a + b }

fn greet(s: String) {
  log = use Logger
  s |> log.info
  s |> log.info()
}

fn main() {
  xs = [1, 2, 3]
  ys = xs |> List.map(fn(x) { x |> add(1) })
  n = xs |> List.fold(from: 0, f: add)
  m = 1 |> add(2) |> add(3)
  k = add(1, 2) |> add(3)
  with Quiet { greet(\"a\") }
}
";

#[test]
fn signature_help_on_the_right_of_a_pipe() {
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", PIPE_MAIN)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let s = |label: &str, params: &[&str], active: Option<u32>| {
        Some((
            label.to_owned(),
            params.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>(),
            active,
        ))
    };
    // The left side fills the first parameter, so it is left out.
    let add_b = s("add(b: Int) -> Int", &["b: Int"], Some(0));
    let f = "f: fn(A) -> B fails E(f) use R(f)";
    assert_eq!(
        client.signature(MAIN, 18, 22),
        s(
            &format!("map({f}) -> List[B] fails E(f) use R(f)"),
            &[f],
            Some(0)
        )
    );
    // Nested in a lambda on the right of another pipe.
    assert_eq!(client.signature(MAIN, 18, 39), add_b);
    // The other parameters given by name.
    let f = "f: fn(B, A) -> B fails E(f) use R(f)";
    let fold = format!("fold(from: B, {f}) -> B fails E(f) use R(f)");
    for (col, active) in [(22, 0), (32, 1), (34, 1)] {
        assert_eq!(
            client.signature(MAIN, 19, col),
            s(&fold, &["from: B", f], Some(active))
        );
    }
    // A chain of pipes.
    assert_eq!(client.signature(MAIN, 20, 15), add_b);
    assert_eq!(client.signature(MAIN, 20, 25), add_b);
    // A call on the left of a pipe keeps all its parameters.
    assert_eq!(
        client.signature(MAIN, 21, 10),
        s("add(a: Int, b: Int) -> Int", &["a: Int", "b: Int"], Some(0))
    );
    assert_eq!(client.signature(MAIN, 21, 23), add_b);
    // An operation of a capability value.
    assert_eq!(client.signature(MAIN, 13, 16), s("info() -> ()", &[], None));
    // The first parameter's label is not offered.
    let items = client.complete(MAIN, 19, 22).unwrap();
    let labels: Vec<&str> = labels(&items)
        .into_iter()
        .filter(|l| l.ends_with(':'))
        .collect();
    assert_eq!(labels, ["from:", "f:"]);
    // Naming the first parameter is an error, so no parameter is active.
    client.open(
        MAIN,
        1,
        &PIPE_MAIN.replace("List.fold(from: 0, f: add)", "List.fold(xs: "),
    );
    assert_eq!(
        client.signature(MAIN, 19, 26),
        s(&fold, &["from: B", f], None)
    );
    drain(&client);
    client.shutdown();
}

#[test]
fn rename_across_files() {
    let mut client = Client::initialize(&nav_files(), Some("/app"), Typecheck::default);
    assert!(matches!(
        emela_lsp::capabilities().rename_provider,
        Some(lsp::OneOf::Right(lsp::RenameOptions {
            prepare_provider: Some(true),
            ..
        }))
    ));
    let edit = |path: &str, s, e, text: &str| (path.to_owned(), range(s, e), text.to_owned());
    // A type and its constructor are renamed together, including import names.
    assert_eq!(
        client.rename(MAIN, 17, 7, "Figure"),
        Ok(vec![
            edit(MAIN, (1, 15), (1, 20), "Figure"),
            edit(MAIN, (17, 6), (17, 11), "Figure"),
            edit(SHAPES, (0, 9), (0, 14), "Figure"),
            edit(SHAPES, (2, 15), (2, 20), "Figure"),
        ])
    );
    // An effect operation: the declaration, the handler's implementation and the calls.
    assert_eq!(
        client.rename(MAIN, 4, 6, "log_info"),
        Ok(vec![
            edit(MAIN, (4, 5), (4, 9), "log_info"),
            edit(MAIN, (8, 5), (8, 9), "log_info"),
            edit(MAIN, (13, 6), (13, 10), "log_info"),
        ])
    );
    // A local variable.
    assert_eq!(
        client.rename(MAIN, 11, 10, "who"),
        Ok(vec![
            edit(MAIN, (11, 9), (11, 13), "who"),
            edit(MAIN, (13, 22), (13, 26), "who"),
        ])
    );
    // Names of the wrong lexical class, and reserved words.
    assert_eq!(
        client.rename(MAIN, 17, 7, "figure"),
        Err("`figure` is not a valid type name".to_owned())
    );
    assert_eq!(
        client.rename(MAIN, 11, 10, "match"),
        Err("`match` is not a valid lowercase name".to_owned())
    );
    // Modules cannot be renamed.
    assert_eq!(
        client.rename(MAIN, 13, 12, "Logs"),
        Err("this name cannot be renamed".to_owned())
    );
    // prepareRename returns the name's range, or null if it cannot be renamed.
    assert_eq!(
        client.prepare_rename(MAIN, 13, 7),
        Some(range((13, 6), (13, 10)))
    );
    assert_eq!(client.prepare_rename(MAIN, 13, 12), None);
    assert_eq!(client.prepare_rename(MAIN, 2, 0), None);
    client.shutdown();
}

/// Shorthand named arguments and field patterns keep their labels. Names in the bundled core
/// cannot be renamed.
#[test]
fn rename_keeps_punned_labels() {
    let src = "error NotFound(id: Int)\n\ntype User(name: String)\n\nfn find(id: Int) -> Int fails NotFound {\n  fail NotFound(id:)\n}\n\nfn greet(u: User) -> String {\n  u match {\n    User(name:) -> name\n  }\n}\n\nfn main() {\n  List.map([1], fn(x) { x })\n}\n";
    let path = "/tmp/main.emel";
    let mut client = Client::initialize(&[(path, src)], None, Typecheck::default);
    let edit = |s, e, text: &str| (path.to_owned(), range(s, e), text.to_owned());
    assert_eq!(
        client.rename(path, 4, 8, "key"),
        Ok(vec![
            edit((4, 8), (4, 10), "key"),
            edit((5, 16), (5, 18), "id: key")
        ])
    );
    assert_eq!(
        client.rename(path, 10, 20, "who"),
        Ok(vec![
            edit((10, 9), (10, 13), "name: who"),
            edit((10, 19), (10, 23), "who")
        ])
    );
    assert_eq!(client.prepare_rename(path, 15, 8), None);
    assert_eq!(client.prepare_rename(path, 15, 3), None);
    client.shutdown();
}

/// Inserts inferred results into omitted annotations (spec 4.5).
#[test]
fn code_action_inserts_inferred_annotations() {
    let main = "import Errs.{NotFound}
import Other

effect Logger {
  fn info(msg: String) -> ()
}

fn find(id: Int) {
  if id > 0 { id } else { fail NotFound(id:) }
}

fn greet() {
  log = use Logger
  log.info(\"hi\")
}

fn both(id: Int) -> Int {
  (use Logger).info(\"x\")
  id
}

fn other() {
  fail Other.Hidden(n: 1)
}

fn apply[A](f: fn() -> A) {
  f()
}
";
    let files = [
        ("/app/Pome.toml", ""),
        ("/app/src/main.emel", main),
        ("/app/src/errs.emel", "pub error NotFound(id: Int)\n"),
        ("/app/src/other.emel", "pub error Hidden(n: Int)\n"),
    ];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    assert!(emela_lsp::capabilities().code_action_provider.is_some());
    let action = |title: &str, at: (u32, u32), text: &str| {
        Some(vec![(
            title.to_owned(),
            vec![(range(at, at), text.to_owned())],
        )])
    };
    // The return type and the inferred E are added together.
    assert_eq!(
        client.code_actions(MAIN, (7, 4), (7, 4)),
        action(
            "Add inferred `-> Int fails NotFound`",
            (7, 16),
            " -> Int fails NotFound"
        )
    );
    // Inferred R.
    assert_eq!(
        client.code_actions(MAIN, (11, 0), (11, 0)),
        action(
            "Add inferred `-> () use Logger`",
            (11, 10),
            " -> () use Logger"
        )
    );
    // With a return type present, only `use` is added, after the return type.
    assert_eq!(
        client.code_actions(MAIN, (16, 5), (16, 5)),
        action("Add inferred `use Logger`", (16, 23), " use Logger")
    );
    // Errors that cannot be written unqualified are not added.
    assert_eq!(client.code_actions(MAIN, (21, 4), (21, 4)), Some(vec![]));
    // The invisible sets of function arguments are not written (they are implicitly added to
    // written clauses).
    assert_eq!(
        client.code_actions(MAIN, (25, 4), (25, 4)),
        action("Add inferred `-> A`", (25, 25), " -> A")
    );
    // Not offered inside a body.
    assert_eq!(client.code_actions(MAIN, (8, 4), (8, 4)), Some(vec![]));
    client.shutdown();
}

#[test]
fn quickfixes_link_only_the_diagnostic_at_the_requested_range() {
    let main = "enum Color { Red, Blue }\nfn color(x: Color) -> Int { x match { Red -> 1 } }\nfn first(count: Int) -> Int { cout }\nfn second(total: Int) -> Int { tota }\ntrait Show { fn show(self) -> String }\nimpl Color: Show {}\n";
    let files = [("/app/Pome.toml", ""), (MAIN, main)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    // Read the workspace's first diagnostics, or they can arrive after the last response and
    // fail `shutdown`.
    let published = client.diagnostics();
    assert_eq!(
        codes(&published),
        ["E0211", "E0211", "W0202", "E0303", "E0325"]
    );
    let capabilities = emela_lsp::capabilities();
    let Some(lsp::CodeActionProviderCapability::Options(options)) =
        capabilities.code_action_provider
    else {
        panic!("code action options")
    };
    assert!(
        options
            .code_action_kinds
            .unwrap()
            .contains(&lsp::CodeActionKind::QUICKFIX)
    );

    for (at, code, inserted) in [
        ((1, 28), "E0303", "Blue -> todo()"),
        ((2, 30), "E0211", "count"),
        ((3, 31), "E0211", "total"),
        ((5, 1), "E0325", "fn show(self) { todo() }"),
    ] {
        let actions = client.quickfixes(MAIN, at, at);
        assert_eq!(actions.len(), 1, "at {at:?}: {actions:?}");
        let action = &actions[0];
        let diagnostic = &action.diagnostics.as_ref().unwrap()[0];
        assert_eq!(
            diagnostic.code,
            Some(lsp::NumberOrString::String(code.to_owned()))
        );
        assert!(diagnostic.range.start <= lsp::Position::new(at.0, at.1));
        assert!(diagnostic.range.end > lsp::Position::new(at.0, at.1));
        let edits = &action.edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri(MAIN)];
        assert_eq!(edits.len(), 1);
        assert!(edits[0].new_text.contains(inserted));
    }
    assert!(client.quickfixes(MAIN, (1, 0), (1, 0)).is_empty());
    assert_eq!(client.quickfixes(MAIN, (1, 0), (6, 0)).len(), 4);
    // The snapshot is rechecked even when the client echoes no diagnostics.
    client.open(MAIN, 1, main);
    client.change(MAIN, 2, &main.replace("cout", "count"));
    assert!(client.quickfixes(MAIN, (2, 30), (2, 30)).is_empty());
    client.shutdown();
}

#[test]
fn impl_stubs_are_quickfixes_linked_to_published_diagnostics() {
    let main = "type User(id: Int)\ntrait Label {\n  fn show(self) -> String\n  fn merge(self, other: Self) -> Self\n  fn default(self) -> Int { 1 }\n}\nimpl User: Label {\n  fn show(self) { \"user\" }\n}\nimpl Int: Label {}\n";
    let files = [("/app/Pome.toml", ""), (MAIN, main)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let published = client.diagnostics();
    assert_eq!(codes(&published), ["E0325", "E0325"]);
    assert_eq!(published.diagnostics[0].range, range((6, 0), (6, 16)));
    assert_eq!(published.diagnostics[1].range, range((9, 0), (9, 15)));
    let id = client.request(
        CodeActionRequest::METHOD,
        json!({
            "textDocument": { "uri": uri(MAIN) },
            "range": range((6, 0), (6, 0)),
            "context": { "diagnostics": [] },
        }),
    );
    let actions: Vec<lsp::CodeAction> =
        serde_json::from_value(client.response(&id).response_result.unwrap()).unwrap();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].kind, Some(lsp::CodeActionKind::QUICKFIX));
    // The old header refactoring is not offered alongside the quickfix.
    let id = client.request(
        CodeActionRequest::METHOD,
        json!({
            "textDocument": { "uri": uri(MAIN) },
            "range": range((6, 0), (6, 0)),
            "context": { "diagnostics": [], "only": ["refactor.rewrite"] },
        }),
    );
    let actions: Vec<lsp::CodeAction> =
        serde_json::from_value(client.response(&id).response_result.unwrap()).unwrap();
    assert!(actions.is_empty());
    assert_eq!(
        published.diagnostics[0].message,
        "incomplete impl: missing methods `merge`"
    );
    assert_eq!(
        published.diagnostics[1].message,
        "incomplete impl: missing methods `show`, `merge`"
    );
    for at in [(6, 0), (6, 5), (6, 15)] {
        let actions = client.quickfixes(MAIN, at, at);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].title, "Implement missing trait methods");
        assert_eq!(
            actions[0].diagnostics.as_ref().unwrap(),
            &vec![published.diagnostics[0].clone()]
        );
        let edits = &actions[0].edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri(MAIN)];
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "\n  fn merge(self, other) { todo() }");
    }
    assert_eq!(client.quickfixes(MAIN, (6, 5), (6, 18)).len(), 1);
    for at in [(6, 19), (6, 20), (6, 21), (7, 2), (7, 18), (8, 0), (8, 1)] {
        assert!(client.quickfixes(MAIN, at, at).is_empty(), "at {at:?}");
    }
    assert!(client.quickfixes(MAIN, (6, 20), (8, 1)).is_empty());
    let actions = client.quickfixes(MAIN, (9, 0), (9, 0));
    assert_eq!(actions.len(), 1);
    let edits = &actions[0].edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri(MAIN)];
    assert!(edits[0].new_text.contains("fn show(self) { todo() }"));
    assert!(
        edits[0]
            .new_text
            .contains("fn merge(self, other) { todo() }")
    );
    assert!(!edits[0].new_text.contains("fn default"));
    assert_eq!(client.quickfixes(MAIN, (6, 0), (10, 0)).len(), 2);
    client.open(MAIN, 1, main);
    client.change(
        MAIN,
        2,
        &main.replace(
            "fn merge(self, other: Self) -> Self",
            "fn merge(self, other: Self) -> Self { self }",
        ),
    );
    assert!(client.quickfixes(MAIN, (6, 0), (6, 0)).is_empty());
    client.shutdown();
}

#[test]
fn quickfixes_insert_imports_and_extend_effect_and_error_annotations() {
    let main = "error Missing\neffect Log { fn log() -> () }\nfn io() -> () { Io.print(\"hi\") }\nfn fails() -> () use {} { fail Missing }\nfn effects() -> () use {} { (use Log).log() }\n";
    let files = [
        ("/app/Pome.toml", ""),
        (MAIN, main),
        ("/app/src/io.emel", "pub fn print(s: String) -> () {}\n"),
    ];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    // Read the workspace's first diagnostics, or they can arrive after the last response and
    // fail `shutdown`.
    let published = client.diagnostics();
    assert_eq!(codes(&published), ["E0211", "E0318", "E0402"]);
    for (at, code, inserted) in [
        ((2, 16), "E0211", "import Io\n"),
        ((3, 26), "E0318", " fails Missing"),
        ((4, 29), "E0402", "use Log"),
    ] {
        let actions = client.quickfixes(MAIN, at, at);
        assert_eq!(actions.len(), 1, "at {at:?}: {actions:?}");
        let action = &actions[0];
        assert_eq!(
            action.diagnostics.as_ref().unwrap()[0].code,
            Some(lsp::NumberOrString::String(code.to_owned()))
        );
        let edits = &action.edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri(MAIN)];
        assert_eq!(edits[0].new_text, inserted);
    }
    client.shutdown();
}

/// Shows the unwritten types, E and R inline (spec 9.3).
#[test]
fn inlay_hints_show_types_errors_and_effects() {
    let src = "error NotFound(id: Int)

fn find(id: Int) -> Int fails NotFound {
  if id > 0 { id } else { fail NotFound(id:) }
}

effect Logger {
  fn info(msg: String) -> ()
}

handler Quiet: Logger {
  fn info(msg) { () }
}

fn greet() {
  log = use Logger
  log.info(\"hi\")
  n = find(1)
  List.map([n], fn(x) { x })
}

fn main() {
  with Quiet { greet() }
}
";
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    assert_eq!(
        emela_lsp::capabilities().inlay_hint_provider,
        Some(lsp::OneOf::Left(true))
    );
    let hint = |line, col, label: &str, padding| (line, col, label.to_owned(), padding);
    assert_eq!(
        client.inlay_hints(MAIN).unwrap(),
        [
            // Parameter types of a handler operation.
            hint(11, 13, ": String", false),
            // An omitted annotation, including invisible sets.
            hint(14, 10, "-> List[Int] fails NotFound use Logger", true),
            // E and R of a call in statement position.
            hint(16, 16, "use Logger", true),
            // The type of a binding (not shown for the capability `log`).
            hint(17, 3, ": Int", false),
            hint(17, 13, "fails NotFound", true),
            // Lambda parameters. Calls with neither E nor R get no hint.
            hint(18, 20, ": Int", false),
            hint(21, 9, "-> List[Int] fails NotFound", true),
            // A call inside `with` shows R.
            hint(22, 22, "fails NotFound use Logger", true),
        ]
    );
    // Written annotations get no hints. Frontends without types give none.
    client.shutdown();
    let mut client = Client::initialize(&files, Some("/app"), ParseOnly::new);
    assert_eq!(client.inlay_hints(MAIN), Some(vec![]));
    client.shutdown();
}

const CAPABILITY_MAIN: &str = "effect Logger {
  fn info(msg: String) -> ()
}

handler Quiet: Logger {
  fn info(msg) { () }
}

handler Loud: Logger {
  fn info(msg) { () }
}

fn greet(names: List[String]) {
  log = use Logger
  List.map(names, fn(n) { log.info(\"hi #{n}\") })
  (use Logger).info(\"bye\")
}

fn main() {
  with Quiet {
    outer = use Logger
    with Loud {
      inner = use Logger
      inner.info(\"a\")
      outer.info(\"b\")
    }
    greet([\"x\"])
  }
}
";

/// The variable type with the `capability` modifier (bit 0).
fn cap(line: u32, col: u32, len: u32) -> Token {
    (line, col, len, 0, 1)
}

/// Capability values (spec 8.2) are marked where bound and where used, including inside closures
/// and nested `with`. Nothing else is marked.
#[test]
fn semantic_tokens_mark_capability_values() {
    let files = [
        ("/app/Pome.toml", ""),
        ("/app/src/main.emel", CAPABILITY_MAIN),
    ];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let Some(lsp::SemanticTokensServerCapabilities::SemanticTokensOptions(options)) =
        emela_lsp::capabilities().semantic_tokens_provider
    else {
        panic!("no semantic tokens options");
    };
    assert_eq!(
        options.legend.token_types,
        [lsp::SemanticTokenType::VARIABLE]
    );
    assert_eq!(
        options.legend.token_modifiers,
        [lsp::SemanticTokenModifier::new("capability")]
    );
    assert_eq!(options.range, Some(true));
    assert_eq!(
        client.semantic_tokens(MAIN, None).unwrap(),
        [
            // The binding and its use in a closure.
            cap(13, 2, 3),
            cap(14, 26, 3),
            // Nested `with`: both the outer and the inner value.
            cap(20, 4, 5),
            cap(22, 6, 5),
            cap(23, 6, 5),
            cap(24, 6, 5),
        ]
    );
    // A range gets only the tokens inside it.
    assert_eq!(
        client
            .semantic_tokens(MAIN, Some(range((22, 0), (24, 0))))
            .unwrap(),
        [cap(22, 6, 5), cap(23, 6, 5)]
    );
    client.shutdown();
    // Frontends without types give none.
    let mut client = Client::initialize(&files, Some("/app"), ParseOnly::new);
    assert_eq!(client.semantic_tokens(MAIN, None), Some(vec![]));
    client.shutdown();
}

/// Errors elsewhere in the file do not hide the capability values that can still be told.
#[test]
fn semantic_tokens_survive_errors() {
    let src = "effect Logger {
  fn info(msg: String) -> ()
}

fn greet() {
  log = use Logger
  n: Int = \"not an int\"
  log.info(missing)
  bad = use Nope
  bad.info(\"x\")
}

fn broken( {
";
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    // Syntax, name and type errors.
    let published = codes(&client.diagnostics());
    for code in ["E0110", "E0211", "E0301"] {
        assert!(published.iter().any(|c| c == code), "{published:?}");
    }
    assert_eq!(
        client.semantic_tokens(MAIN, None).unwrap(),
        [
            cap(5, 2, 3),
            cap(7, 2, 3),
            // `use` of an unknown effect is still a capability value.
            cap(8, 2, 3),
            cap(9, 2, 3),
        ]
    );
    client.shutdown();
}

const CHAIN_MAIN: &str = "type Point(x: Int, y: Int)
type Line(from: Point, to: Point)
type Box[A](value: A)

effect Clock {
  fn now() -> Point
}

fn main() {
  l = Line(from: Point(x: 1, y: 2), to: Point(x: 3, y: 4))
  b = Box(value: l)
  c = use Clock
  0
}
";

/// `a.b.` chains. The left of a nameless `.` never reaches type inference, so its type is found
/// by walking from the left.
#[test]
fn completion_after_dot_chains() {
    let files = [("/app/Pome.toml", ""), (MAIN, CHAIN_MAIN)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let with = |line: &str| CHAIN_MAIN.replace("  0\n", &format!("  {line}\n"));
    // A field of a field.
    client.open(MAIN, 1, &with("l.from."));
    let items = client.complete(MAIN, 12, 9).unwrap();
    assert_eq!(labels(&items), ["x", "y"]);
    // Fields of a generic type substitute the type arguments (`value` of `Box[Line]` is `Line`).
    client.change(MAIN, 2, &with("b.value.to."));
    assert_eq!(labels(&client.complete(MAIN, 12, 13).unwrap()), ["x", "y"]);
    // The return value of a capability's operation.
    client.change(MAIN, 3, &with("c.now()."));
    assert_eq!(labels(&client.complete(MAIN, 12, 10).unwrap()), ["x", "y"]);
    // Same with a partially typed name.
    client.change(MAIN, 4, &with("l.to.x"));
    assert_eq!(labels(&client.complete(MAIN, 12, 8).unwrap()), ["x", "y"]);
    drain(&client);
    client.shutdown();
}

/// After a handler value (`self.`), its fields are offered (spec 8.4).
#[test]
fn completion_of_handler_fields() {
    let src = "effect Users {\n  fn find(id: Int) -> String\n}\n\nhandler DbUsers(url: String, port: Int): Users {\n  init { DbUsers(url: \"x\", port: 1) }\n  fn find(id) { self.url }\n}\n";
    let files = [("/app/Pome.toml", ""), (MAIN, src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let items = client.complete(MAIN, 6, 21).unwrap();
    assert_eq!(labels(&items), ["url", "port"]);
    drain(&client);
    client.shutdown();
}

/// At the start of a call argument, labels not yet written come first.
#[test]
fn completion_of_argument_labels() {
    let files = [("/app/Pome.toml", ""), (MAIN, CHAIN_MAIN)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let with = |line: &str| CHAIN_MAIN.replace("  0\n", &format!("  {line}\n"));
    let raw = |client: &mut Client, line: u32, character: u32| -> Vec<lsp::CompletionItem> {
        let id = client.request(Completion::METHOD, Client::position(MAIN, line, character));
        let result: Option<lsp::CompletionResponse> =
            serde_json::from_value(client.response(&id).response_result.unwrap()).unwrap();
        let Some(lsp::CompletionResponse::Array(mut items)) = result else {
            panic!("not an array");
        };
        items.sort_by(|a, b| a.sort_text.cmp(&b.sort_text));
        items
    };
    client.open(MAIN, 1, &with("Point("));
    let items = raw(&mut client, 12, 8);
    let first: Vec<_> = items[..2]
        .iter()
        .map(|i| {
            (
                i.label.as_str(),
                i.kind,
                i.detail.as_deref(),
                i.insert_text.as_deref(),
            )
        })
        .collect();
    let property = Some(lsp::CompletionItemKind::PROPERTY);
    assert_eq!(
        first,
        [
            ("x:", property, Some("Int"), Some("x: ")),
            ("y:", property, Some("Int"), Some("y: ")),
        ]
    );
    // Value candidates follow.
    assert!(items.iter().any(|i| i.label == "l"));
    // Written labels are not offered; still offered with a partially typed name.
    client.change(MAIN, 2, &with("Line(from: Point(x: 1, y: 2), t"));
    let items = client.complete(MAIN, 12, 33).unwrap();
    assert_eq!(
        items[0],
        (
            "to:".to_owned(),
            lsp::CompletionItemKind::PROPERTY,
            Some("Point".to_owned())
        )
    );
    assert!(!labels(&items).contains(&"from:"));
    // Not offered after a label (in value position).
    client.change(MAIN, 3, &with("Point(x: "));
    let items = client.complete(MAIN, 12, 11).unwrap();
    assert!(
        !labels(&items).iter().any(|l| l.ends_with(':')),
        "{items:?}"
    );
    // A nested call offers the inner call's labels.
    client.change(MAIN, 4, &with("Line(from: Point("));
    let items = client.complete(MAIN, 12, 19).unwrap();
    assert_eq!(labels(&items)[..2], ["x:", "y:"]);
    drain(&client);
    client.shutdown();
}

/// Requests an IDE result at a substring in the main file, keeping documentation fields.
fn doc_query(
    client: &mut Client,
    method: &str,
    text: &str,
    needle: &str,
    within: usize,
) -> serde_json::Value {
    let offset = text.find(needle).unwrap() + within;
    let prefix = &text[..offset];
    let line = prefix.bytes().filter(|&b| b == b'\n').count() as u32;
    let character = prefix.rsplit('\n').next().unwrap().encode_utf16().count() as u32;
    let id = client.request(method, Client::position(MAIN, line, character));
    client.response(&id).response_result.unwrap()
}

#[test]
fn doc_comments_are_markdown_and_follow_unsaved_changes() {
    let text = "## Doubles **値**.\n##\n##     double(2)\nfn double(value: Int) -> Int { value * 2 }\nfn main() -> Int { double(1) }\n";
    let mut client = Client::initialize(
        &[("/app/Pome.toml", ""), (MAIN, text)],
        None,
        Typecheck::default,
    );
    client.open(MAIN, 1, text);
    let doc = "Doubles **値**.\n\n    double(2)";
    let markup = json!({ "kind": "markdown", "value": doc });
    let hover = doc_query(&mut client, HoverRequest::METHOD, text, "double(1)", 0);
    assert_eq!(hover["contents"]["kind"], "markdown");
    assert_eq!(
        hover["contents"]["value"],
        format!("```emela\ndouble: fn(Int) -> Int\n```\n\n---\n\n{doc}")
    );
    let help = doc_query(
        &mut client,
        SignatureHelpRequest::METHOD,
        text,
        "double(1)",
        7,
    );
    assert_eq!(help["signatures"][0]["documentation"], markup);
    // There is no parameter-doc convention; the whole Markdown remains on the signature.
    assert!(help["signatures"][0]["parameters"][0]["documentation"].is_null());
    let items = doc_query(&mut client, Completion::METHOD, text, "double(1)", 0);
    let item = items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["label"] == "double")
        .unwrap();
    assert_eq!(item["documentation"], markup);

    let changed = text.replace("Doubles **値**.", "Updated docs.");
    client.change(MAIN, 2, &changed);
    let help = doc_query(
        &mut client,
        SignatureHelpRequest::METHOD,
        &changed,
        "double(1)",
        7,
    );
    assert_eq!(
        help["signatures"][0]["documentation"]["value"],
        "Updated docs.\n\n    double(2)"
    );
    let no_doc = "# Ordinary comment.\nfn double(value: Int) -> Int { value * 2 }\nfn main() -> Int { double(1) }\n";
    client.change(MAIN, 3, no_doc);
    let hover = doc_query(&mut client, HoverRequest::METHOD, no_doc, "double(1)", 0);
    assert_eq!(
        hover["contents"]["value"],
        "```emela\ndouble: fn(Int) -> Int\n```"
    );
    let items = doc_query(&mut client, Completion::METHOD, no_doc, "double(1)", 0);
    assert!(
        items
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["label"] == "double")
            .unwrap()["documentation"]
            .is_null()
    );
    drain(&client);
    client.shutdown();
}

#[test]
fn docs_use_definition_spans_across_modules_and_std() {
    let text = "import Alpha\nimport Beta\nimport Std.Duration\nfn main() -> Int {\n  Alpha.same(1)\n  Beta.same(2)\n  Duration.to_seconds(Duration.milliseconds(1))\n  0\n}\n";
    let alpha = "## Alpha's docs.\npub fn same(n: Int) -> Int { n }\n";
    let beta = "## Beta's docs.\npub fn same(n: Int) -> Int { n }\n";
    let mut client = Client::initialize(
        &[
            ("/app/Pome.toml", ""),
            (MAIN, text),
            ("/app/src/alpha.emel", alpha),
            ("/app/src/beta.emel", beta),
        ],
        None,
        Typecheck::default,
    );
    for (needle, name, within, doc) in [
        ("Alpha.same(1)", "same", 6, "Alpha's docs."),
        ("Beta.same(2)", "same", 5, "Beta's docs."),
        (
            "Duration.to_seconds(",
            "to_seconds",
            9,
            "Truncates toward zero.",
        ),
    ] {
        let hover = doc_query(&mut client, HoverRequest::METHOD, text, needle, within);
        assert!(
            hover["contents"]["value"]
                .as_str()
                .unwrap()
                .ends_with(&format!("\n\n---\n\n{doc}")),
            "{hover}"
        );
        let help = doc_query(
            &mut client,
            SignatureHelpRequest::METHOD,
            text,
            needle,
            within + name.len() + 1,
        );
        assert_eq!(
            help["signatures"][0]["documentation"],
            json!({"kind": "markdown", "value": doc})
        );
        let items = doc_query(&mut client, Completion::METHOD, text, needle, within);
        let item = items
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["label"] == name)
            .unwrap();
        assert_eq!(
            item["documentation"],
            json!({"kind": "markdown", "value": doc})
        );
    }
    // Reading the definition uses the overlay too, not the dependency's disk contents.
    client.open(
        "/app/src/alpha.emel",
        1,
        &alpha.replace("Alpha's docs.", "Unsaved docs."),
    );
    let help = doc_query(
        &mut client,
        SignatureHelpRequest::METHOD,
        text,
        "Alpha.same(1)",
        11,
    );
    assert_eq!(
        help["signatures"][0]["documentation"]["value"],
        "Unsaved docs."
    );
    drain(&client);
    client.shutdown();
}

#[test]
fn docs_coexist_with_highlights_and_completion_based_quickfixes() {
    let text = "## Returns a count.\nfn count(value: Int) -> Int { value }\n## A record.\ntype Point(value: Int)\ntrait Read { fn read(self) -> Int }\n## Implements reading.\nimpl Point: Read {\n  ## Reads the field.\n  fn read(self) { self.value }\n}\nfn main() -> Int { count(1) }\nfn broken() -> Int { cont(2) }\n";
    let mut client = Client::initialize(
        &[("/app/Pome.toml", ""), (MAIN, text)],
        None,
        Typecheck::default,
    );
    let hover = doc_query(&mut client, HoverRequest::METHOD, text, "count(1)", 0);
    assert!(
        hover["contents"]["value"]
            .as_str()
            .unwrap()
            .ends_with("\n\n---\n\nReturns a count.")
    );
    let highlights: Vec<lsp::DocumentHighlight> = serde_json::from_value(doc_query(
        &mut client,
        "textDocument/documentHighlight",
        text,
        "count(1)",
        0,
    ))
    .unwrap();
    assert_eq!(
        highlights.iter().map(|h| h.range).collect::<Vec<_>>(),
        [range((1, 3), (1, 8)), range((10, 19), (10, 24))]
    );
    let items = doc_query(&mut client, Completion::METHOD, text, "cont(2)", 0);
    assert_eq!(
        items
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["label"] == "count")
            .unwrap()["documentation"]["value"],
        "Returns a count."
    );
    let fixes = client.quickfixes(MAIN, (11, 21), (11, 21));
    assert_eq!(fixes.len(), 1);
    let edits = &fixes[0].edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri(MAIN)];
    assert_eq!(edits[0].new_text, "count");

    // Impl methods use their own docs, while local names still resolve through symbol_at.
    let hover = doc_query(
        &mut client,
        HoverRequest::METHOD,
        text,
        "fn read(self) {",
        3,
    );
    assert!(
        hover["contents"]["value"]
            .as_str()
            .unwrap()
            .ends_with("\n\n---\n\nReads the field.")
    );
    let highlights: Vec<lsp::DocumentHighlight> = serde_json::from_value(doc_query(
        &mut client,
        "textDocument/documentHighlight",
        text,
        "fn read(self) {",
        8,
    ))
    .unwrap();
    assert_eq!(
        highlights.iter().map(|h| h.range).collect::<Vec<_>>(),
        [range((8, 10), (8, 14)), range((8, 18), (8, 22))]
    );
    let empty = doc_query(
        &mut client,
        "textDocument/documentHighlight",
        text,
        "impl Point: Read {",
        20,
    );
    assert_eq!(empty, json!([]));
    assert!(
        doc_query(
            &mut client,
            HoverRequest::METHOD,
            text,
            "impl Point: Read {",
            20
        )
        .is_null()
    );
    drain(&client);
    client.shutdown();
}

/// The old impl and handler heads come with quick fixes that rewrite them (E0144, E0145).
#[test]
fn old_forms_have_rewriting_quickfixes() {
    let main = "trait Label { fn label(self) -> String }\nimpl Label for Int {\n  fn label(self) { \"n\" }\n}\neffect Log { fn log() -> () }\nhandler Quiet implements Log {\n  fn log() { () }\n}\n";
    let files = [("/app/Pome.toml", ""), (MAIN, main)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let published = client.diagnostics();
    assert_eq!(codes(&published), ["E0144", "E0145"]);
    for ((line, character), expected) in [((1, 0), "impl Int: Label"), ((5, 14), ":")] {
        let actions = client.quickfixes(MAIN, (line, character), (line, character));
        assert_eq!(actions.len(), 1, "at {line}:{character}");
        let edits = &actions[0].edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri(MAIN)];
        assert_eq!(edits[0].new_text, expected);
    }
    client.shutdown();
}
