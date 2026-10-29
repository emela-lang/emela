//! Diagnostics shared by all stages. Lexer, resolver and type checker diagnostics are converted
//! into these before rendering.
//!
//! The original diagnostic types stay in their crates; only the conversions live here.

use std::path::{Path, PathBuf};

use emela_resolve::ModuleId;
use emela_resolve::{DiagnosticKind, NameError};
use emela_types::{
    DiagnosticKind as TypeDiagnosticKind, EffectContext, ImmediateSource, SuspendingCall, Ty,
    TyCons, TypeError, TypeErrorKind,
};
use la_arena::ArenaMap;
use line_index::TextRange;
use smol_str::SmolStr;

use crate::code;
use crate::source::{FileId, SourceDb};

/// The prefix of notes that are help.
pub const HELP_PREFIX: &str = "help: ";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Error,
    Warning,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    Span(Span),
    /// The whole file (a bad file name, ...).
    File(FileId),
    /// A path not in the source table (a directory, a file that could not be read).
    Path(PathBuf),
}

/// An explanation attached somewhere other than the primary location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub span: Span,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    /// The error code ([`crate::code`]); `None` for stages that do not assign one yet (lexing).
    pub code: Option<&'static str>,
    pub message: String,
    /// `None` for diagnostics without a location (node not found, ...).
    pub location: Option<Location>,
    pub labels: Vec<Label>,
    /// Notes starting with [`HELP_PREFIX`] are printed as help (`= help: …`).
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

    /// Printed as `= help: …`. Stored as a note rather than a new field, to keep the error side
    /// of `Result` small.
    pub fn with_help(self, help: impl AsRef<str>) -> Self {
        self.with_note(format!("{HELP_PREFIX}{}", help.as_ref()))
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// The code (E01xx) and English message come from emela-syntax.
    pub fn from_syntax(file: FileId, diagnostic: &emela_syntax::Diagnostic) -> Self {
        Diagnostic::error(&diagnostic.message)
            .with_code(diagnostic.code.as_str())
            .with_span(Span::new(file, diagnostic.range))
    }

    /// Points to a file by ID if its path is in the source table, otherwise keeps the path.
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
            // Paths in the message are relative to the base, like the diagnostic's location.
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
            DiagnosticKind::TestOnlyModule { name } => {
                return Diagnostic::error(format!(
                    "`{name}` can only be imported under `test/`"
                ))
                .with_code(code::TEST_ONLY_MODULE)
                .with_note(
                    "the test root and [dev-dependencies] are only for the tests; move the tests that use it under test/ (13.1)",
                )
                .at(location);
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

    /// Name resolution diagnostics (declarations, imports, names in expressions): E0211 to
    /// E0220, W0201 to W0202.
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
            DiagnosticKind::ImplHead { head } => (
                Severity::Error,
                code::IMPL_HEAD,
                format!("impl head must be a type with distinct type parameters: `{head}`"),
            ),
            DiagnosticKind::FnNamedLikeField { name, ty } => (
                Severity::Error,
                code::FN_NAMED_LIKE_FIELD,
                format!("function `{name}` has the same name as a field of `{ty}`"),
            ),
            _ => unreachable!("module and import graph diagnostics are handled by from_resolve"),
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

    /// `TypeError` has no location, so the caller passes `span`.
    pub fn from_type_error(error: &TypeError, cons: &TyCons, span: Span) -> Self {
        Diagnostic::type_error(error, cons).with_span(span)
    }

    /// A type mismatch diagnostic without a location.
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
    /// Places that type-checked but cannot be lowered to Core IR yet. The message matches the
    /// type checker's `Unsupported`, and the location is found as in [`Diagnostic::from_type`].
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
            emela_core::LowerError::SuspendingExport { name, span } => (
                Diagnostic::error(format!(
                    "exported function `{name}` may suspend, which target `c` does not support"
                ))
                .with_code(code::SUSPENDING_EXPORT),
                span,
            ),
            emela_core::LowerError::WasmExport { name, span } => (
                Diagnostic::error(format!(
                    "exported function `{name}` has a `wasm` export, which the WASI 0.3 component target does not support yet"
                )),
                span,
            ),
            emela_core::LowerError::HttpServiceOnC { span } => (
                Diagnostic::error("HTTP services are not supported on target `c` yet")
                    .with_code(code::HTTP_SERVICE_ON_C),
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

    /// The location is a HIR position (module and range), whose file is looked up in `files`.
    /// Positions without source (Prelude declarations) get no location.
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
        let mut note = None;
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
            TypeDiagnosticKind::NoFieldOrFunction { ty, name } => (
                Severity::Error,
                Some(code::NO_FIELD),
                format!("no field or function `{name}` for type {}", show(ty)),
            ),
            TypeDiagnosticKind::MethodNotCalled { ty, name, receiver } => {
                let receiver = receiver.as_deref().unwrap_or("x");
                help = Some(format!("call it as `{receiver}.{name}(...)`"));
                (
                    Severity::Error,
                    Some(code::NO_FIELD),
                    format!("no field `{name}` for type {}", show(ty)),
                )
            }
            TypeDiagnosticKind::NoFields { ty, field } => (
                Severity::Error,
                Some(code::NO_FIELDS),
                format!("type {} has no field or function `{field}`", show(ty)),
            ),
            TypeDiagnosticKind::FieldOnUnknownType { field, call } => {
                help = Some("add a type annotation".to_owned());
                let what = if *call {
                    format!("call `{field}`")
                } else {
                    format!("access field `{field}`")
                };
                (
                    Severity::Error,
                    Some(code::FIELD_ON_UNKNOWN_TYPE),
                    format!("the type of this value must be known to {what}"),
                )
            }
            TypeDiagnosticKind::AmbiguousMethod {
                ty,
                name,
                field,
                qualified,
                receiver,
            } => {
                let receiver = receiver.as_deref().unwrap_or("x");
                let mut candidates: Vec<String> = Vec::new();
                if *field {
                    candidates.push(format!("the field `{name}`"));
                }
                candidates.extend(qualified.iter().map(|q| format!("`{q}`")));
                let calls: Vec<String> = qualified
                    .iter()
                    .map(|q| format!("`{q}({receiver})`"))
                    .collect();
                let mut text = format!("write {}", or_list(&calls));
                if *field {
                    text.push_str(&format!(
                        ", or take the field out first (`f = {receiver}.{name}`)"
                    ));
                }
                help = Some(text);
                (
                    Severity::Error,
                    Some(code::AMBIGUOUS_METHOD),
                    format!(
                        "ambiguous method `{name}` for {}: {}",
                        show(ty),
                        and_list(&candidates)
                    ),
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
            TypeDiagnosticKind::MissingInitParamType { handler, param } => (
                Severity::Error,
                Some(code::MISSING_PARAM_TYPE),
                format!("parameter `{param}` of the `init` of `{handler}` needs a type"),
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
            TypeDiagnosticKind::UnusedDerive { trait_name, ty } => {
                // dbg and a failed assert show a value by its Show if it has one (spec 10.6,
                // 13.2), and do not count as uses.
                help = Some(if trait_name == "Show" {
                    format!(
                        "remove it from `derive`; `dbg` then shows `{ty}` as constructed, and a failed `assert` no longer shows its values"
                    )
                } else {
                    "remove it from `derive`".to_owned()
                });
                (
                    Severity::Warning,
                    Some(code::UNUSED_DERIVE),
                    format!("derived `{trait_name}` for `{ty}` is never used"),
                )
            }
            TypeDiagnosticKind::UnreachableArm => (
                Severity::Warning,
                Some(code::UNREACHABLE_MATCH_ARM),
                "unreachable match arm".to_owned(),
            ),
            TypeDiagnosticKind::UnhandledErrors {
                errors,
                declared,
                layer_arg,
            } => {
                let list = quoted_list("error", "errors", errors);
                if let Some(arg) = layer_arg {
                    help = Some(layer_arg_help(arg));
                    (
                        Severity::Error,
                        Some(code::UNHANDLED_ERRORS),
                        format!("unhandled {list}"),
                    )
                } else if *declared {
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
            TypeDiagnosticKind::UnusedLabelProvider { label } => (
                Severity::Warning,
                Some(code::UNUSED_LABEL_PROVIDER),
                format!("label `{label}` is listed here but never used"),
            ),
            TypeDiagnosticKind::ExportOutsideEntry { attr } => (
                Severity::Error,
                Some(code::EXPORT_OUTSIDE_ENTRY),
                format!("`@{attr}` is only allowed in the entry module"),
            ),
            TypeDiagnosticKind::NotEntryAdapter { name } => (
                Severity::Error,
                Some(code::NOT_ENTRY_ADAPTER),
                match name {
                    Some(name) => format!("`{name}` is not an entry adapter"),
                    None => "the argument of `@entry` must name an entry adapter".to_owned(),
                },
            ),
            TypeDiagnosticKind::DuplicateExport { name, target } => (
                Severity::Error,
                Some(code::DUPLICATE_EXPORT),
                format!("`{name}` is exported twice for target `{target}`"),
            ),
            TypeDiagnosticKind::NotBoundary { function, ty, ret } => (
                Severity::Error,
                Some(code::NOT_BOUNDARY_EXPORT),
                if *ret {
                    format!("exported function `{function}` cannot return `{ty}`")
                } else {
                    format!("exported function `{function}` cannot take `{ty}`")
                },
            ),
            TypeDiagnosticKind::ExportFails { function, errors } => (
                Severity::Error,
                Some(code::UNHANDLED_EFFECTS),
                format!("exported function `{function}` cannot fail with `{errors}`"),
            ),
            TypeDiagnosticKind::EntryMismatch { entry, adapter } => (
                Severity::Error,
                Some(code::ENTRY_MISMATCH),
                format!("`{entry}` does not fit `{adapter}`"),
            ),
            TypeDiagnosticKind::UnknownOperation { effect, op } => (
                Severity::Error,
                Some(code::UNKNOWN_OPERATION),
                format!("effect `{effect}` has no operation `{op}`"),
            ),
            TypeDiagnosticKind::UnhandledEffects {
                effects,
                context,
                scope,
            } => {
                if let Some(scope) = scope {
                    note = Some(format!(
                        "fibers forked in `{scope}` get their handlers from where `{scope}` is called, so `with` inside it does not reach them"
                    ));
                }
                let list = quoted_list("effect", "effects", effects);
                let verb = if effects.len() == 1 { "is" } else { "are" };
                let message = match context {
                    EffectContext::Declared => {
                        help = Some(match scope {
                            Some(scope) => format!(
                                "add it to `use` or provide it with `with` outside `{scope}`"
                            ),
                            None => "add it to `use` or provide it with `with`".to_owned(),
                        });
                        format!("{list} {verb} not declared in `use`")
                    }
                    EffectContext::Const => format!("constant values cannot use {list}"),
                    EffectContext::LayerArg(arg) => {
                        help = Some(layer_arg_help(arg));
                        format!("arguments in layer `{}` cannot use {list}", arg.layer)
                    }
                    EffectContext::Main => {
                        help = Some(match scope {
                            Some(scope) => format!("provide it with `with` outside `{scope}`"),
                            None => "provide it with `with`".to_owned(),
                        });
                        format!("{list} {verb} not provided in `main`")
                    }
                    EffectContext::Export { name } => {
                        help = Some(match scope {
                            Some(scope) => format!("provide it with `with` outside `{scope}`"),
                            None => "provide it with `with`".to_owned(),
                        });
                        format!("{list} {verb} not provided in exported function `{name}`")
                    }
                    EffectContext::Test { name } => {
                        help = Some(match scope {
                            Some(scope) => format!("provide it with `with` outside `{scope}`"),
                            None => "provide it with `with`".to_owned(),
                        });
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
            TypeDiagnosticKind::ExternalOutsideHandler { name, suspend } => {
                if !*suspend {
                    help = Some("declare it with `use {}` if it has no side effects".to_owned());
                }
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
            TypeDiagnosticKind::ConstCycle { cycle } => {
                let path: Vec<String> = cycle.iter().map(|c| format!("`{c}`")).collect();
                (
                    Severity::Error,
                    Some(code::CONST_CYCLE),
                    format!("constants refer to each other: {}", path.join(" -> ")),
                )
            }
            TypeDiagnosticKind::HandlerCtorOutsideInit { handler } => {
                help = Some(format!(
                    "use `with {handler} {{ ... }}`; `init` and `release` run there"
                ));
                (
                    Severity::Error,
                    Some(code::HANDLER_CTOR_OUTSIDE_INIT),
                    format!("handler `{handler}` can only be constructed in its own `init`"),
                )
            }
            TypeDiagnosticKind::MissingInit { handler } => {
                help = Some(format!(
                    "add `init {{ ... }}` that returns `{handler}(...)`"
                ));
                (
                    Severity::Error,
                    Some(code::MISSING_INIT),
                    format!("handler `{handler}` has fields but no `init`"),
                )
            }
            TypeDiagnosticKind::MissingInitArgs { handler, params } => {
                let args: Vec<String> = params.iter().map(|p| format!("{p}: ...")).collect();
                help = Some(format!("write `{handler}({})`", args.join(", ")));
                (
                    Severity::Error,
                    Some(code::MISSING_INIT_ARGS),
                    format!("handler `{handler}` needs arguments for its `init`"),
                )
            }
            TypeDiagnosticKind::UnexpectedInitArgs { name, kind } => {
                help = Some(format!("write `{name}` without parentheses"));
                (
                    Severity::Error,
                    Some(code::UNEXPECTED_INIT_ARGS),
                    format!("{kind} `{name}` takes no arguments"),
                )
            }
            TypeDiagnosticKind::ReleaseFails { handler, errors } => {
                help = Some(
                    "handle it with `escape`, or turn it into a defect with `panic`".to_owned(),
                );
                let list = quoted_list("error", "errors", errors);
                (
                    Severity::Error,
                    Some(code::RELEASE_FAILS),
                    format!(
                        "`release` of handler `{handler}` cannot fail, but this may fail with {list}"
                    ),
                )
            }
            TypeDiagnosticKind::DuplicateLayerEffect {
                layer,
                effect,
                first,
                second,
            } => {
                let message = if first == second {
                    format!("handler `{first}` appears more than once in layer `{layer}`")
                } else {
                    format!(
                        "effect `{effect}` is provided by both `{first}` and `{second}` in layer `{layer}`"
                    )
                };
                if first != second {
                    help = Some(format!(
                        "keep one of them, or override it with `with {layer}, {second} {{ ... }}`"
                    ));
                }
                (Severity::Error, Some(code::DUPLICATE_LAYER_EFFECT), message)
            }
            TypeDiagnosticKind::LayerCycle { cycle } => {
                let path: Vec<String> = cycle.iter().map(|l| format!("`{l}`")).collect();
                (
                    Severity::Error,
                    Some(code::LAYER_CYCLE),
                    format!("layers include each other: {}", path.join(" -> ")),
                )
            }
            TypeDiagnosticKind::MissingEffectTypeArgs {
                effect,
                params,
                handler,
            } => {
                let holes = vec!["..."; params.len()].join(", ");
                let message = match handler {
                    Some(handler) => {
                        help = Some(format!(
                            "write the types, like `handler {handler}: {effect}[{holes}]`; a handler cannot be generic"
                        ));
                        format!(
                            "handler `{handler}` must implement `{effect}[{}]` with concrete types",
                            params.join(", ")
                        )
                    }
                    None => {
                        help = Some(format!("write the types, like `{effect}[{holes}]`"));
                        format!(
                            "missing type arguments for effect `{effect}[{}]`",
                            params.join(", ")
                        )
                    }
                };
                (
                    Severity::Error,
                    Some(code::MISSING_EFFECT_TYPE_ARGS),
                    message,
                )
            }
            TypeDiagnosticKind::EffectParamNotType { effect, param } => (
                Severity::Error,
                Some(code::EFFECT_PARAM_NOT_TYPE),
                format!(
                    "type parameter `{param}` of effect `{effect}` is used as a set, but effect type parameters must be ordinary types"
                ),
            ),
            TypeDiagnosticKind::LabelBase {
                label,
                base,
                reason,
            } => {
                let why = match reason {
                    emela_types::LabelBaseReason::Label(inner) => {
                        help = Some(format!(
                            "write `effect {label} of {inner}`; `{base}` can still be plugged into it with `with {label} by {base}`"
                        ));
                        format!("`{base}` is a label")
                    }
                    emela_types::LabelBaseReason::BuiltIn => {
                        format!("`{base}` is a built-in effect")
                    }
                };
                (
                    Severity::Error,
                    Some(code::LABEL_BASE),
                    format!("label `{label}` cannot stand for `{base}`: {why}"),
                )
            }
            TypeDiagnosticKind::NotALabel { name, by } => {
                if *by {
                    (
                        Severity::Error,
                        Some(code::NOT_A_LABEL),
                        format!("only a label can take `by`, but `{name}` is not a label"),
                    )
                } else {
                    help = Some(format!(
                        "list a handler of `{name}`, or declare a label with `effect L of {name}`"
                    ));
                    (
                        Severity::Error,
                        Some(code::NOT_A_LABEL),
                        format!(
                            "effect `{name}` is not a label: `with` takes a handler, a layer or a label"
                        ),
                    )
                }
            }
            TypeDiagnosticKind::LabelMismatch {
                label,
                base,
                provider,
                provided,
            } => (
                Severity::Error,
                Some(code::LABEL_MISMATCH),
                format!(
                    "label `{label}` stands for `{base}`, but `{provider}` provides `{provided}`"
                ),
            ),
            TypeDiagnosticKind::LabelProvidedTwice { label } => (
                Severity::Error,
                Some(code::LABEL_PROVIDED_TWICE),
                format!("label `{label}` is given a provider more than once in this `with`"),
            ),
            TypeDiagnosticKind::HandlerOfLabel {
                handler,
                label,
                base,
            } => {
                help = Some(format!(
                    "implement `{base}` instead, and plug it in with `with {label} by {handler}`"
                ));
                (
                    Severity::Error,
                    Some(code::HANDLER_OF_LABEL),
                    format!("handler `{handler}` cannot implement label `{label}`"),
                )
            }
            TypeDiagnosticKind::AsyncHandler { handler } => (
                Severity::Error,
                Some(code::ASYNC_HANDLER),
                format!(
                    "handler `{handler}` cannot implement `Async`: only the runtime provides it"
                ),
            ),
            TypeDiagnosticKind::IntrinsicHandler {
                handler,
                effect,
                provider,
            } => (
                Severity::Error,
                Some(code::ASYNC_HANDLER),
                format!(
                    "handler `{handler}` cannot implement `{effect}`: only `{provider}` provides it"
                ),
            ),
            TypeDiagnosticKind::EscapingFromScope { ty, scope } => {
                note = Some(format!(
                    "every fiber has ended once `{scope}` returns, so the value would be of no use outside it"
                ));
                (
                    Severity::Error,
                    Some(code::ESCAPING_FROM_SCOPE),
                    format!("a `{ty}` cannot leave the `{scope}` that started it"),
                )
            }
            TypeDiagnosticKind::OpTypeParams { op } => (
                Severity::Error,
                Some(code::OP_TYPE_PARAMS),
                format!(
                    "operation `{op}` cannot have type parameters: only built-in effects may declare them"
                ),
            ),
            TypeDiagnosticKind::UseAsync => (
                Severity::Error,
                Some(code::USE_ASYNC),
                "`Async` has no operations and cannot be used with `use`".to_owned(),
            ),
            TypeDiagnosticKind::SuspendMismatch {
                op,
                effect,
                suspend,
            } => {
                if *suspend {
                    help = Some(format!("implement it with `suspend fn {op}(..., resume)`"));
                    (
                        Severity::Error,
                        Some(code::SUSPEND_MISMATCH),
                        format!("operation `{op}` is a `suspend fn` in effect `{effect}`"),
                    )
                } else {
                    help = Some("remove `suspend`".to_owned());
                    (
                        Severity::Error,
                        Some(code::SUSPEND_MISMATCH),
                        format!("operation `{op}` is not a `suspend fn` in effect `{effect}`"),
                    )
                }
            }
            TypeDiagnosticKind::SuspendingFnOp { op, call } => {
                let declare = format!("declare `{op}` as a `suspend fn` in the effect");
                let cause = match call {
                    SuspendingCall::Op { effect, op: called } => {
                        help = Some(declare);
                        format!(
                            "operation `{called}` of `{effect}` is a `suspend fn` and may suspend"
                        )
                    }
                    SuspendingCall::External { callee } => {
                        help = Some(declare);
                        format!("external function `{callee}` may suspend")
                    }
                    SuspendingCall::Fn { callee, effects } => {
                        // Going through a function counts as possibly suspending even if it only
                        // calls fn operations (spec 8.1). With only Async, calling the operation
                        // directly changes nothing, so no hint is given.
                        let ops: Vec<String> = effects
                            .iter()
                            .filter(|e| e.as_str() != "Async")
                            .map(|e| format!("`{e}`"))
                            .collect();
                        help = Some(if ops.is_empty() {
                            declare
                        } else {
                            format!(
                                "{declare}, or call the `fn` operations of {} directly instead of through a function",
                                ops.join(", ")
                            )
                        });
                        let list = quoted_list("effect", "effects", effects);
                        match callee {
                            Some(f) => format!("calling `{f}` may suspend because it uses {list}"),
                            None => format!("this call may suspend because it uses {list}"),
                        }
                    }
                };
                (
                    Severity::Error,
                    Some(code::SUSPENDING_FN_OP),
                    format!("{cause}, but operation `{op}` is not a `suspend fn`"),
                )
            }
            TypeDiagnosticKind::NotImmediate {
                effects,
                sets,
                required,
            } => {
                let mut parts = Vec::new();
                if !effects.is_empty() {
                    parts.push(quoted_list("effect", "effects", effects));
                }
                if !sets.is_empty() {
                    parts.push(quoted_list("effect set", "effect sets", sets));
                    help = Some("constrain the effect set with `: Immediate`".to_owned());
                }
                let list = parts.join(" and ");
                let message = match required {
                    ImmediateSource::Bound { param, callee } => format!(
                        "{list} may suspend, but `{param}` of `{callee}` must be `Immediate`"
                    ),
                    ImmediateSource::ExternalCallback { callee } => format!(
                        "{list} may suspend, but callbacks passed to external function `{callee}` must be `Immediate`"
                    ),
                };
                (Severity::Error, Some(code::NOT_IMMEDIATE), message)
            }
            TypeDiagnosticKind::SetAsType => (
                Severity::Error,
                Some(code::SET_AS_TYPE),
                "expected a type, found a set: sets can only be passed to set type parameters"
                    .to_owned(),
            ),
            TypeDiagnosticKind::EffectAsType { name } => {
                help = Some(format!(
                    "a capability cannot be passed around; take `use {name}` where it is needed, or pass a closure that captures it"
                ));
                (
                    Severity::Error,
                    Some(code::EFFECT_AS_TYPE),
                    format!("effect `{name}` cannot be used as a type"),
                )
            }
            TypeDiagnosticKind::SetArgMismatch {
                param,
                ty,
                effects,
                found,
            } => {
                let set = if *effects {
                    "an effect set"
                } else {
                    "an error set"
                };
                help = Some(if *effects {
                    "write an effect or an effect set type parameter".to_owned()
                } else {
                    "write an error or an error set type parameter".to_owned()
                });
                (
                    Severity::Error,
                    Some(code::SET_ARG_MISMATCH),
                    format!("`{param}` of `{ty}` is {set}, found {found}"),
                )
            }
            TypeDiagnosticKind::NotImplemented { ty, trait_name } => (
                Severity::Error,
                Some(code::NOT_IMPLEMENTED),
                format!("type `{ty}` does not implement `{trait_name}`"),
            ),
            TypeDiagnosticKind::NotDerivable { trait_name } => (
                Severity::Error,
                Some(code::NOT_IMPLEMENTED),
                format!("cannot derive `{trait_name}`: only `Eq`, `Ord` and `Show` can be derived"),
            ),
            TypeDiagnosticKind::SupertraitCycle { cycle } => (
                Severity::Error,
                Some(code::SUPERTRAIT_CYCLE),
                format!(
                    "trait `{}` is its own supertrait: {}",
                    cycle[0],
                    cycle
                        .iter()
                        .map(|t| format!("`{t}`"))
                        .collect::<Vec<_>>()
                        .join(" -> ")
                ),
            ),
            TypeDiagnosticKind::DuplicateImpl { ty, trait_name } => (
                Severity::Error,
                Some(code::DUPLICATE_IMPL),
                format!("`{trait_name}` is already implemented for `{ty}`"),
            ),
            TypeDiagnosticKind::OrphanImpl { ty, trait_name } => {
                help = Some(format!(
                    "wrap `{ty}` in a type of this module and implement `{trait_name}` for it"
                ));
                (
                    Severity::Error,
                    Some(code::ORPHAN_IMPL),
                    format!(
                        "`{trait_name}` for `{ty}` can only be implemented in the module of \
                         `{trait_name}` or of `{ty}`"
                    ),
                )
            }
            TypeDiagnosticKind::AmbiguousSelf { trait_name } => (
                Severity::Error,
                Some(code::AMBIGUOUS_SELF),
                format!("cannot infer which impl of `{trait_name}` to use: add a type annotation"),
            ),
            TypeDiagnosticKind::ImplFnParamCount {
                name,
                trait_name,
                expected,
                found,
            } => (
                Severity::Error,
                Some(code::IMPL_FN_PARAM_COUNT),
                format!(
                    "wrong number of parameters: `{name}` of `{trait_name}` takes {expected}, \
                     found {found}"
                ),
            ),
            TypeDiagnosticKind::ImmediateOnNonEffect { param } => (
                Severity::Error,
                Some(code::IMMEDIATE_ON_NON_EFFECT),
                format!(
                    "`Immediate` can only constrain effect sets, but `{param}` is not used in `use`"
                ),
            ),
        };
        let mut out = Diagnostic::new(severity, message);
        if let Some(code) = code {
            out = out.with_code(code);
        }
        if let Some(span) = span {
            out = out.with_span(span);
        }
        if let Some(note) = note {
            out = out.with_note(note);
        }
        if let Some(help) = help {
            out = out.with_help(help);
        }
        out
    }
}

/// The help of E0318 and E0402 in the arguments of a layer item (`[effect.layer]`).
fn layer_arg_help(arg: &emela_types::LayerArg) -> String {
    format!(
        "the arguments in a layer must be pure; to pass this value, list `{}` after the layer: `with {}, {}(...) {{ ... }}`",
        arg.handler, arg.layer, arg.handler
    )
}

/// Lists names as `` `a` `` or `` fields `a`, `b` ``. Four or more show the first three and
/// `and N more`.
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

/// The English message for emela-resolve's `NameError`.
fn name_error(error: &NameError) -> String {
    match error {
        NameError::NotSnakeCase => "not lower snake_case".to_owned(),
        NameError::NotTypeName { converted } => {
            format!("the converted name `{converted}` is not a valid type name")
        }
    }
}

pub fn error_count(diagnostics: &[Diagnostic]) -> usize {
    diagnostics.iter().filter(|d| d.is_error()).count()
}

/// `a`, `a and b`, `a, b and c`.
fn and_list(items: &[String]) -> String {
    joined(items, "and")
}

/// `a`, `a or b`, `a, b or c`.
fn or_list(items: &[String]) -> String {
    joined(items, "or")
}

fn joined(items: &[String], word: &str) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} {word} {last}", init.join(", ")),
    }
}
