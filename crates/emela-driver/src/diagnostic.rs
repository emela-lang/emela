//! 各段で共通の診断．字句解析，名前解決，型検査の診断をここへ写してから表示する．
//!
//! 元の診断の型はそれぞれのクレートに残し，ここでは変換だけを持つ．

use std::path::{Path, PathBuf};

use emela_resolve::ModuleId;
use emela_resolve::{DiagnosticKind, NameError};
use emela_types::{
    DiagnosticKind as TypeDiagnosticKind, EffectContext, Ty, TyCons, TypeError, TypeErrorKind,
};
use la_arena::ArenaMap;
use line_index::TextRange;
use smol_str::SmolStr;

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
        Diagnostic::type_error(error, cons).with_span(span)
    }

    /// 型の不一致の診断（位置なし）．
    fn type_error(error: &TypeError, cons: &TyCons) -> Self {
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
        diagnostic.with_code(code)
    }
}

impl Diagnostic {
    /// lowering の誤り．型検査は通ったが Core IR にまだできない箇所．文面は型検査の
    /// `Unsupported` と同じ．位置の引き方は [`Diagnostic::from_type`] と同じ．
    pub fn from_lower(error: &emela_core::LowerError, files: &ArenaMap<ModuleId, FileId>) -> Self {
        let (diagnostic, span) = match error {
            emela_core::LowerError::Unsupported { what, span } => (
                Diagnostic::error(format!("not supported yet: {what}")),
                span,
            ),
            emela_core::LowerError::MissingExternal { name, target, span } => (
                Diagnostic::error(format!(
                    "external function `{name}` has no `@external` for target `{target}`"
                ))
                .with_code(code::MISSING_EXTERNAL_TARGET)
                .with_help(format!(
                    "add `@external({target}, ...)`, or avoid calling it from the entry"
                )),
                span,
            ),
            emela_core::LowerError::MissingCoreFile { path, span } => (
                Diagnostic::error(format!(
                    "internal error: the bundled core has no file `{path}`"
                )),
                span,
            ),
        };
        match span.and_then(|s| files.get(s.module).map(|&file| Span::new(file, s.range))) {
            Some(span) => diagnostic.with_span(span),
            None => diagnostic,
        }
    }

