//! 言語サーバーの本体．メッセージを受けて開いているファイルを更新し，診断を出し直す．

// `Uri` は内部に `Cell` を持つが，比較とハッシュは文字列（`as_str`）だけで決まるので，鍵にしてよい．
#![allow(clippy::mutable_key_type)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use emela_driver::{Diagnostic, FileSystem, Frontend, Input};
use lsp_server::{Connection, ErrorCode, Message, Notification, ProtocolError, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument,
    Notification as _, PublishDiagnostics,
};
use lsp_types::{self as lsp, Uri};

use crate::convert::{self, path_to_uri, uri_to_path};
use crate::overlay::Overlay;

/// プロジェクトの目印のファイル（仕様 4.1）．位置のない診断をプロジェクトに付けるときに使う．
const PROJECT_FILE: &str = "Pome.toml";

/// サーバーの能力．テキストは全文で同期する．
pub fn capabilities() -> lsp::ServerCapabilities {
    lsp::ServerCapabilities {
        text_document_sync: Some(lsp::TextDocumentSyncCapability::Options(
            lsp::TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(lsp::TextDocumentSyncKind::FULL),
                save: Some(lsp::TextDocumentSyncSaveOptions::SaveOptions(
                    lsp::SaveOptions {
                        include_text: Some(false),
                    },
                )),
                ..lsp::TextDocumentSyncOptions::default()
            },
        )),
        ..lsp::ServerCapabilities::default()
    }
}

/// initialize の握手をしてから，exit まで診断を出し続ける．
///
/// `frontend` は解析のたびに呼んで，新しいフロントエンドを作る（`emela check` と同じものを渡す）．
pub fn serve<Fs, F>(
    connection: &Connection,
    fs: Fs,
    frontend: impl FnMut() -> F,
) -> Result<(), Error>
where
    Fs: FileSystem,
    F: Frontend,
{
    let capabilities = serde_json::to_value(capabilities()).expect("能力は JSON にできる");
    let params = connection.initialize(capabilities)?;
    let params: lsp::InitializeParams = serde_json::from_value(params)?;
    let mut server = Server::new(connection, Overlay::new(fs), frontend);
    server.set_workspace(&params);
    Ok(server.run()?)
}

/// サーバーを止めた誤り（プロトコルの誤り，initialize の引数の誤り，入出力の誤り）．
pub type Error = Box<dyn std::error::Error + Send + Sync>;

/// 1つのメッセージを処理した結果．
enum Step {
    /// 開いているファイルが変わった．診断を出し直す．
    Changed,
    Unchanged,
    Exit,
}

struct Document {
    /// driver が使う絶対パス．
    path: PathBuf,
    version: i32,
}

/// ワークスペースのフォルダ．
struct Folder {
    /// エディタから来たパス．
    original: PathBuf,
    /// driver が使う絶対パス（シンボリックリンクを解いたもの）．
    path: PathBuf,
}

struct Server<'c, Fs, M> {
    connection: &'c Connection,
    fs: Overlay<Fs>,
    make_frontend: M,
    /// ワークスペースのフォルダ．`Pome.toml` があれば，ファイルを開かなくても解析する．
    workspace: Vec<Folder>,
    documents: HashMap<Uri, Document>,
    /// 前回に診断を送ったファイル．診断がなくなったら空の配列を送る．
    published: HashSet<Uri>,
}

