//! メモリ上の接続でサーバーを立て，メッセージをやりとりして診断を確かめる．

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
    Initialize, PrepareRenameRequest, References, Rename, Request as _, Shutdown,
    SignatureHelpRequest,
};
use lsp_types::{self as lsp, Uri};
use serde_json::json;

/// コードアクションの題と編集（範囲，挿入するテキスト）．
type Action = (String, Vec<(lsp::Range, String)>);

const TIMEOUT: Duration = Duration::from_secs(10);

fn uri(path: &str) -> Uri {
    Uri::from_str(&format!("file://{path}")).unwrap()
}

/// エディタの側．サーバーは別のスレッドで動かす．
struct Client {
    connection: Connection,
    server: Option<JoinHandle<()>>,
    next_id: i32,
}

impl Client {
    /// 送るメッセージを先に溜めてからサーバーを立てられるように，起動は [`Client::start`] で分ける．
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

    /// initialize と initialized を送る．応答はまだ読まない．
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
            panic!("initialize の応答がない: {response:?}");
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
            .expect("サーバーから応答がない")
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

    /// `id` の要求の応答．先に届いた publishDiagnostics は読み飛ばす（解析と要求の順は決まらない）．
    fn response(&self, id: &RequestId) -> lsp_server::Response {
        loop {
            match self.recv() {
                Message::Response(response) if response.id == *id => return response,
                Message::Notification(n) if n.method == PublishDiagnostics::METHOD => {}
                other => panic!("応答の前に届いた: {other:?}"),
            }
        }
    }

    /// 文書の整形を求め，結果（`null` なら `None`）を返す．
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

    /// 位置の情報を求め，結果（`null` なら `None`）を返す．
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

