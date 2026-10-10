//! 各段で共通の診断．字句解析，名前解決，型検査の診断をここへ写してから表示する．
//!
//! 元の診断の型はそれぞれのクレートに残し，ここでは変換だけを持つ．

use std::path::{Path, PathBuf};

use emela_resolve::{DiagnosticKind, NameError};
use emela_types::{Ty, TyCons, TypeError, TypeErrorKind};
use line_index::TextRange;

use crate::code;
use crate::source::{FileId, SourceDb};

/// 注記のうち助言を表すものの頭．
pub const HELP_PREFIX: &str = "help: ";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Error,
    Warning,
}

/// ファイルの中の範囲．
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

/// 診断の主な位置．
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    /// ファイルの中の範囲．
    Span(Span),
    /// ファイル全体（ファイル名の誤りなど）．
    File(FileId),
    /// ソースの表にないパス（ディレクトリ，読めなかったファイル）．
    Path(PathBuf),
}

/// 主な位置とは別の場所に付ける説明．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub span: Span,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    /// エラーコード（[`crate::code`]）．まだ振っていない段（字句解析）の診断は `None`．
    pub code: Option<&'static str>,
    pub message: String,
    /// 位置がない診断（node が見つからないなど）は `None`．
    pub location: Option<Location>,
    pub labels: Vec<Label>,
    /// 注記．[`HELP_PREFIX`] で始まるものは助言（`= help: …`）として出す．
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

    /// 直し方の助言．`= help: …` として出す．`Result` の誤りの側に置く型が大きくならないよう，
    /// 欄を足さずに注記に入れる．
    pub fn with_help(self, help: impl AsRef<str>) -> Self {
        self.with_note(format!("{HELP_PREFIX}{}", help.as_ref()))
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// 字句解析と構文解析の診断．コード（E01xx）と英語の文面は emela-syntax が持つ．
    pub fn from_syntax(file: FileId, diagnostic: &emela_syntax::Diagnostic) -> Self {
        Diagnostic::error(&diagnostic.message)
            .with_code(diagnostic.code.as_str())
            .with_span(Span::new(file, diagnostic.range))
    }

    /// 名前解決の診断．パスがソースの表にあれば ID で指し，なければパスのまま持つ．
    pub fn from_resolve(diagnostic: &emela_resolve::Diagnostic, sources: &SourceDb) -> Self {
        let location = match (sources.file_id(&diagnostic.file), diagnostic.range) {
            (Some(file), Some(range)) => Location::Span(Span::new(file, range)),
            (Some(file), None) => Location::File(file),
            (None, _) => Location::Path(diagnostic.file.clone()),
        };
        let display = |path: &Path| format!("`{}`", sources.display_path(path).display());
        let (code, message) = match &diagnostic.kind {
            DiagnosticKind::InvalidFileName { name, error } => (
                code::INVALID_FILE_NAME,
                format!(
                    "file name `{name}` cannot be a module name: {}",
                    name_error(error)
                ),
            ),
            DiagnosticKind::InvalidDirName { name, error } => (
                code::INVALID_DIR_NAME,
                format!(
                    "directory name `{name}` cannot be a module name: {}",
                    name_error(error)
                ),
            ),
            DiagnosticKind::ReservedModuleName { name } => (
                code::RESERVED_MODULE_NAME,
                format!("module name `{name}` is reserved"),
            ),
            // 文面のパスも，診断の位置と同じく起点からの相対パスにする．
            DiagnosticKind::DuplicateModule { name, first } => (
                code::DUPLICATE_MODULE,
                format!(
                    "duplicate module `{name}` (already defined by {})",
                    display(first)
                ),
            ),
            DiagnosticKind::UndefinedModule { name } => {
                (code::UNDEFINED_MODULE, format!("undefined module `{name}`"))
            }
            DiagnosticKind::ImportCycle { modules } => {
                let message = match modules.as_slice() {
                    [only] => format!("module `{only}` imports itself"),
                    _ => {
                        let mut cycle: Vec<String> =
                            modules.iter().map(ToString::to_string).collect();
                        cycle.push(modules[0].to_string());
                        format!("import cycle: {}", cycle.join(" → "))
                    }
                };
                (code::IMPORT_CYCLE, message)
            }
            DiagnosticKind::Io { message } => (
                code::CANNOT_READ,
                format!("cannot read directory: {message}"),
            ),
            _ => return Diagnostic::from_name_resolution(diagnostic, location),
        };
        Diagnostic::error(message).with_code(code).at(location)
    }

    /// 名前解決（宣言，import，式の中の名前）の診断．E0211〜E0220，W0201〜W0202．
    fn from_name_resolution(diagnostic: &emela_resolve::Diagnostic, location: Location) -> Self {
        let first_here = |first: &Option<TextRange>| match (&location, first) {
            (Location::Span(span), Some(range)) => Some(Span::new(span.file, *range)),
            _ => None,
        };
        let mut label = None;
        let mut help = None;
        let (severity, code, message) = match &diagnostic.kind {
            DiagnosticKind::UndefinedName {
                name,
                expected,
                suggestion,
            } => {
                help = suggestion.as_ref().map(|s| format!("did you mean `{s}`?"));
                (
                    Severity::Error,
                    code::UNDEFINED_NAME,
                    format!("cannot find {} `{name}` in this scope", expected.describe()),
                )
            }
            DiagnosticKind::UndefinedMember {
                name,
                owner,
                owner_kind,
                suggestion,
            } => {
                help = suggestion.as_ref().map(|s| format!("did you mean `{s}`?"));
                (
                    Severity::Error,
                    code::UNDEFINED_MEMBER,
                    format!("{owner_kind} `{owner}` has no `{name}`"),
                )
            }
            DiagnosticKind::Duplicate { name, first } => {
                label =
                    first_here(first).map(|span| (span, format!("`{name}` is first defined here")));
                (
                    Severity::Error,
                    code::DUPLICATE_DEFINITION,
                    format!("`{name}` is defined more than once"),
                )
            }
            DiagnosticKind::Private { name, module } => (
                Severity::Error,
                code::PRIVATE_ITEM,
                format!("`{name}` is private to module `{module}`"),
            ),
            DiagnosticKind::OpaqueConstruction { name, module } => {
                help = Some(format!(
                    "`{name}` is opaque: use the functions of module `{module}` to make one"
                ));
                (
                    Severity::Error,
                    code::OPAQUE_CONSTRUCTION,
                    format!("cannot construct opaque `{name}` outside module `{module}`"),
                )
            }
            DiagnosticKind::OpaquePattern { name, module } => (
                Severity::Error,
                code::OPAQUE_PATTERN,
                format!("cannot match on opaque `{name}` outside module `{module}`"),
            ),
            DiagnosticKind::WrongKind {
                name,
                expected,
                found,
            } => {
                if *found == "effect" && *expected == emela_resolve::Expected::ModuleOrTrait {
                    help = Some(format!(
                        "call the operations of an effect through its capability: `(use {name}).op(...)`"
                    ));
                } else if *found == "module" {
                    help = Some(format!(
                        "`{name}` is a module: qualify the name (`{name}.Name`) or list it in the import (`import ….{name}.{{Name}}`)"
                    ));
                }
                (
                    Severity::Error,
                    code::WRONG_KIND_OF_NAME,
                    format!("expected {}, found {found} `{name}`", expected.describe()),
                )
            }
            DiagnosticKind::Ambiguous { name } => (
                Severity::Error,
                code::AMBIGUOUS_NAME,
                format!("`{name}` refers to both a module and a trait"),
            ),
            DiagnosticKind::SelfOutside { self_type } => {
                let message = if *self_type {
                    "`Self` can only be used in traits and impls"
                } else {
                    "`self` can only be used in handlers, impls, and traits"
                };
                (Severity::Error, code::SELF_OUTSIDE, message.to_owned())
            }
            DiagnosticKind::EffectAsType { name } => {
                help = Some(format!(
                    "a capability cannot be passed around; take `use {name}` where it is needed, or pass a closure that captures it"
                ));
                (
                    Severity::Error,
                    code::EFFECT_AS_TYPE,
                    format!("effect `{name}` cannot be used as a type"),
                )
            }
            DiagnosticKind::InvalidUpdateTarget { name, found } => {
                help = Some(
                    "only a type, an error, or a handler with fields (one constructor) can be updated"
                        .to_owned(),
                );
                let message = match name {
                    Some(name) => format!("cannot update {found} `{name}` with `..`"),
                    None => format!("cannot update an {found} with `..`"),
                };
                (Severity::Error, code::INVALID_UPDATE_TARGET, message)
            }
            DiagnosticKind::PositionalInUpdate => (
                Severity::Error,
                code::POSITIONAL_IN_UPDATE,
                "fields to update must be named: `field: value`".to_owned(),
            ),
            DiagnosticKind::IntrinsicOutsideCore => {
                help = Some("use `@external` to bind a host function".to_owned());
                (
                    Severity::Error,
                    code::INTRINSIC_OUTSIDE_CORE,
                    "`@intrinsic` can only be used in the bundled core library".to_owned(),
                )
            }
            DiagnosticKind::MisplacedIntrinsic => (
                Severity::Error,
                code::MISPLACED_INTRINSIC,
                "`@intrinsic` can only be used on a function without a body".to_owned(),
            ),
            DiagnosticKind::DuplicateBinding { name, first } => {
                label =
                    first_here(first).map(|span| (span, format!("`{name}` is first bound here")));
                (
                    Severity::Error,
                    code::DUPLICATE_BINDING,
                    format!("`{name}` is bound more than once in the same pattern"),
                )
            }
            DiagnosticKind::ShadowedByTypeParam { name } => (
                Severity::Warning,
                code::SHADOWED_BY_TYPE_PARAM,
                format!("type parameter `{name}` shadows an outer definition"),
            ),
            DiagnosticKind::ShadowsPrelude { name } => (
                Severity::Warning,
                code::SHADOWS_PRELUDE,
                format!("`{name}` shadows the prelude's `{name}`"),
            ),
            _ => unreachable!("モジュールと import のグラフの診断は from_resolve で扱う"),
        };
        let mut out = Diagnostic::new(severity, message)
            .with_code(code)
            .at(location);
        if let Some((span, message)) = label {
            out = out.with_label(span, message);
        }
        if let Some(help) = help {
            out = out.with_help(help);
        }
        out
    }

    /// 型検査の診断．`TypeError` は位置を持たないので，呼び出し側が `span` を渡す．
    pub fn from_type_error(error: &TypeError, cons: &TyCons, span: Span) -> Self {
        let show = |ty: &Ty| format!("`{}`", ty.display(cons));
        let diagnostic = match &*error.kind {
            TypeErrorKind::Mismatch { expected, actual } => {
                let diagnostic = Diagnostic::error(format!(
                    "type mismatch: expected {}, found {}",
                    show(&error.expected),
                    show(&error.actual)
                ));
                if *expected == error.expected && *actual == error.actual {
                    diagnostic
                } else {
                    diagnostic.with_note(format!(
                        "the mismatch is between {} and {}",
                        show(expected),
                        show(actual)
                    ))
                }
            }
            TypeErrorKind::InfiniteType { var, ty } => Diagnostic::error(format!(
                "infinite type: {} occurs in {}",
                show(&Ty::Var(*var)),
                show(ty)
            ))
            .with_note(format!(
                "expected {}, found {}",
                show(&error.expected),
                show(&error.actual)
            )),
        };
        let code = match &*error.kind {
            TypeErrorKind::Mismatch { .. } => code::TYPE_MISMATCH,
            TypeErrorKind::InfiniteType { .. } => code::INFINITE_TYPE,
        };
        diagnostic.with_code(code).with_span(span)
    }
}

/// 名前の誤りの理由（emela-resolve の `NameError` の英語の文面）．
fn name_error(error: &NameError) -> String {
    match error {
        NameError::NotSnakeCase => "not lower snake_case".to_owned(),
        NameError::NotTypeName { converted } => {
            format!("the converted name `{converted}` is not a valid type name")
        }
    }
}

/// エラーの件数．
pub fn error_count(diagnostics: &[Diagnostic]) -> usize {
    diagnostics.iter().filter(|d| d.is_error()).count()
}
