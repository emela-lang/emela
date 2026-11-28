//! Diagnostics of emela-resolve: the module tree (file and module names, imports) and name
//! resolution.

use std::path::Path;

use emela_resolve::{DiagnosticKind, NameError};
use line_index::TextRange;

use super::{Diagnostic, Location, NAMED_ITEM_SUGGESTION, Severity, Span};
use crate::diagnostic::code;
use crate::source::SourceDb;

impl Diagnostic {
    /// Points to a file by ID if its path is in the source table, otherwise keeps the path.
    pub fn from_resolve(diagnostic: &emela_resolve::Diagnostic, sources: &SourceDb) -> Self {
        let location = match (sources.file_id(&diagnostic.file), diagnostic.range) {
            (Some(file), Some(range)) => Location::Span(Span::new(file, range)),
            (Some(file), None) => Location::File(file),
            (None, _) => Location::Path(diagnostic.file.clone()),
        };
        let display = |path: &Path| format!("`{}`", sources.display_path(path));
        let (code, message) = match &diagnostic.kind {
            DiagnosticKind::InvalidFileName { name, error } => (
                code::NOT_A_MODULE_NAME,
                format!(
                    "file name `{name}` cannot be a module name: {}",
                    name_error(error)
                ),
            ),
            DiagnosticKind::InvalidDirName { name, error } => (
                code::NOT_A_MODULE_NAME,
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
            // `Std.Fiber` was removed when tasks replaced fibers (`[task]`).
            DiagnosticKind::UndefinedModule { name } if name.to_string() == "Std.Fiber" => {
                return Diagnostic::error(format!("undefined module `{name}`"))
                    .with_code(code::UNDEFINED_MODULE)
                    .with_help("`Std.Fiber` was replaced by `Std.Task` (`[task]`)")
                    .at(location);
            }
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

    /// Name resolution diagnostics (declarations, imports, names in expressions): E2101 to
    /// E2109, W2101 to W2102.
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
                    code::OPAQUE_OUTSIDE_MODULE,
                    format!("cannot construct opaque `{name}` outside module `{module}`"),
                )
            }
            DiagnosticKind::OpaquePattern { name, module } => (
                Severity::Error,
                code::OPAQUE_OUTSIDE_MODULE,
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
            DiagnosticKind::SelfValueOutside => {
                help = Some(
                    "inside a handler's `init`, `Self(...)` constructs that handler".to_owned(),
                );
                (
                    Severity::Error,
                    code::SELF_OUTSIDE,
                    "`Self` can only be used as a value in the `init` of a handler".to_owned(),
                )
            }
            DiagnosticKind::DeprecatedUse { effect, rewrite } => {
                help = Some(match rewrite {
                    Some(emela_resolve::DeprecatedRewrite::InPlace { alias, op }) => format!(
                        "write `{alias} <- {effect}` before the statement, and call `{alias}.{op}` here"
                    ),
                    Some(emela_resolve::DeprecatedRewrite::Binding { alias }) => {
                        format!("write `{alias} <- {effect}`")
                    }
                    None => format!("write `x <- {effect}`, or {NAMED_ITEM_SUGGESTION}"),
                });
                (
                    Severity::Warning,
                    code::DEPRECATED_USE,
                    format!("`use {effect}` as an expression is deprecated"),
                )
            }
            DiagnosticKind::EffectAliasAsValue { name } => {
                help = Some(format!("for a fixed value, use {NAMED_ITEM_SUGGESTION}"));
                (
                    Severity::Error,
                    code::EFFECT_ALIAS_MISUSE,
                    format!(
                        "`{name}` is an effect alias, not a value: only `{name}.op(...)` and `{name}.op` can be written"
                    ),
                )
            }
            DiagnosticKind::MemberThroughEffectAlias {
                alias,
                effect,
                member,
            } => {
                help = Some(format!(
                    "open it through the effect: `x = {effect}.{member}(...)`"
                ));
                (
                    Severity::Error,
                    code::EFFECT_ALIAS_MISUSE,
                    format!(
                        "handler member `{member}` cannot be opened through effect alias `{alias}`"
                    ),
                )
            }
            DiagnosticKind::HandlerNameAsValue {
                handler,
                deprecated,
                update,
            } => {
                help = Some(match update {
                    Some(update) => format!("write `Self` with every field, as in `{update}`"),
                    None => format!("write `Self` in place of `{handler}`"),
                });
                if *deprecated {
                    (
                        Severity::Warning,
                        code::DEPRECATED_HANDLER_NAME_AS_VALUE,
                        format!("constructing `{handler}` by its name is deprecated"),
                    )
                } else {
                    (
                        Severity::Error,
                        code::HANDLER_NAME_AS_VALUE,
                        format!("the value of `{handler}` is made with `Self`"),
                    )
                }
            }
            DiagnosticKind::HandlerNamePattern {
                handler,
                deprecated,
                field,
            } => {
                help = Some(format!("read the fields with `.`, as in `self.{field}`"));
                if *deprecated {
                    (
                        Severity::Warning,
                        code::DEPRECATED_HANDLER_NAME_PATTERN,
                        format!("matching `{handler}` by its name is deprecated"),
                    )
                } else {
                    (
                        Severity::Error,
                        code::HANDLER_NAME_PATTERN,
                        format!("handler `{handler}` cannot be matched by its name"),
                    )
                }
            }
            DiagnosticKind::SelfUpdate => {
                help = Some("`Self` constructs a handler from its fields only".to_owned());
                (
                    Severity::Error,
                    code::INVALID_UPDATE_TARGET,
                    "cannot update `Self` with `..`".to_owned(),
                )
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
