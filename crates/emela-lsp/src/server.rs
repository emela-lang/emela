//! The server loop: handles messages, updates open files and republishes diagnostics.

// `Uri` holds a `Cell` internally, but equality and hashing depend only on the string (`as_str`),
// so it is safe as a key.
#![allow(clippy::mutable_key_type)]

mod navigation;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use emela_driver::{Analysis, Diagnostic, FileId, FileSystem, Frontend, Input, NavTarget};
use lsp_server::{Connection, ErrorCode, Message, Notification, ProtocolError, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument,
    Notification as _, PublishDiagnostics,
};
use lsp_types::request::{
    CodeActionRequest, Completion, DocumentHighlightRequest, DocumentSymbolRequest,
    FoldingRangeRequest, Formatting, GotoDefinition, GotoImplementation, GotoTypeDefinition,
    HoverRequest, InlayHintRequest, PrepareRenameRequest, References, Rename, Request as _,
    SelectionRangeRequest, SemanticTokensFullRequest, SemanticTokensRangeRequest,
    SignatureHelpRequest, WorkspaceSymbolRequest,
};
use lsp_types::{self as lsp, Uri};

use crate::convert::{self, path_to_uri, uri_to_path};
use crate::overlay::Overlay;

/// The project marker file (spec 4.1). Diagnostics without a location attach to it.
const PROJECT_FILE: &str = "Pome.toml";