    /// 定義の位置を求め，結果（`null` なら `None`）を返す．
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
            other => panic!("位置が1つでない: {other:?}"),
        }
    }

    /// 参照を求め，（パス，範囲）をパスと位置の順に並べて返す．`null` なら `None`．
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

    /// 文書のシンボルを求める．`null` なら `None`．
    fn document_symbols(&mut self, path: &str) -> Option<Vec<lsp::DocumentSymbol>> {
        let id = self.request(
            DocumentSymbolRequest::METHOD,
            json!({ "textDocument": { "uri": uri(path) } }),
        );
        let result: Option<lsp::DocumentSymbolResponse> =
            serde_json::from_value(self.response(&id).response_result.unwrap()).unwrap();
        match result? {
            lsp::DocumentSymbolResponse::Nested(symbols) => Some(symbols),
            other => panic!("入れ子でない: {other:?}"),
        }
    }

    /// 補完を求め，（名前，種類，説明）の並びを返す．`null` なら `None`．
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
            panic!("配列でない");
        };
        items.sort_by(|a, b| a.sort_text.cmp(&b.sort_text));
        Some(
            items
                .into_iter()
                .map(|i| (i.label, i.kind.unwrap(), i.detail))
                .collect(),
        )
    }

    /// 位置のテキスト文書の引数．
    fn position(path: &str, line: u32, character: u32) -> serde_json::Value {
        json!({
            "textDocument": { "uri": uri(path) },
            "position": { "line": line, "character": character },
        })
    }

    /// シグネチャのヘルプを求め，（ラベル，引数のラベルの文字列，カーソルのある引数）を返す．
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
                _ => panic!("位置でない"),
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
            other => panic!("範囲でない: {other:?}"),
        }
    }

    /// 名前を変える編集を，（パス，範囲，新しいテキスト）をパスと位置の順に並べて返す．
    /// 誤りの応答なら文面を返す．
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

    /// 範囲のコードアクションを求め，（題，編集）を返す．編集は（範囲，挿入するテキスト）．
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
                    other => panic!("コマンドでなくアクション: {other:?}"),
                })
                .collect(),
        )
    }

    /// 次の publishDiagnostics．
    fn diagnostics(&self) -> lsp::PublishDiagnosticsParams {
        match self.recv() {
            Message::Notification(n) if n.method == PublishDiagnostics::METHOD => {
                serde_json::from_value(n.params).unwrap()
            }
            other => panic!("publishDiagnostics でない: {other:?}"),
        }
    }

    /// 次から `n` 件の publishDiagnostics を URI の順に並べたもの．1回の解析の分は URI の順に届く．
    fn diagnostics_n(&self, n: usize) -> Vec<lsp::PublishDiagnosticsParams> {
        (0..n).map(|_| self.diagnostics()).collect()
    }

    /// shutdown と exit を送り，それまでに届いた通知が他にないことを確かめる．
    fn shutdown(mut self) {
        let id = self.request(Shutdown::METHOD, serde_json::Value::Null);
        match self.recv() {
            Message::Response(response) if response.id == id => {}
            other => panic!("shutdown の応答の前に届いた: {other:?}"),
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
            other => panic!("コードがない: {other:?}"),
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
    // ディスクの内容には誤りがないので，最初は何も届かない．
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
    // `Pome.toml` がないので単独ファイル．
    let files = [("/tmp/x/main.emel", "")];
    let client = Client::initialize(&files, None, ParseOnly::new);
    // `あ` と `日本` は UTF-16 で1単位ずつ，`😀` は2単位．UTF-8 ではそれぞれ3バイトと4バイト．
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
    // ワークスペースのプロジェクトは，ファイルを開く前から診断が出る．
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

    // 開いた別のファイルの未保存の内容で import を壊す．診断はそのファイルに出て，main は変わらない．
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

    // ディスクにまだないファイルも，開けばモジュールになる．
    client.open("/app/src/http/clinet.emel", 1, "const GET = 2\n");
    let published = client.diagnostics_n(2);
    // main の誤りが消えたので，空の配列で消す（消す方を先に送る）．
    assert_eq!(published[0].uri, uri("/app/src/main.emel"));
    assert!(published[0].diagnostics.is_empty());
    assert_eq!(published[1].uri, uri("/app/src/util.emel"));
    assert_eq!(codes(&published[1]), ["E0205"]);
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

    // 保存しても（ディスクは書き換わらないので）診断は変わらない．
    client.save("/app/src/main.emel");
    client.close("/app/src/main.emel");
    let published = client.diagnostics();
    assert_eq!(published.uri, uri("/app/src/main.emel"));
    assert_eq!(published.version, None);
    assert_eq!(codes(&published), ["E0107"]);
    client.shutdown();
}

/// エディタの URI の書き方（ここでは括弧を符号化しない）がサーバーの書き方と違っても，
/// 古い書き方の URI を先に消すので，同じファイルの診断は最後に送ったものが残る．
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
    // サーバーを立てる前に全部送っておく．届いている書き換えはまとめて1回だけ解析する．
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
    // 古い版の診断はこの後にも届かない（shutdown の応答が次に来る）．
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
    // 能力で整形を知らせる．
    assert_eq!(
        emela_lsp::capabilities().document_formatting_provider,
        Some(lsp::OneOf::Left(true))
    );
    // 開いた文書は未保存の内容を整形する．範囲は文書の全体（末尾の改行の後ろまで）．
    client.open("/app/src/main.emel", 1, "fn main( ) {\n  \"あ\"}");
    let edits = client.format("/app/src/main.emel").unwrap();
    assert_eq!(
        edits,
        vec![lsp::TextEdit {
            range: range((0, 0), (1, 6)),
            new_text: "fn main() {\n  \"あ\"\n}\n".to_owned(),
        }]
    );
    // 整っていれば編集なし．
    client.change("/app/src/main.emel", 2, "fn main() { 1 }\n");
    assert_eq!(client.format("/app/src/main.emel"), Some(vec![]));
    // 開いていなければディスクの内容．
    let edits = client.format("/app/src/util.emel").unwrap();
    assert_eq!(edits[0].range, range((0, 0), (1, 0)));
    assert_eq!(edits[0].new_text, "fn f() { 1 }\n");
    // 構文エラーなら null．
    client.change("/app/src/main.emel", 3, "fn main( {\n");
    assert_eq!(client.format("/app/src/main.emel"), None);
    // 読めないファイルも null．
    assert_eq!(client.format("/app/src/nowhere.emel"), None);
    // 先に届いていない診断を読んでから終える．
    while let Ok(message) = client
        .connection
        .receiver
        .recv_timeout(Duration::from_millis(200))
    {
        assert!(matches!(message, Message::Notification(_)), "{message:?}");
    }
    client.shutdown();
}