impl<'c, Fs, M, F> Server<'c, Fs, M>
where
    Fs: FileSystem,
    M: FnMut() -> F,
    F: Frontend,
{
    fn new(connection: &'c Connection, fs: Overlay<Fs>, make_frontend: M) -> Self {
        Server {
            connection,
            fs,
            make_frontend,
            workspace: Vec::new(),
            documents: HashMap::new(),
            published: HashSet::new(),
        }
    }

    fn set_workspace(&mut self, params: &lsp::InitializeParams) {
        #[allow(deprecated)] // workspaceFolders を送らない古いクライアントのために rootUri も見る．
        let uris: Vec<&Uri> = match (&params.workspace_folders, &params.root_uri) {
            (Some(folders), _) if !folders.is_empty() => folders.iter().map(|f| &f.uri).collect(),
            (_, Some(root)) => vec![root],
            _ => Vec::new(),
        };
        self.workspace = uris
            .into_iter()
            .filter_map(uri_to_path)
            .map(|original| Folder {
                path: self.fs.normalize(&original),
                original,
            })
            .collect();
    }

    fn run(&mut self) -> Result<(), ProtocolError> {
        // ワークスペースのプロジェクトは，ファイルを開く前から診断を出す．
        let mut dirty = true;
        loop {
            // 溜まっているメッセージを全部読んでから解析する（続けて来た didChange は1回にまとめる）．
            match self.drain()? {
                Step::Exit => return Ok(()),
                Step::Changed => dirty = true,
                Step::Unchanged => {}
            }
            while dirty {
                let diagnostics = self.analyze();
                // 解析の間に書き換えが届いていたら，この結果は古いので捨てて解析し直す．
                match self.drain()? {
                    Step::Exit => return Ok(()),
                    Step::Changed => continue,
                    Step::Unchanged => {}
                }
                self.publish(diagnostics);
                dirty = false;
            }
            let Ok(message) = self.connection.receiver.recv() else {
                return Ok(());
            };
            match self.handle(message)? {
                Step::Exit => return Ok(()),
                Step::Changed => dirty = true,
                Step::Unchanged => {}
            }
        }
    }

    /// すでに届いているメッセージを待たずに全部処理する．
    fn drain(&mut self) -> Result<Step, ProtocolError> {
        let mut step = Step::Unchanged;
        while let Ok(message) = self.connection.receiver.try_recv() {
            match self.handle(message)? {
                Step::Exit => return Ok(Step::Exit),
                Step::Changed => step = Step::Changed,
                Step::Unchanged => {}
            }
        }
        Ok(step)
    }

    fn handle(&mut self, message: Message) -> Result<Step, ProtocolError> {
        match message {
            Message::Request(request) => self.request(request),
            Message::Notification(notification) => Ok(self.notification(notification)),
            Message::Response(_) => Ok(Step::Unchanged),
        }
    }

    fn request(&mut self, request: Request) -> Result<Step, ProtocolError> {
        if self.connection.handle_shutdown(&request)? {
            return Ok(Step::Exit);
        }
        let response = Response::new_err(
            request.id,
            ErrorCode::MethodNotFound as i32,
            format!("unsupported request `{}`", request.method),
        );
        self.send(response.into());
        Ok(Step::Unchanged)
    }

    fn notification(&mut self, notification: Notification) -> Step {
        match notification.method.as_str() {
            "exit" => Step::Exit,
            DidOpenTextDocument::METHOD => {
                let Some(params) = parse::<DidOpenTextDocument>(notification) else {
                    return Step::Unchanged;
                };
                let document = params.text_document;
                self.open(document.uri, document.version, document.text)
            }
            DidChangeTextDocument::METHOD => {
                let Some(params) = parse::<DidChangeTextDocument>(notification) else {
                    return Step::Unchanged;
                };
                // 全文の同期なので，最後の変更が新しい全文．
                let Some(change) = params.content_changes.into_iter().last() else {
                    return Step::Unchanged;
                };
                let document = params.text_document;
                self.open(document.uri, document.version, change.text)
            }
            DidSaveTextDocument::METHOD => {
                let Some(params) = parse::<DidSaveTextDocument>(notification) else {
                    return Step::Unchanged;
                };
                let uri = params.text_document.uri;
                match (params.text, self.documents.get(&uri)) {
                    (Some(text), Some(document)) => {
                        let version = document.version;
                        self.open(uri, version, text)
                    }
                    // 開いている内容はもう重ねてある．ディスクの他のファイルが変わったかもしれないので解析し直す．
                    _ => Step::Changed,
                }
            }
            DidCloseTextDocument::METHOD => {
                let Some(params) = parse::<DidCloseTextDocument>(notification) else {
                    return Step::Unchanged;
                };
                match self.documents.remove(&params.text_document.uri) {
                    Some(document) => {
                        self.fs.remove(&document.path);
                        Step::Changed
                    }
                    None => Step::Unchanged,
                }
            }
            _ => Step::Unchanged,
        }
    }

    /// 開いたか書き換えたファイルを重ねる．`file:` でない URI は扱わない．
    fn open(&mut self, uri: Uri, version: i32, text: String) -> Step {
        let Some(path) = uri_to_path(&uri) else {
            return Step::Unchanged;
        };
        let path = match self.documents.get(&uri) {
            Some(document) => document.path.clone(),
            None => self.fs.normalize(&path),
        };
        self.fs.set(path.clone(), text);
        self.documents.insert(uri, Document { path, version });
        Step::Changed
    }

    /// 開いているファイルのプロジェクトと，ワークスペースのプロジェクトを全部検査する．
    /// 結果はファイルのパスごとの診断．
    fn analyze(&mut self) -> BTreeMap<PathBuf, Vec<lsp::Diagnostic>> {
        let mut result: BTreeMap<PathBuf, Vec<lsp::Diagnostic>> = BTreeMap::new();
        let mut inputs: Vec<Input> = Vec::new();
        for folder in &self.workspace {
            // `Pome.toml` のないフォルダは，開いたファイルごとに単独ファイルとして扱う．
            if let Ok(input) = Input::resolve(&self.fs, &folder.path) {
                inputs.push(input);
            }
        }
        let mut paths: Vec<&PathBuf> = self.documents.values().map(|d| &d.path).collect();
        paths.sort();
        for path in paths {
            let input = match Input::resolve(&self.fs, path) {
                // プロジェクトは `emela check <プロジェクト>` と同じく，ディレクトリから検査する．
                Ok(input) if !input.single_file => Input::resolve(&self.fs, &input.base),
                other => other,
            };
            match input {
                Ok(input) => {
                    if !inputs.contains(&input) {
                        inputs.push(input);
                    }
                }
                Err(diagnostic) => {
                    // ソースのルートの外など．開いたファイルの先頭に付ける．
                    let lsp = self.convert(&diagnostic, &Default::default(), path).1;
                    result.entry(path.clone()).or_default().push(lsp);
                }
            }
        }

        for input in &inputs {
            let (analysis, _) = emela_driver::check(&self.fs, input, &mut (self.make_frontend)());
            // 位置のない診断は，エントリか，なければ `Pome.toml` に付ける．
            let anchor = input
                .entry
                .clone()
                .unwrap_or_else(|| input.base.join(PROJECT_FILE));
            for diagnostic in &analysis.diagnostics {
                let (path, lsp) = self.convert(diagnostic, &analysis.sources, &anchor);
                // 単独ファイルが同じディレクトリにいくつか開いていると，同じ診断が重なる．
                let entry = result.entry(path).or_default();
                if !entry.contains(&lsp) {
                    entry.push(lsp);
                }
            }
        }
        result
    }

    fn convert(
        &self,
        diagnostic: &Diagnostic,
        sources: &emela_driver::SourceDb,
        anchor: &Path,
    ) -> (PathBuf, lsp::Diagnostic) {
        convert::diagnostic(diagnostic, sources, anchor, &|path| self.uri_of(path))
    }

    /// パスの URI．開いているファイルならエディタから来た URI をそのまま使う．
    /// 開いていなければ，ワークスペースのフォルダの下はエディタから来たフォルダのパスでつなぐ
    /// （シンボリックリンクを解いたパスで送ると，エディタが別のファイルとみなす）．
    fn uri_of(&self, path: &Path) -> Option<Uri> {
        if let Some((uri, _)) = self.documents.iter().find(|(_, d)| d.path == path) {
            return Some(uri.clone());
        }
        let original = self.workspace.iter().find_map(|folder| {
            let rest = path.strip_prefix(&folder.path).ok()?;
            Some(folder.original.join(rest))
        });
        path_to_uri(original.as_deref().unwrap_or(path))
    }

    fn publish(&mut self, diagnostics: BTreeMap<PathBuf, Vec<lsp::Diagnostic>>) {
        let mut published = HashSet::new();
        for (path, diagnostics) in diagnostics {
            let Some(uri) = self.uri_of(&path) else {
                continue;
            };
            self.publish_one(uri.clone(), diagnostics);
            published.insert(uri);
        }
        // 前に送ったのに今回は診断のないファイルは，空の配列を送って消す．
        let mut cleared: Vec<Uri> = self.published.difference(&published).cloned().collect();
        cleared.sort();
        for uri in cleared {
            self.publish_one(uri, Vec::new());
        }
        self.published = published;
    }

    fn publish_one(&self, uri: Uri, diagnostics: Vec<lsp::Diagnostic>) {
        let version = self.documents.get(&uri).map(|d| d.version);
        let params = lsp::PublishDiagnosticsParams {
            uri,
            diagnostics,
            version,
        };
        self.send(Notification::new(PublishDiagnostics::METHOD.to_owned(), params).into());
    }

    fn send(&self, message: Message) {
        // 相手が切れていたら，次の recv で終わる．
        let _ = self.connection.sender.send(message);
    }
}

fn parse<N: lsp::notification::Notification>(notification: Notification) -> Option<N::Params> {
    notification.extract(N::METHOD).ok()
}