/// Text is synced in full.
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
        document_highlight_provider: Some(lsp::OneOf::Left(true)),
        workspace_symbol_provider: Some(lsp::OneOf::Left(true)),
        folding_range_provider: Some(lsp::FoldingRangeProviderCapability::Simple(true)),
        selection_range_provider: Some(lsp::SelectionRangeProviderCapability::Simple(true)),
        type_definition_provider: Some(lsp::TypeDefinitionProviderCapability::Simple(true)),
        implementation_provider: Some(lsp::ImplementationProviderCapability::Simple(true)),
        document_symbol_provider: Some(lsp::OneOf::Left(true)),
        signature_help_provider: Some(lsp::SignatureHelpOptions {
            trigger_characters: Some(vec!["(".to_owned(), ",".to_owned()]),
            ..lsp::SignatureHelpOptions::default()
        }),
        rename_provider: Some(lsp::OneOf::Right(lsp::RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: lsp::WorkDoneProgressOptions::default(),
        })),
        inlay_hint_provider: Some(lsp::OneOf::Left(true)),
        semantic_tokens_provider: Some(
            lsp::SemanticTokensOptions {
                legend: convert::semantic_tokens_legend(),
                range: Some(true),
                full: Some(lsp::SemanticTokensFullOptions::Bool(true)),
                ..lsp::SemanticTokensOptions::default()
            }
            .into(),
        ),
        code_action_provider: Some(lsp::CodeActionProviderCapability::Options(
            lsp::CodeActionOptions {
                code_action_kinds: Some(vec![
                    lsp::CodeActionKind::REFACTOR_REWRITE,
                    lsp::CodeActionKind::QUICKFIX,
                ]),
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

/// Performs the initialize handshake, then keeps publishing diagnostics until `exit`.
///
/// `frontend` is called for every analysis to make a fresh frontend (pass the same one as
/// `emela check`).
pub fn serve<Fs, F>(
    connection: &Connection,
    fs: Fs,
    frontend: impl FnMut() -> F,
) -> Result<(), Error>
where
    Fs: FileSystem,
    F: Frontend,
{
    let capabilities =
        serde_json::to_value(capabilities()).expect("capabilities serialize to JSON");
    let params = connection.initialize(capabilities)?;
    let params: lsp::InitializeParams = serde_json::from_value(params)?;
    let mut server = Server::new(connection, Overlay::new(fs), frontend);
    server.set_workspace(&params);
    Ok(server.run()?)
}

/// Errors that stop the server: protocol errors, bad initialize params, I/O errors.
pub type Error = Box<dyn std::error::Error + Send + Sync>;

enum Step {
    /// An open file changed, so diagnostics must be republished.
    Changed,
    Unchanged,
    Exit,
}

struct Document {
    /// The absolute path the driver uses.
    path: PathBuf,
    version: i32,
}

struct Folder {
    /// The path as it came from the editor.
    original: PathBuf,
    /// The absolute path the driver uses (symlinks resolved).
    path: PathBuf,
}

struct Server<'c, Fs, M> {
    connection: &'c Connection,
    fs: Overlay<Fs>,
    make_frontend: M,
    /// Folders with a `Pome.toml` are analyzed even if no file is open.
    workspace: Vec<Folder>,
    documents: HashMap<Uri, Document>,
    /// Paths that diagnostics were last published for, with the URI used. When a file's
    /// diagnostics go away, an empty array is sent. Compared by path because the same file can
    /// come with differently encoded URIs (`%` escapes).
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
        #[allow(deprecated)] // Also read rootUri for old clients that do not send workspaceFolders.
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
        // Workspace projects get diagnostics before any file is opened.
        let mut dirty = true;
        loop {
            // Drain all pending messages before analyzing, so a burst of didChange is analyzed once.
            match self.drain()? {
                Step::Exit => return Ok(()),
                Step::Changed => dirty = true,
                Step::Unchanged => {}
            }
            while dirty {
                let diagnostics = self.analyze();
                // If an edit arrived during analysis, the result is stale: discard it and redo.
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

    /// Handles every message already received, without waiting.
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
            InlayHintRequest::METHOD => respond(request, |params| self.inlay_hints(&params)),
            SemanticTokensFullRequest::METHOD => {
                respond(request, |params| self.semantic_tokens_full(&params))
            }
            SemanticTokensRangeRequest::METHOD => {
                respond(request, |params| self.semantic_tokens_range(&params))
            }
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
            DocumentHighlightRequest::METHOD => {
                respond(request, |params| self.document_highlights(&params))
            }
            WorkspaceSymbolRequest::METHOD => {
                respond(request, |params| self.workspace_symbols(&params))
            }
            FoldingRangeRequest::METHOD => respond(request, |params| self.folding_ranges(&params)),
            SelectionRangeRequest::METHOD => {
                respond(request, |params| self.selection_ranges(&params))
            }
            GotoTypeDefinition::METHOD => respond(request, |params| self.type_definition(&params)),
            GotoImplementation::METHOD => respond(request, |params| self.implementations(&params)),
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

    /// Same as `emela fmt`. Uses the unsaved contents if the file is open, otherwise the file on
    /// disk. Returns an empty array if already formatted, or one edit replacing the whole text.
    /// Returns `null` if the file cannot be formatted (e.g. syntax errors) rather than an error
    /// response, since the diagnostics are already published. Width and indentation use the
    /// defaults, ignoring `options`.
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

    /// The type (and E) of the name or expression under the cursor. Rechecks the project
    /// containing the file on the spot. `null` for frontends without types, files that cannot be
    /// checked, and positions with nothing there.
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

    /// Shows the types, E and R left unwritten (spec 9.3). `null` for files that cannot be
    /// checked.
    fn inlay_hints(&mut self, params: &lsp::InlayHintParams) -> Option<Vec<lsp::InlayHint>> {
        let (analysis, program, file) = self.check_file(&params.text_document.uri)?;
        let index = analysis.sources[file].line_index();
        let start = convert::offset(index, params.range.start)?;
        // If the range ends past the file (beyond the last line), clamp to the end of the file.
        let end = convert::offset(index, params.range.end)
            .unwrap_or(index.len())
            .max(start);
        let range = line_index::TextRange::new(start, end);
        let hints = F::inlay_hints(&program, &analysis, file, range)
            .iter()
            .map(|hint| convert::inlay_hint(index, hint))
            .collect();
        Some(hints)
    }

    /// Marks capability values (spec 8.2) for the whole file. `null` for files that cannot be
    /// checked.
    fn semantic_tokens_full(
        &mut self,
        params: &lsp::SemanticTokensParams,
    ) -> Option<lsp::SemanticTokensResult> {
        self.semantic_tokens(&params.text_document.uri, None)
            .map(lsp::SemanticTokensResult::Tokens)
    }

    fn semantic_tokens_range(
        &mut self,
        params: &lsp::SemanticTokensRangeParams,
    ) -> Option<lsp::SemanticTokensRangeResult> {
        self.semantic_tokens(&params.text_document.uri, Some(params.range))
            .map(lsp::SemanticTokensRangeResult::Tokens)
    }

    fn semantic_tokens(
        &mut self,
        uri: &Uri,
        range: Option<lsp::Range>,
    ) -> Option<lsp::SemanticTokens> {
        let (analysis, program, file) = self.check_file(uri)?;
        let index = analysis.sources[file].line_index();
        let range = match range {
            None => line_index::TextRange::up_to(index.len()),
            Some(range) => {
                let start = convert::offset(index, range.start)?;
                let end = convert::offset(index, range.end)
                    .unwrap_or(index.len())
                    .max(start);
                line_index::TextRange::new(start, end)
            }
        };
        let tokens = F::semantic_tokens(&program, &analysis, file, range);
        Some(convert::semantic_tokens(index, &tokens))
    }

    /// Refactorings and diagnostic-linked quick fixes. `null` for files that cannot be checked.
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
            .filter(|action| {
                let kind = convert::code_action_kind(action.kind);
                params.context.only.as_ref().is_none_or(|only| {
                    only.iter().any(|requested| {
                        requested.as_str().is_empty()
                            || requested == &kind
                            || kind
                                .as_str()
                                .starts_with(&format!("{}.", requested.as_str()))
                    })
                })
            })
            .map(|action| {
                let diagnostics = action
                    .diagnostics
                    .iter()
                    .map(|d| {
                        convert::diagnostic(
                            d,
                            &analysis.sources,
                            analysis.sources[file].path(),
                            &|p| self.uri_of(p),
                        )
                        .1
                    })
                    .collect::<Vec<_>>();
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
                    kind: Some(convert::code_action_kind(action.kind)),
                    diagnostics: (!diagnostics.is_empty()).then_some(diagnostics),
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

    /// Rechecks on the spot, like hover. Filtering by the partially typed name is left to the
    /// editor. `null` for files that cannot be checked.
    fn complete(&mut self, params: &lsp::CompletionParams) -> Option<lsp::CompletionResponse> {
        let at = self.check_at(&params.text_document_position)?;
        let items = F::complete(&at.program, &at.analysis, at.file, at.offset)
            .iter()
            .enumerate()
            .map(|(i, item)| convert::completion_item(i, item))
            .collect();
        Some(lsp::CompletionResponse::Array(items))
    }

    /// The signature of the call whose parentheses contain the cursor. `null` outside a call or
    /// when the callee's type is unknown.
    fn signature_help(&mut self, params: &lsp::SignatureHelpParams) -> Option<lsp::SignatureHelp> {
        let at = self.check_at(&params.text_document_position_params)?;
        let info = F::signature_help(&at.program, &at.analysis, at.file, at.offset)?;
        Some(convert::signature_help(&info))
    }

    /// The range of the name if it can be renamed, otherwise `null`.
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

    /// Names that cannot be renamed, and new names of the wrong lexical class, are error
    /// responses.
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

    /// Rechecks on the spot, like hover. `null` for definitions in the bundled core or without
    /// source, and for positions without a name.
    fn definition(
        &mut self,
        params: &lsp::GotoDefinitionParams,
    ) -> Option<lsp::GotoDefinitionResponse> {
        let at = self.check_at(&params.text_document_position_params)?;
        let target = F::definition(&at.program, &at.analysis, at.file, at.offset)?;
        let location = self.location(&at.analysis, target)?;
        Some(lsp::GotoDefinitionResponse::Scalar(location))
    }

    /// Only within the checked project (for a single file, the files reachable from the entry).
    /// `null` for positions without a name.
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

    /// Built from the syntax tree alone, without checking. Uses the unsaved contents if the file
    /// is open.
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

    /// Checks the project containing the file (or just the file if it is standalone).
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

    /// Checks the project containing the file (or just the file if it is standalone).
    fn check_file(&mut self, uri: &Uri) -> Option<(Analysis, F::Program, FileId)> {
        let path = self.path_of(uri)?;
        let input = match Input::resolve(&self.fs, &path).ok()? {
            input if !input.single_file => Input::resolve(&self.fs, &input.base).ok()?,
            input => input,
        };
        let (input, _) = with_dependencies(&self.fs, input);
        let (analysis, program) =
            emela_driver::check(&self.fs, &input, &mut (self.make_frontend)());
        let program = program?;
        let file = analysis.sources.file_id(&path)?;
        Some((analysis, program, file))
    }

    /// The absolute path the driver uses.
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
                // Full sync, so the last change is the whole new text.
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
                    // Open contents are already overlaid, but other files on disk may have
                    // changed, so reanalyze.
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

    /// URIs other than `file:` are ignored.
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

    /// Checks the projects of all open files and all workspace projects. Returns diagnostics
    /// per file path.
    fn analyze(&mut self) -> BTreeMap<PathBuf, Vec<lsp::Diagnostic>> {
        let mut result: BTreeMap<PathBuf, Vec<lsp::Diagnostic>> = BTreeMap::new();
        let mut inputs: Vec<Input> = Vec::new();
        for folder in &self.workspace {
            // A folder without `Pome.toml` is handled per open file, as standalone files.
            if let Ok(input) = Input::resolve(&self.fs, &folder.path) {
                inputs.push(input);
            }
        }
        let mut paths: Vec<&PathBuf> = self.documents.values().map(|d| &d.path).collect();
        paths.sort();
        for path in paths {
            let input = match Input::resolve(&self.fs, path) {
                // Check a project from its directory, like `emela check <project>`.
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
                    // E.g. outside the source root. Attach to the start of the open file.
                    let lsp = self.convert(&diagnostic, &Default::default(), path).1;
                    result.entry(path.clone()).or_default().push(lsp);
                }
            }
        }

        for input in inputs {
            let (input, problem) = with_dependencies(&self.fs, input);
            let input = &input;
            let (mut analysis, _) =
                emela_driver::check(&self.fs, input, &mut (self.make_frontend)());
            analysis.diagnostics.extend(problem);
            // Diagnostics without a location attach to the entry, or to `Pome.toml` if none.
            let anchor = input
                .entry
                .clone()
                .unwrap_or_else(|| input.base.join(PROJECT_FILE));
            for diagnostic in &analysis.diagnostics {
                let (path, lsp) = self.convert(diagnostic, &analysis.sources, &anchor);
                // Several standalone files open in the same directory would duplicate the
                // diagnostic.
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

    /// Uses the editor's URI for open files. Otherwise, a path under a workspace folder is joined
    /// onto the folder path as the editor sent it: sending a symlink-resolved path would make the
    /// editor treat it as a different file.
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
        // Send an empty array to clear files that had diagnostics before but have none now, and
        // the old URI of files whose URI spelling changed. Clearing goes first so that new
        // diagnostics survive even if the editor treats both spellings as the same file.
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
        // If the peer is gone, the next recv ends the loop.
        let _ = self.connection.sender.send(message);
    }
}

struct CheckedAt<P> {
    analysis: Analysis,
    program: P,
    file: FileId,
    offset: line_index::TextSize,
}

/// Reads the request's params and builds the response; InvalidParams if they cannot be read.
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

/// Adds the project's dependencies from the cache. The language server does not go to the
/// network (spec 4.7): a dependency missing from the cache, or a missing lock, is a diagnostic on
/// `Pome.toml`, and the project is checked without dependencies. The test root and
/// `[dev-dependencies]` are read too, like `emela check`.
fn with_dependencies(fs: &dyn FileSystem, input: Input) -> (Input, Option<Diagnostic>) {
    if input.single_file {
        return (input, None);
    }
    let mut input = input.with_tests(fs);
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    let result = emela_pome::fetch::config::Config::from_env(|k| std::env::var(k).ok(), home)
        .map_err(emela_pome::project::PomeError::Usage)
        .and_then(|mut config| {
            config.sources.clear();
            let fetcher =
                emela_pome::fetch::Fetcher::new(config, emela_pome::fetch::transport::Http::new());
            emela_driver::project_dependencies(fs, &input.base, &fetcher, true)
        });
    match result {
        Ok(dependencies) => {
            input.dependencies = dependencies;
            (input, None)
        }
        Err(e) => {
            let diagnostic = Diagnostic::error(format!("the dependencies cannot be used: {e}"))
                .with_code(emela_driver::code::DEPENDENCIES_NOT_READY)
                .with_note("run `emela pome install` to get them")
                .at(emela_driver::Location::Path(input.base.join(PROJECT_FILE)));
            (input, Some(diagnostic))
        }
    }
}