/// ホバーで型と E（エラーの集合）を見せる（9.3）．
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
    // 定義の名前は宣言の型．
    assert_eq!(
        text(client.hover("/app/src/main.emel", 2, 4)),
        "```emela\nfind: fn(Int) -> Int fails NotFound\n```"
    );
    // 推論した E．
    assert_eq!(
        text(client.hover("/app/src/main.emel", 6, 4)),
        "```emela\nmain: fn() -> List[Int] fails NotFound\n```"
    );
    // 呼び出しの式は型と E．範囲は式．
    let hover = client.hover("/app/src/main.emel", 7, 12).unwrap();
    assert_eq!(hover.range, Some(range((7, 6), (7, 13))));
    assert_eq!(text(Some(hover)), "```emela\nInt\nfails NotFound\n```");
    // 局所変数．
    assert_eq!(
        text(client.hover("/app/src/main.emel", 7, 2)),
        "```emela\nn: Int\n```"
    );
    // 汎用の関数の参照は汎化した型．引数の関数の見えない集合が出る．
    assert_eq!(
        text(client.hover("/app/src/main.emel", 8, 8)),
        "```emela\nmap: ∀ A, B, E(f), R(f). fn(List[A], fn(A) -> B fails E(f) use R(f)) -> List[B] fails E(f) use R(f)\n```"
    );
    // 何もない位置．
    assert_eq!(client.hover("/app/src/main.emel", 1, 0), None);
    // 型を持たないフロントエンドは null．
    client.shutdown();
    let mut client = Client::initialize(&files, Some("/app"), ParseOnly::new);
    assert_eq!(client.hover("/app/src/main.emel", 2, 4), None);
    client.shutdown();
}

#[test]
fn hover_shows_effects() {
    let src = "effect Logger {\n  fn info(msg: String) -> ()\n}\n\nhandler Quiet implements Logger {\n  fn info(msg) { () }\n}\n\nfn greet() {\n  log = use Logger\n  log.info(\"hi\")\n}\n\nfn main() {\n  with Quiet { greet() }\n}\n";
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let text = |hover: Option<lsp::Hover>| match hover.map(|h| h.contents) {
        Some(lsp::HoverContents::Markup(m)) => m.value,
        other => panic!("{other:?}"),
    };
    // 推論した R．
    assert_eq!(
        text(client.hover("/app/src/main.emel", 8, 4)),
        "```emela\ngreet: fn() -> () use Logger\n```"
    );
    // 能力の値の局所変数．
    assert_eq!(
        text(client.hover("/app/src/main.emel", 9, 2)),
        "```emela\nlog: use Logger\n```"
    );
    // 操作の呼び出しの式は型と R．
    assert_eq!(
        text(client.hover("/app/src/main.emel", 10, 16)),
        "```emela\n()\nuse Logger\n```"
    );
    // with の中の呼び出しには R が出て，with の外には出ない．
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

/// suspend な操作と，Async が残るハンドラ（8.4，8.7）．操作の型には `suspend` を付け，
/// handler の名前には依存を出す．
#[test]
fn hover_shows_suspend_and_async() {
    let src = "effect Clock {\n  suspend fn sleep(ms: Int) -> ()\n}\n\n@external(js, \"setTimeout\")\nfn set_timeout(ms: Int, f: fn() -> ()) -> ()\n\nhandler JsClock implements Clock {\n  suspend fn sleep(ms, resume) {\n    set_timeout(ms, fn() { resume(()) })\n  }\n}\n\nfn main() {\n  with JsClock { (use Clock).sleep(1) }\n}\n";
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
        "```emela\nhandler JsClock implements Clock\nuse Async\n```"
    );
    // resume は操作の戻り値の型を受けて `()` を返す関数．
    assert_eq!(
        text(client.hover("/app/src/main.emel", 8, 24)),
        "```emela\nresume: fn(()) -> ()\n```"
    );
    assert_eq!(
        text(client.hover("/app/src/main.emel", 14, 31)),
        "```emela\nsuspend fn(Int) -> () use Clock\nuse Clock\n```"
    );
    // main の R には Async が残ってよい（8.3）．
    assert_eq!(
        text(client.hover("/app/src/main.emel", 13, 4)),
        "```emela\nmain: fn() -> () use Async\n```"
    );
    client.shutdown();
}

/// エフェクトと集合の型引数を混ぜた use 節（8.3）．with の中では書き出したエフェクトが見え，
/// with の外では集合の型引数だけが残る．
#[test]
fn hover_shows_mixed_effect_sets() {
    let src = "effect Quiet {\n  fn hush() -> ()\n}\n\nhandler Silent implements Quiet {\n  fn hush() { () }\n}\n\nfn run[R](f: fn() -> () use { Quiet, R }) -> () use R {\n  with Silent { f() }\n}\n";
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
    // 呼び出しの式の R は集合の型引数を含む．
    assert_eq!(
        text(client.hover("/app/src/main.emel", 9, 17)),
        "```emela\n()\nfails E(f)\nuse { Quiet, R }\n```"
    );
    // with の外では Quiet が消える．
    assert_eq!(
        text(client.hover("/app/src/main.emel", 9, 3)),
        "```emela\n()\nfails E(f)\nuse R\n```"
    );
    client.shutdown();
}

