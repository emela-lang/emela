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
use lsp_types::request::{Formatting, HoverRequest, Initialize, Request as _, Shutdown};
use lsp_types::{self as lsp, Uri};
use serde_json::json;

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
    let id = client.request("textDocument/completion", json!({}));
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
