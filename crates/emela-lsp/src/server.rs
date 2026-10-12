//! 言語サーバーの本体．メッセージを受けて開いているファイルを更新し，診断を出し直す．

// `Uri` は内部に `Cell` を持つが，比較とハッシュは文字列（`as_str`）だけで決まるので，鍵にしてよい．
#![allow(clippy::mutable_key_type)]

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use emela_driver::{Analysis, Diagnostic, FileId, FileSystem, Frontend, Input, NavTarget};
use lsp_server::{Connection, ErrorCode, Message, Notification, ProtocolError, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument,
    Notification as _, PublishDiagnostics,
};
use lsp_types::request::{
    CodeActionRequest, Completion, DocumentSymbolRequest, Formatting, GotoDefinition, HoverRequest,
    PrepareRenameRequest, References, Rename, Request as _, SignatureHelpRequest,
};
use lsp_types::{self as lsp, Uri};

use crate::convert::{self, path_to_uri, uri_to_path};
use crate::overlay::Overlay;

/// プロジェクトの目印のファイル（仕様 4.1）．位置のない診断をプロジェクトに付けるときに使う．
const PROJECT_FILE: &str = "Pome.toml";

/// サーバーの能力．テキストは全文で同期する．文書全体の整形，ホバー，定義へ移動，参照の検索，
/// 文書のシンボルを受ける．
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
        document_formatting_provider: Some(lsp::OneOf::Left(true)),
        hover_provider: Some(lsp::HoverProviderCapability::Simple(true)),
        definition_provider: Some(lsp::OneOf::Left(true)),
        references_provider: Some(lsp::OneOf::Left(true)),
        document_symbol_provider: Some(lsp::OneOf::Left(true)),
        signature_help_provider: Some(lsp::SignatureHelpOptions {
            trigger_characters: Some(vec!["(".to_owned(), ",".to_owned()]),
            ..lsp::SignatureHelpOptions::default()
        }),
        rename_provider: Some(lsp::OneOf::Right(lsp::RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: lsp::WorkDoneProgressOptions::default(),
        })),
        code_action_provider: Some(lsp::CodeActionProviderCapability::Options(
            lsp::CodeActionOptions {
                code_action_kinds: Some(vec![lsp::CodeActionKind::REFACTOR_REWRITE]),
                ..lsp::CodeActionOptions::default()
            },
        )),
        completion_provider: Some(lsp::CompletionOptions {
            trigger_characters: Some(vec![".".to_owned()]),
            ..lsp::CompletionOptions::default()
        }),
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
    /// 前回に診断を送ったファイルのパスと，送った URI．診断がなくなったら空の配列を送る．
    /// 同じファイルでも URI の書き方（`%` の符号化）が変わることがあるので，パスで比べる．
    published: HashMap<PathBuf, Uri>,
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
            published: HashMap::new(),
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
        let response = match request.method.as_str() {
            Formatting::METHOD => respond(request, |params| self.format(&params)),
            HoverRequest::METHOD => respond(request, |params| self.hover(&params)),
            CodeActionRequest::METHOD => respond(request, |params| self.code_actions(&params)),
            Completion::METHOD => respond(request, |params| self.complete(&params)),
            SignatureHelpRequest::METHOD => respond(request, |params| self.signature_help(&params)),
            PrepareRenameRequest::METHOD => respond(request, |params| self.prepare_rename(&params)),
            Rename::METHOD => match serde_json::from_value(request.params) {
                Ok(params) => match self.rename(&params) {
                    Ok(edit) => Response::new_ok(request.id, edit),
                    Err(message) => {
                        Response::new_err(request.id, ErrorCode::RequestFailed as i32, message)
                    }
                },
                Err(err) => Response::new_err(
                    request.id,
                    ErrorCode::InvalidParams as i32,
                    format!("invalid params: {err}"),
                ),
            },
            GotoDefinition::METHOD => respond(request, |params| self.definition(&params)),
            References::METHOD => respond(request, |params| self.references(&params)),
            DocumentSymbolRequest::METHOD => {
                respond(request, |params| self.document_symbols(&params))
            }
            _ => Response::new_err(
                request.id,
                ErrorCode::MethodNotFound as i32,
                format!("unsupported request `{}`", request.method),
            ),
        };
        self.send(response.into());
        Ok(Step::Unchanged)
    }

    /// 文書全体を整形する（`emela fmt` と同じ）．開いていれば未保存の内容を，開いていなければディスクの内容を使う．
    /// 整っていれば空の配列，変わるなら全体を置き換える編集1つ．構文エラーなどで整形できなければ `null`
    /// （誤りの応答にはしない．診断は publishDiagnostics で出ている）．幅と字下げは `options` を見ずに既定のもの．
    fn format(&self, params: &lsp::DocumentFormattingParams) -> Option<Vec<lsp::TextEdit>> {
        let uri = &params.text_document.uri;
        let path = match self.documents.get(uri) {
            Some(document) => document.path.clone(),
            None => self.fs.normalize(&uri_to_path(uri)?),
        };
        let text = self.fs.read_to_string(&path).ok()?;
        let formatted = emela_driver::format_source(&path, &text).ok()?;
        Some(match formatted {
            None => Vec::new(),
            Some(formatted) => vec![lsp::TextEdit {
                range: convert::whole_range(&text),
                new_text: formatted,
            }],
        })
    }

    /// カーソルの下の名前や式の型（と E）．ファイルを含むプロジェクトをその場で検査し直す．
    /// 型を持たないフロントエンド，検査できないファイル，何もない位置では `null`．
    fn hover(&mut self, params: &lsp::HoverParams) -> Option<lsp::Hover> {
        let at = self.check_at(&params.text_document_position_params)?;
        let index = at.analysis.sources[at.file].line_index();
        let info = F::hover(&at.program, &at.analysis, at.file, at.offset)?;
        Some(lsp::Hover {
            contents: lsp::HoverContents::Markup(lsp::MarkupContent {
                kind: lsp::MarkupKind::Markdown,
                value: info.contents,
            }),
            range: Some(convert::range(index, info.range)),
        })
    }

    /// 範囲に掛かるコードアクション（省いた注釈に推論結果を挿入する，4.5）．検査できないファイルでは `null`．
    fn code_actions(
        &mut self,
        params: &lsp::CodeActionParams,
    ) -> Option<Vec<lsp::CodeActionOrCommand>> {
        let uri = &params.text_document.uri;
        let (analysis, program, file) = self.check_file(uri)?;
        let index = analysis.sources[file].line_index();
        let start = convert::offset(index, params.range.start)?;
        let end = convert::offset(index, params.range.end)?.max(start);
        let range = line_index::TextRange::new(start, end);
        let actions = F::code_actions(&program, &analysis, file, range)
            .into_iter()
            .map(|action| {
                let edits = action
                    .edits
                    .iter()
                    .map(|(range, text)| lsp::TextEdit {
                        range: convert::range(index, *range),
                        new_text: text.clone(),
                    })
                    .collect();
                lsp::CodeActionOrCommand::CodeAction(lsp::CodeAction {
                    title: action.title,
                    kind: Some(lsp::CodeActionKind::REFACTOR_REWRITE),
                    edit: Some(lsp::WorkspaceEdit {
                        changes: Some(HashMap::from([(uri.clone(), edits)])),
                        ..lsp::WorkspaceEdit::default()
                    }),
                    ..lsp::CodeAction::default()
                })
            })
            .collect();
        Some(actions)
    }

    /// カーソルの位置の補完の候補．ホバーと同じくその場で検査し直す．書きかけの名前での絞り込みは
    /// エディタに任せる．検査できないファイルでは `null`．
    fn complete(&mut self, params: &lsp::CompletionParams) -> Option<lsp::CompletionResponse> {
        let at = self.check_at(&params.text_document_position)?;
        let items = F::complete(&at.program, &at.analysis, at.file, at.offset)
            .iter()
            .enumerate()
            .map(|(i, item)| convert::completion_item(i, item))
            .collect();
        Some(lsp::CompletionResponse::Array(items))
    }

    /// カーソルを括弧の中に含む呼び出しのシグネチャ．呼び出しの外や，呼ぶ先の型が分からなければ `null`．
    fn signature_help(&mut self, params: &lsp::SignatureHelpParams) -> Option<lsp::SignatureHelp> {
        let at = self.check_at(&params.text_document_position_params)?;
        let info = F::signature_help(&at.program, &at.analysis, at.file, at.offset)?;
        Some(convert::signature_help(&info))
    }

    /// 名前を変えられる位置なら，その名前の範囲．変えられなければ `null`．
    fn prepare_rename(
        &mut self,
        params: &lsp::TextDocumentPositionParams,
    ) -> Option<lsp::PrepareRenameResponse> {
        let at = self.check_at(params)?;
        let range = F::prepare_rename(&at.program, &at.analysis, at.file, at.offset)?;
        let index = at.analysis.sources[at.file].line_index();
        Some(lsp::PrepareRenameResponse::Range(convert::range(
            index, range,
        )))
    }

    /// 名前を変える編集．変えられない名前や字句クラスの合わない新しい名前は誤りの応答にする．
    fn rename(&mut self, params: &lsp::RenameParams) -> Result<lsp::WorkspaceEdit, String> {
        let not_renamable = || emela_driver::RenameError::NotRenamable.to_string();
        let at = self
            .check_at(&params.text_document_position)
            .ok_or_else(not_renamable)?;
        let edits = F::rename(
            &at.program,
            &at.analysis,
            at.file,
            at.offset,
            &params.new_name,
        )
        .map_err(|err| err.to_string())?;
        let mut changes: HashMap<Uri, Vec<lsp::TextEdit>> = HashMap::new();
        for (target, new_text) in edits {
            let Some(location) = self.location(&at.analysis, target) else {
                continue;
            };
            changes
                .entry(location.uri)
                .or_default()
                .push(lsp::TextEdit {
                    range: location.range,
                    new_text,
                });
        }
        Ok(lsp::WorkspaceEdit {
            changes: Some(changes),
            ..lsp::WorkspaceEdit::default()
        })
    }

    /// カーソルの下の名前の定義．ホバーと同じくその場で検査し直す．
    /// 同梱の core やソースのない定義，名前のない位置では `null`．
    fn definition(
        &mut self,
        params: &lsp::GotoDefinitionParams,
    ) -> Option<lsp::GotoDefinitionResponse> {
        let at = self.check_at(&params.text_document_position_params)?;
        let target = F::definition(&at.program, &at.analysis, at.file, at.offset)?;
        let location = self.location(&at.analysis, target)?;
        Some(lsp::GotoDefinitionResponse::Scalar(location))
    }

    /// カーソルの下の名前の参照．検査したプロジェクト（単独ファイルならエントリからたどれるファイル）の中だけ．
    /// 名前のない位置では `null`．
    fn references(&mut self, params: &lsp::ReferenceParams) -> Option<Vec<lsp::Location>> {
        let at = self.check_at(&params.text_document_position)?;
        let declaration = params.context.include_declaration;
        let targets = F::references(&at.program, &at.analysis, at.file, at.offset, declaration);
        if targets.is_empty() {
            return None;
        }
        Some(
            targets
                .into_iter()
                .filter_map(|target| self.location(&at.analysis, target))
                .collect(),
        )
    }

    /// ファイルの中の宣言．構文木だけから作るので検査はしない．開いていれば未保存の内容を使う．
    fn document_symbols(
        &self,
        params: &lsp::DocumentSymbolParams,
    ) -> Option<lsp::DocumentSymbolResponse> {
        let uri = &params.text_document.uri;
        let path = self.path_of(uri)?;
        let text = self.fs.read_to_string(&path).ok()?;
        let index = line_index::LineIndex::new(&text);
        let symbols = emela_driver::document_symbols(&text)
            .iter()
            .map(|symbol| convert::document_symbol(&index, symbol))
            .collect();
        Some(lsp::DocumentSymbolResponse::Nested(symbols))
    }

    /// 位置のあるファイルを含むプロジェクト（単独ファイルならそのファイル）を検査する．
    fn check_at(
        &mut self,
        position: &lsp::TextDocumentPositionParams,
    ) -> Option<CheckedAt<F::Program>> {
        let (analysis, program, file) = self.check_file(&position.text_document.uri)?;
        let offset = convert::offset(analysis.sources[file].line_index(), position.position)?;
        Some(CheckedAt {
            analysis,
            program,
            file,
            offset,
        })
    }

    /// ファイルを含むプロジェクト（単独ファイルならそのファイル）を検査する．
    fn check_file(&mut self, uri: &Uri) -> Option<(Analysis, F::Program, FileId)> {
        let path = self.path_of(uri)?;
        let input = match Input::resolve(&self.fs, &path).ok()? {
            input if !input.single_file => Input::resolve(&self.fs, &input.base).ok()?,
            input => input,
        };
        let (analysis, program) =
            emela_driver::check(&self.fs, &input, &mut (self.make_frontend)());
        let program = program?;
        let file = analysis.sources.file_id(&path)?;
        Some((analysis, program, file))
    }

    /// URI のファイルの，driver が使う絶対パス．
    fn path_of(&self, uri: &Uri) -> Option<PathBuf> {
        match self.documents.get(uri) {
            Some(document) => Some(document.path.clone()),
            None => Some(self.fs.normalize(&uri_to_path(uri)?)),
        }
    }

    fn location(&self, analysis: &Analysis, target: NavTarget) -> Option<lsp::Location> {
        let source = &analysis.sources[target.file];
        Some(lsp::Location {
            uri: self.uri_of(source.path())?,
            range: convert::range(source.line_index(), target.range),
        })
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
        let mut published = HashMap::new();
        for path in diagnostics.keys() {
            if let Some(uri) = self.uri_of(path) {
                published.insert(path.clone(), uri);
            }
        }
        // 前に送ったのに今回は診断のないファイルと，URI の書き方が変わったファイルの古い URI には，
        // 空の配列を送って消す．エディタが2つの書き方を同じファイルとみなしても新しい診断が残るように，
        // 消す方を先に送る．
        let mut cleared: Vec<Uri> = self
            .published
            .iter()
            .filter(|(path, uri)| published.get(*path) != Some(*uri))
            .map(|(_, uri)| uri.clone())
            .collect();
        cleared.sort();
        for uri in cleared {
            self.publish_one(uri, Vec::new());
        }
        for (path, diagnostics) in diagnostics {
            if let Some(uri) = published.get(&path) {
                self.publish_one(uri.clone(), diagnostics);
            }
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

/// 位置の要求のために検査した結果．
struct CheckedAt<P> {
    analysis: Analysis,
    program: P,
    file: FileId,
    offset: line_index::TextSize,
}

/// 要求の引数を読んで応答を作る．読めなければ InvalidParams の誤り．
fn respond<P, R>(request: Request, f: impl FnOnce(P) -> R) -> Response
where
    P: serde::de::DeserializeOwned,
    R: serde::Serialize,
{
    match serde_json::from_value(request.params) {
        Ok(params) => Response::new_ok(request.id, f(params)),
        Err(err) => Response::new_err(
            request.id,
            ErrorCode::InvalidParams as i32,
            format!("invalid params: {err}"),
        ),
    }
}

fn parse<N: lsp::notification::Notification>(notification: Notification) -> Option<N::Params> {
    notification.extract(N::METHOD).ok()
}