/// 定義へ移動，参照の検索，文書のシンボルを確かめるプロジェクト．
const NAV_MAIN: &str = "import Util.Log
import Shapes.{Shape, area}

effect Logger {
  fn info(msg: String) -> ()
}

handler Quiet implements Logger {
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
    // エフェクトの操作（`log.info` の info）．
    assert_eq!(
        client.definition(MAIN, 13, 7),
        location(MAIN, (4, 5), (4, 9))
    );
    // handler の操作の名前から effect の操作へ．
    assert_eq!(
        client.definition(MAIN, 8, 6),
        location(MAIN, (4, 5), (4, 9))
    );
    // 局所変数．
    assert_eq!(
        client.definition(MAIN, 13, 3),
        location(MAIN, (12, 2), (12, 5))
    );
    assert_eq!(
        client.definition(MAIN, 13, 23),
        location(MAIN, (11, 9), (11, 13))
    );
    // `.` の左のモジュールはファイルの先頭，その右の関数は定義．
    assert_eq!(
        client.definition(MAIN, 13, 12),
        location(LOG, (0, 0), (0, 0))
    );
    assert_eq!(
        client.definition(MAIN, 13, 17),
        location(LOG, (0, 7), (0, 13))
    );
    // import のパスと取り込んだ名前．`Util` はモジュールでないので null．
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
    // エフェクト，handler，構成子，関数．名前の直後の位置も名前とみなす．
    assert_eq!(
        client.definition(MAIN, 12, 12),
        location(MAIN, (3, 7), (3, 13))
    );
    assert_eq!(
        client.definition(MAIN, 7, 25),
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
    // 定義の名前は自分自身．
    assert_eq!(
        client.definition(MAIN, 11, 4),
        location(MAIN, (11, 3), (11, 8))
    );
    // 型の位置．
    assert_eq!(
        client.definition(SHAPES, 2, 16),
        location(SHAPES, (0, 9), (0, 14))
    );
    // 何もない位置．
    assert_eq!(client.definition(MAIN, 2, 0), None);
    client.shutdown();
}

/// 同梱の core の定義には移動先がない．
#[test]
fn definition_of_core_is_null() {
    let src = "fn main() {\n  List.map([1], fn(x) { x })\n}\n";
    let files = [("/tmp/main.emel", src)];
    let mut client = Client::initialize(&files, None, Typecheck::default);
    assert_eq!(client.definition("/tmp/main.emel", 1, 3), None);
    assert_eq!(client.definition("/tmp/main.emel", 1, 8), None);
    // 単独ファイルの局所変数には移動できる．
    assert_eq!(
        client.definition("/tmp/main.emel", 1, 24),
        location("/tmp/main.emel", (1, 19), (1, 20))
    );
    // 名前を解決しないフロントエンドは null．
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
    // 型と同じ名前の構成子は同じものとみなす．定義を含める．
    assert_eq!(
        client.references(SHAPES, 0, 10, true),
        Some(vec![
            main((1, 15), (1, 20)),
            main((17, 6), (17, 11)),
            shapes((0, 9), (0, 14)),
            shapes((2, 15), (2, 20)),
        ])
    );
    // 参照の位置から引いても同じ．定義を含めない．
    assert_eq!(
        client.references(MAIN, 17, 7, false),
        Some(vec![
            main((1, 15), (1, 20)),
            main((17, 6), (17, 11)),
            shapes((2, 15), (2, 20)),
        ])
    );
    // エフェクトの操作．handler の実装と能力の値の呼び出し．
    assert_eq!(
        client.references(MAIN, 4, 6, false),
        Some(vec![main((8, 5), (8, 9)), main((13, 6), (13, 10))])
    );
    // 局所変数．
    assert_eq!(
        client.references(MAIN, 12, 3, true),
        Some(vec![main((12, 2), (12, 5)), main((13, 2), (13, 5))])
    );
    // モジュール．import のパスと `.` の左．
    assert_eq!(
        client.references(MAIN, 13, 12, true),
        Some(vec![main((0, 12), (0, 15)), main((13, 11), (13, 14))])
    );
    // 何もない位置．
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
    // 範囲は宣言の全体，選ぶ範囲は名前．
    assert_eq!(symbols[0].range, range((3, 0), (5, 1)));
    assert_eq!(symbols[0].selection_range, range((3, 7), (3, 13)));
    let symbols = client.document_symbols(SHAPES).unwrap();
    assert_eq!(
        outline(&symbols),
        ["Shape Struct [w Field, h Field]", "area Function"]
    );
    // 開いた文書は未保存の内容．構文の誤りがあっても読めた宣言は出す．
    let src = "enum Color { Red\n Green }\n\nconst MAX: Int = 3\n\ntrait Named {\n  fn name(self) -> String\n}\n\nimpl Named for Color {\n  fn name(self) { \"c\" }\n}\n\nfn broken( {\n";
    client.open("/app/src/other.emel", 1, src);
    assert_eq!(
        outline(&client.document_symbols("/app/src/other.emel").unwrap()),
        [
            "Color Enum [Red EnumMember, Green EnumMember]",
            "MAX Constant",
            "Named Interface [name Method]",
            "impl Named for Color Namespace [name Method]",
            "broken Function",
        ]
    );
    // 読めないファイルは null．
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

/// 補完の名前だけ．
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
    // 能力の値の操作．書きかけの名前があってもなくても同じ候補．
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
    // モジュールの pub な定義．
    client.change(MAIN, 2, &NAV_MAIN.replace("Log.prefix(name)", "Log."));
    assert_eq!(labels(&client.complete(MAIN, 13, 15).unwrap()), ["prefix"]);
    // type のフィールド．説明は書いた型．
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
    // import しなくても書ける core のモジュール．
    client.change(MAIN, 4, &NAV_MAIN.replace("  area(s)\n", "  List.\n"));
    let items = client.complete(MAIN, 19, 7).unwrap();
    assert!(labels(&items).contains(&"map"), "{items:?}");
    // 型の分からない式の後ろは空．
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
    // モジュール名の最初の区切り（自分は出さない）．
    let items = client.complete(MAIN, 0, 7).unwrap();
    assert!(labels(&items).contains(&"Shapes"), "{items:?}");
    assert!(labels(&items).contains(&"Util"), "{items:?}");
    assert!(!labels(&items).contains(&"Main"), "{items:?}");
    assert!(items.iter().all(|i| i.1 == lsp::CompletionItemKind::MODULE));
    // 次の区切りはモジュール名だけ（関数の名前は出さない）．
    client.change(MAIN, 2, &format!("import Util.\n{NAV_MAIN}"));
    assert_eq!(
        client.complete(MAIN, 0, 12).unwrap(),
        [(
            "Log".to_owned(),
            lsp::CompletionItemKind::MODULE,
            Some("Util.Log".to_owned())
        )]
    );
    // 波括弧の中はモジュールの pub な名前．
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
    // greet の本体．局所変数（内側が先），モジュールの定義，import した名前，モジュール，キーワード．
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
    // 別の関数の局所変数と，import していない名前は出さない．
    assert!(!names.contains(&"s"), "{names:?}");
    assert!(!names.contains(&"prefix"), "{names:?}");
    // 束縛の前と右辺の中では，その変数は見えない．
    let items = client.complete(MAIN, 12, 2).unwrap();
    assert!(!labels(&items).contains(&"log"), "{items:?}");
    let items = client.complete(MAIN, 12, 12).unwrap();
    assert!(!labels(&items).contains(&"log"), "{items:?}");
    // `use` の後ろはエフェクト，`with` の後ろは handler と layer だけ．
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

/// 先に届いていない診断を読み捨てる．
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
    // 一番内側の呼び出し．
    assert_eq!(
        client.signature(MAIN, 13, 22),
        s("prefix(s: String) -> String", &["s: String"], Some(0))
    );
    // effect の操作．
    assert_eq!(
        client.signature(MAIN, 13, 11),
        s("info(msg: String) -> ()", &["msg: String"], Some(0))
    );
    // 構成子．カンマの数で引数を決める．
    let shape = "Shape(w: Int, h: Int) -> Shape";
    assert_eq!(
        client.signature(MAIN, 17, 12),
        s(shape, &["w: Int", "h: Int"], Some(0))
    );
    assert_eq!(
        client.signature(MAIN, 17, 18),
        s(shape, &["w: Int", "h: Int"], Some(1))
    );
    // 書きかけの呼び出しと，ラベルで決まる引数．
    client.open(
        MAIN,
        1,
        &NAV_MAIN.replace("Shape(w: 1, h: 2)", "Shape(h: 2, w:"),
    );
    assert_eq!(
        client.signature(MAIN, 17, 20),
        s(shape, &["w: Int", "h: Int"], Some(0))
    );
    // 呼び出しの外．
    client.change(MAIN, 2, NAV_MAIN);
    assert_eq!(client.signature(MAIN, 12, 2), None);
    assert_eq!(client.signature(MAIN, 17, 23), None);
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
    // 型と構成子をまとめて変える．import の名前も．
    assert_eq!(
        client.rename(MAIN, 17, 7, "Figure"),
        Ok(vec![
            edit(MAIN, (1, 15), (1, 20), "Figure"),
            edit(MAIN, (17, 6), (17, 11), "Figure"),
            edit(SHAPES, (0, 9), (0, 14), "Figure"),
            edit(SHAPES, (2, 15), (2, 20), "Figure"),
        ])
    );
    // effect の操作は宣言，handler の実装，呼び出し．
    assert_eq!(
        client.rename(MAIN, 4, 6, "log_info"),
        Ok(vec![
            edit(MAIN, (4, 5), (4, 9), "log_info"),
            edit(MAIN, (8, 5), (8, 9), "log_info"),
            edit(MAIN, (13, 6), (13, 10), "log_info"),
        ])
    );
    // 局所変数．
    assert_eq!(
        client.rename(MAIN, 11, 10, "who"),
        Ok(vec![
            edit(MAIN, (11, 9), (11, 13), "who"),
            edit(MAIN, (13, 22), (13, 26), "who"),
        ])
    );
    // 字句クラスの合わない名前，予約語．
    assert_eq!(
        client.rename(MAIN, 17, 7, "figure"),
        Err("`figure` is not a valid type name".to_owned())
    );
    assert_eq!(
        client.rename(MAIN, 11, 10, "match"),
        Err("`match` is not a valid lowercase name".to_owned())
    );
    // モジュールは変えられない．
    assert_eq!(
        client.rename(MAIN, 13, 12, "Logs"),
        Err("this name cannot be renamed".to_owned())
    );
    // prepareRename は名前の範囲．変えられなければ null．
    assert_eq!(
        client.prepare_rename(MAIN, 13, 7),
        Some(range((13, 6), (13, 10)))
    );
    assert_eq!(client.prepare_rename(MAIN, 13, 12), None);
    assert_eq!(client.prepare_rename(MAIN, 2, 0), None);
    client.shutdown();
}

/// 省略形の名前付き引数とフィールドのパターンはラベルを残す．同梱の core の名前は変えられない．
#[test]
fn rename_keeps_punned_labels() {
    let src = "error NotFound(id: Int)\n\ntype User(name: String)\n\nfn find(id: Int) -> Int fails NotFound {\n  fail NotFound(id:)\n}\n\nfn greet(u: User) -> String {\n  match u {\n    User(name:) -> name\n  }\n}\n\nfn main() {\n  List.map([1], fn(x) { x })\n}\n";
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

/// 省いた注釈に推論結果を挿入する（4.5）．
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
    // 戻り値の型と，推論した E を一緒に足す．
    assert_eq!(
        client.code_actions(MAIN, (7, 4), (7, 4)),
        action(
            "Add inferred `-> Int fails NotFound`",
            (7, 16),
            " -> Int fails NotFound"
        )
    );
    // 推論した R．
    assert_eq!(
        client.code_actions(MAIN, (11, 0), (11, 0)),
        action(
            "Add inferred `-> () use Logger`",
            (11, 10),
            " -> () use Logger"
        )
    );
    // 戻り値の型があれば use だけ．戻り値の型の後ろに足す．
    assert_eq!(
        client.code_actions(MAIN, (16, 5), (16, 5)),
        action("Add inferred `use Logger`", (16, 23), " use Logger")
    );
    // 修飾なしに書けないエラーは足さない．
    assert_eq!(client.code_actions(MAIN, (21, 4), (21, 4)), Some(vec![]));
    // 引数の関数の見えない集合は書かない（書いた節に暗に足される）．
    assert_eq!(
        client.code_actions(MAIN, (25, 4), (25, 4)),
        action("Add inferred `-> A`", (25, 25), " -> A")
    );
    // 本体の中では出さない．
    assert_eq!(client.code_actions(MAIN, (8, 4), (8, 4)), Some(vec![]));
    client.shutdown();
}