    /// 型検査の診断．位置は HIR の位置（モジュールと範囲）で，モジュールのファイルを
    /// `files` で引く．ソースのない位置（Prelude の宣言）は位置なしにする．
    pub fn from_type(
        diagnostic: &emela_types::Diagnostic,
        cons: &TyCons,
        files: &ArenaMap<ModuleId, FileId>,
    ) -> Self {
        let span = diagnostic
            .span
            .and_then(|s| files.get(s.module).map(|&file| Span::new(file, s.range)));
        let show = |ty: &Ty| format!("`{}`", ty.display(cons));
        let mut help = None;
        let (severity, code, message) = match &diagnostic.kind {
            TypeDiagnosticKind::Mismatch(error) => {
                let out = Diagnostic::type_error(error, cons);
                return match span {
                    Some(span) => out.with_span(span),
                    None => out,
                };
            }
            TypeDiagnosticKind::WrongTypeArgCount {
                name,
                expected,
                found,
            } => (
                Severity::Error,
                Some(code::WRONG_TYPE_ARG_COUNT),
                format!("wrong number of type arguments: `{name}` takes {expected}, found {found}"),
            ),
            TypeDiagnosticKind::Unsupported { what } => {
                (Severity::Error, None, format!("not supported yet: {what}"))
            }
            TypeDiagnosticKind::WrongArgCount {
                callee,
                expected,
                found,
            } => (
                Severity::Error,
                Some(code::WRONG_ARG_COUNT),
                match callee {
                    Some(callee) => format!(
                        "wrong number of arguments: `{callee}` takes {expected}, found {found}"
                    ),
                    None => format!(
                        "wrong number of arguments: this function takes {expected}, found {found}"
                    ),
                },
            ),
            TypeDiagnosticKind::UnknownLabel { callee, label } => (
                Severity::Error,
                Some(code::UNKNOWN_LABEL),
                format!("no parameter named `{label}` in `{callee}`"),
            ),
            TypeDiagnosticKind::LabelOnUnnamedCallee { label } => (
                Severity::Error,
                Some(code::UNKNOWN_LABEL),
                format!("no parameter named `{label}`: a function value has no parameter names"),
            ),
            TypeDiagnosticKind::DuplicateArg { label } => (
                Severity::Error,
                Some(code::DUPLICATE_ARG),
                format!("argument `{label}` given more than once"),
            ),
            TypeDiagnosticKind::MissingPatternFields { ctor, fields } => {
                help = Some("add `..` to ignore the remaining fields".to_owned());
                (
                    Severity::Error,
                    Some(code::MISSING_PATTERN_FIELDS),
                    format!(
                        "pattern `{ctor}` does not mention {}",
                        quoted_list("field", "fields", fields)
                    ),
                )
            }
            TypeDiagnosticKind::PatternFieldCount {
                ctor,
                expected,
                found,
            } => (
                Severity::Error,
                Some(code::PATTERN_FIELD_COUNT),
                format!("pattern `{ctor}` has {expected} fields, found {found}"),
            ),
            TypeDiagnosticKind::NotNumeric { found, integer } => (
                Severity::Error,
                Some(code::NOT_NUMERIC),
                if *integer {
                    format!(
                        "expected an integer (`Int` or `Int64`), found {}",
                        show(found)
                    )
                } else {
                    format!(
                        "expected a number (`Int`, `Int64` or `Float`), found {}",
                        show(found)
                    )
                },
            ),
            TypeDiagnosticKind::IntLiteralOutOfRange { literal, ty } => (
                Severity::Error,
                Some(code::INT_LITERAL_OUT_OF_RANGE),
                format!(
                    "integer literal `{literal}` is out of range for {}",
                    show(ty)
                ),
            ),
            TypeDiagnosticKind::NoField { ty, field } => (
                Severity::Error,
                Some(code::NO_FIELD),
                format!("no field `{field}` on type {}", show(ty)),
            ),
            TypeDiagnosticKind::NoFields { ty, .. } => (
                Severity::Error,
                Some(code::NO_FIELDS),
                format!("type {} has no fields", show(ty)),
            ),
            TypeDiagnosticKind::FieldOnUnknownType { field } => {
                help = Some("add a type annotation".to_owned());
                (
                    Severity::Error,
                    Some(code::FIELD_ON_UNKNOWN_TYPE),
                    format!("the type of this value must be known to access field `{field}`"),
                )
            }
            TypeDiagnosticKind::OpaqueField { ty, field } => (
                Severity::Error,
                Some(code::OPAQUE_FIELD),
                format!(
                    "cannot access field `{field}` of opaque type {} outside its module",
                    show(ty)
                ),
            ),
            TypeDiagnosticKind::MissingParamType { param } => (
                Severity::Error,
                Some(code::MISSING_PARAM_TYPE),
                format!("parameter `{param}` of a top-level function needs a type"),
            ),
            TypeDiagnosticKind::NonExhaustiveMatch { missing } => (
                Severity::Error,
                Some(code::NON_EXHAUSTIVE_MATCH),
                format!(
                    "non-exhaustive match: {} not covered",
                    quoted_list("pattern", "patterns", missing)
                ),
            ),
            TypeDiagnosticKind::RefutableBinding { missing } => {
                help = Some("use `match` to handle the other cases".to_owned());
                (
                    Severity::Error,
                    Some(code::REFUTABLE_BINDING),
                    format!(
                        "refutable pattern in binding: {} not covered",
                        quoted_list("pattern", "patterns", missing)
                    ),
                )
            }
            TypeDiagnosticKind::UnreachableArm => (
                Severity::Warning,
                Some(code::UNREACHABLE_MATCH_ARM),
                "unreachable match arm".to_owned(),
            ),
            TypeDiagnosticKind::UnhandledErrors { errors, declared } => {
                let list = quoted_list("error", "errors", errors);
                if *declared {
                    help = Some("add it to `fails` or handle it with `escape`".to_owned());
                    let verb = if errors.len() == 1 { "is" } else { "are" };
                    (
                        Severity::Error,
                        Some(code::UNHANDLED_ERRORS),
                        format!("{list} {verb} not declared in `fails`"),
                    )
                } else {
                    help = Some("handle it with `escape`, or declare it with `fails`".to_owned());
                    (
                        Severity::Error,
                        Some(code::UNHANDLED_ERRORS),
                        format!("unhandled {list}"),
                    )
                }
            }
            TypeDiagnosticKind::NotAnError { found } => (
                Severity::Error,
                Some(code::NOT_AN_ERROR),
                format!("expected an error value, found {}", show(found)),
            ),
            TypeDiagnosticKind::EscapeArmNotError => (
                Severity::Error,
                Some(code::ESCAPE_ARM_NOT_ERROR),
                "escape arms must match an error constructor".to_owned(),
            ),
            TypeDiagnosticKind::ErrorSetMismatch { expected, found } => (
                Severity::Error,
                Some(code::ERROR_SET_MISMATCH),
                format!("error set mismatch: expected `fails {expected}`, found `fails {found}`"),
            ),
            TypeDiagnosticKind::UnraisedError { error } => (
                Severity::Warning,
                Some(code::UNRAISED_ERROR),
                format!("error `{error}` is never raised here"),
            ),
            TypeDiagnosticKind::UnknownOperation { effect, op } => (
                Severity::Error,
                Some(code::UNKNOWN_OPERATION),
                format!("effect `{effect}` has no operation `{op}`"),
            ),
            TypeDiagnosticKind::UnhandledEffects { effects, context } => {
                let list = quoted_list("effect", "effects", effects);
                let verb = if effects.len() == 1 { "is" } else { "are" };
                let message = match context {
                    EffectContext::Declared => {
                        help = Some("add it to `use` or provide it with `with`".to_owned());
                        format!("{list} {verb} not declared in `use`")
                    }
                    EffectContext::Const => format!("constant values cannot use {list}"),
                    EffectContext::Main => {
                        help = Some("provide it with `with`".to_owned());
                        format!("{list} {verb} not provided in `main`")
                    }
                    EffectContext::Test { name } => {
                        help = Some("provide it with `with`".to_owned());
                        format!("{list} {verb} not provided in test function `{name}`")
                    }
                };
                (Severity::Error, Some(code::UNHANDLED_EFFECTS), message)
            }
            TypeDiagnosticKind::EffectSetMismatch { expected, found } => (
                Severity::Error,
                Some(code::EFFECT_SET_MISMATCH),
                format!("effect set mismatch: expected `use {expected}`, found `use {found}`"),
            ),
            TypeDiagnosticKind::MissingHandlerOps { handler, ops } => (
                Severity::Error,
                Some(code::MISSING_HANDLER_OPS),
                format!(
                    "handler `{handler}` does not implement {}",
                    quoted_list("operation", "operations", ops)
                ),
            ),
            TypeDiagnosticKind::DuplicateHandlerOp { op } => (
                Severity::Error,
                Some(code::DUPLICATE_HANDLER_OP),
                format!("operation `{op}` is implemented more than once"),
            ),
            TypeDiagnosticKind::HandlerOpArgCount {
                op,
                expected,
                found,
            } => (
                Severity::Error,
                Some(code::HANDLER_OP_ARG_COUNT),
                format!(
                    "wrong number of parameters: operation `{op}` takes {expected}, found {found}"
                ),
            ),
            TypeDiagnosticKind::ExternalOutsideHandler { name } => {
                help = Some("declare it with `use {}` if it has no side effects".to_owned());
                (
                    Severity::Error,
                    Some(code::EXTERNAL_OUTSIDE_HANDLER),
                    format!("external function `{name}` can only be used in handlers"),
                )
            }
            TypeDiagnosticKind::CapabilityArgument => {
                help = Some("pass a closure that uses the capability instead".to_owned());
                (
                    Severity::Error,
                    Some(code::CAPABILITY_ARGUMENT),
                    "a capability cannot be passed as an argument".to_owned(),
                )
            }
            TypeDiagnosticKind::CapabilityInData => (
                Severity::Error,
                Some(code::CAPABILITY_IN_DATA),
                "a capability cannot be stored in a data structure".to_owned(),
            ),
            TypeDiagnosticKind::CapabilityReturned => (
                Severity::Error,
                Some(code::CAPABILITY_RETURNED),
                "a capability cannot be returned from a function".to_owned(),
            ),
            TypeDiagnosticKind::HandlerCycle { cycle } => {
                let path: Vec<String> = cycle.iter().map(|h| format!("`{h}`")).collect();
                (
                    Severity::Error,
                    Some(code::HANDLER_CYCLE),
                    format!("handlers depend on each other: {}", path.join(" -> ")),
                )
            }
        };
        let mut out = Diagnostic::new(severity, message);
        if let Some(code) = code {
            out = out.with_code(code);
        }
        if let Some(span) = span {
            out = out.with_span(span);
        }
        if let Some(help) = help {
            out = out.with_help(help);
        }
        out
    }
}

/// 名前を `` `a` `` か `` fields `a`, `b` `` の形で並べる．4つ以上なら先頭の3つと `and N more`．
fn quoted_list(one: &str, many: &str, names: &[SmolStr]) -> String {
    match names {
        [only] => format!("{one} `{only}`"),
        _ => {
            let shown: Vec<String> = names.iter().take(3).map(|n| format!("`{n}`")).collect();
            let mut list = shown.join(", ");
            if names.len() > 3 {
                list.push_str(&format!(" and {} more", names.len() - 3));
            }
            format!("{many} {list}")
        }
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
