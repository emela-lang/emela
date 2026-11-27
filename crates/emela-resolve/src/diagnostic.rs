//! Name-resolution diagnostics. Resolution does not stop at the first one; all are collected and
//! returned together.

use std::fmt;
use std::path::{Path, PathBuf};

use rowan::TextRange;
use smol_str::SmolStr;

use crate::ModuleName;
use crate::naming::NameError;

/// A path as the compiler and tools show it: a relative path (a file in the project, relative to
/// the project directory) has its components separated by `/` on every OS, so the output of a
/// project is the same on Windows (#812). An absolute path keeps the separators of the OS.
pub fn shown_path(path: &Path) -> String {
    if path.is_absolute() || path.has_root() {
        return path.display().to_string();
    }
    let mut shown = String::new();
    for component in path.components() {
        if !shown.is_empty() {
            shown.push('/');
        }
        shown.push_str(&component.as_os_str().to_string_lossy());
    }
    shown
}

/// `range` is the range within `file`, when known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub file: PathBuf,
    pub range: Option<TextRange>,
    pub kind: DiagnosticKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticKind {
    /// The file name (without extension) cannot be converted to a type name.
    InvalidFileName { name: String, error: NameError },
    /// The directory name cannot be converted to a type name. Files under it do not become
    /// modules.
    InvalidDirName { name: String, error: NameError },
    /// A file claims a reserved module name (`Prelude`).
    ReservedModuleName { name: ModuleName },
    /// Two files map to the same module name. `first` is the one kept in the module map.
    DuplicateModule { name: ModuleName, first: PathBuf },
    /// The imported module does not exist.
    UndefinedModule { name: ModuleName },
    /// E2010. A module of the source root imports a module of the test root or a
    /// `[dev-dependencies]` Pome, which only the test root may import (spec 4.7, 13.1).
    TestOnlyModule { name: ModuleName },
    /// `modules` goes around the cycle once (the edge back to the first module is omitted).
    ImportCycle { modules: Vec<ModuleName> },
    /// A directory could not be read.
    Io { message: SmolStr },
    /// E2101. `expected` is the position the name was looked up in.
    UndefinedName {
        name: SmolStr,
        expected: Expected,
        suggestion: Option<SmolStr>,
    },
    /// A module, trait or effect has no item with that name (E2102). Used for import lists,
    /// `Module.f`, `Trait.f`, handler operations and impl functions.
    UndefinedMember {
        name: SmolStr,
        owner: SmolStr,
        owner_kind: &'static str,
        suggestion: Option<SmolStr>,
    },
    /// A duplicate in the same namespace (E2103). `first` is the earlier definition or import in
    /// the same file.
    Duplicate {
        name: SmolStr,
        first: Option<TextRange>,
    },
    /// A non-pub definition imported or referenced qualified from another module (E2104).
    Private { name: SmolStr, module: ModuleName },
    /// An opaque type constructed outside its defining module (E2105).
    OpaqueConstruction { name: SmolStr, module: ModuleName },
    /// An opaque type used in a pattern outside its defining module (E2105).
    OpaquePattern { name: SmolStr, module: ModuleName },
    /// The name was found, but its kind cannot be written in that position (E2106).
    WrongKind {
        name: SmolStr,
        expected: Expected,
        found: &'static str,
    },
    /// The name left of `.` refers to both a module and a trait (E2107).
    Ambiguous { name: SmolStr },
    /// `self` or `Self` where it cannot be used (E2108).
    SelfOutside { self_type: bool },
    /// `Self` as an expression outside a handler (E2108, `[grammar.expr]`). Inside a handler it
    /// names the handler's constructor, and outside its init that is E4005.
    SelfValueOutside,
    /// `use E` written as an expression, which is deprecated (W4401, `[effect.use]`). `rewrite`
    /// is the effect alias that means the same, when there is one.
    DeprecatedUse {
        effect: SmolStr,
        rewrite: Option<DeprecatedRewrite>,
    },
    /// An effect alias written other than as the receiver of `x.op(...)` or `x.op` (E4404,
    /// `[effect.use]`).
    EffectAliasAsValue { name: SmolStr },
    /// A handler member opened through an effect alias (`fs.Temporary()`, E4404,
    /// `[effect.use]`).
    MemberThroughEffectAlias {
        alias: SmolStr,
        effect: SmolStr,
        member: SmolStr,
    },
    /// `Self` as the target of a record update `Self(f: e, ..r)` (E2111, `[grammar.expr]`).
    SelfUpdate,
    /// An effect name in a type position (E2110). The type of a capability value cannot be
    /// written in source (spec 8.2).
    EffectAsType { name: SmolStr },
    /// The same name bound twice in one pattern or parameter list (E2109).
    DuplicateBinding {
        name: SmolStr,
        first: Option<TextRange>,
    },
    /// The target of a record update `C(f: e, ..r)` is not something with a single constructor
    /// (a type, an error, or a handler with fields) (E2111, spec 5.3). `found` is the kind of
    /// name that was found.
    InvalidUpdateTarget {
        name: Option<SmolStr>,
        found: &'static str,
    },
    /// A record update gives a changed field by position (E2112, spec 5.3).
    PositionalInUpdate,
    /// `@intrinsic` used outside the core sources bundled with the compiler (E2201).
    IntrinsicOutsideCore,
    /// `@intrinsic` on something other than an fn without a body (E2202).
    MisplacedIntrinsic,
    /// A type parameter hides an outer type of the same name (W2101, spec 2.3).
    ShadowedByTypeParam { name: SmolStr },
    /// A module definition or import hides a Prelude name of the same name (W2102).
    ShadowsPrelude { name: SmolStr },
    /// An impl head that is not a type name with distinct type parameters (E3303, spec 10.2):
    /// `impl List[Int]: Show`, `impl Pair[A, A]: Eq`. `head` is the head as written.
    ImplHead { head: SmolStr },
    /// In the module of a type, an fn whose first parameter has that type at its head has the
    /// name of one of its fields (E3109, `[data.type]`). A method call would find both.
    FnNamedLikeField { name: SmolStr, ty: SmolStr },
}

/// Where a name was looked up. Used in the diagnostic message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Expected {
    /// Lowercase and uppercase names in expressions (variables, functions, consts).
    Value,
    /// Type names in expressions and patterns (constructors).
    Constructor,
    Type,
    /// Uppercase names in type positions.
    TypeParam,
    /// Left of `.`.
    ModuleOrTrait,
    /// `use X`, `handler H: X`, the `uses` clause.
    Effect,
    /// The contents of a layer.
    HandlerOrLayer,
    /// `with X`: a handler, a layer, or a label. Any effect fits here: whether it is a label is
    /// left to type checking (E4202).
    WithItem,
    /// The left of `by` in `with L by P`.
    Label,
    /// The right of `by` in `with L by P`.
    HandlerOrEffect,
    /// `fail e`, escape arms, the `fails` clause.
    Error,
    /// Constraints, derive, impl.
    Trait,
    /// Uppercase names in patterns.
    Const,
}

impl Expected {
    pub fn describe(self) -> &'static str {
        match self {
            Expected::Value => "value",
            Expected::Constructor => "constructor",
            Expected::Type => "type",
            Expected::TypeParam => "type parameter",
            Expected::ModuleOrTrait => "module or trait",
            Expected::Effect => "effect",
            Expected::HandlerOrLayer => "handler or layer",
            Expected::WithItem => "handler, layer or label",
            Expected::Label => "label",
            Expected::HandlerOrEffect => "handler or effect",
            Expected::Error => "error",
            Expected::Trait => "trait",
            Expected::Const => "constant",
        }
    }
}

/// How a deprecated `use E` can be rewritten to an effect alias with the same meaning
/// ([`DiagnosticKind::DeprecatedUse`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeprecatedRewrite {
    /// `(use E).op(...)`: `alias <- E` before the statement, and `alias.op(...)` in place.
    InPlace { alias: SmolStr, op: SmolStr },
    /// `x = use E`, where x is only a receiver: `x <- E`.
    Binding { alias: SmolStr },
}

impl DiagnosticKind {
    /// Errors otherwise.
    pub fn is_warning(&self) -> bool {
        matches!(
            self,
            DiagnosticKind::ShadowedByTypeParam { .. }
                | DiagnosticKind::ShadowsPrelude { .. }
                | DiagnosticKind::DeprecatedUse { .. }
        )
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
    }
}

fn suggest(f: &mut fmt::Formatter<'_>, suggestion: &Option<SmolStr>) -> fmt::Result {
    match suggestion {
        Some(s) => write!(f, " (did you mean `{s}`?)"),
        None => Ok(()),
    }
}

impl fmt::Display for DiagnosticKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DiagnosticKind::InvalidFileName { name, error } => {
                write!(f, "file name `{name}` cannot be a module name: {error}")
            }
            DiagnosticKind::InvalidDirName { name, error } => {
                write!(
                    f,
                    "directory name `{name}` cannot be a module name: {error}"
                )
            }
            DiagnosticKind::ReservedModuleName { name } => {
                write!(f, "module name `{name}` is reserved")
            }
            DiagnosticKind::DuplicateModule { name, first } => write!(
                f,
                "duplicate module `{name}` (already defined by `{}`)",
                shown_path(first)
            ),
            DiagnosticKind::UndefinedModule { name } => {
                write!(f, "undefined module `{name}`")
            }
            DiagnosticKind::TestOnlyModule { name } => {
                write!(f, "module `{name}` can only be imported under `test/`")
            }
            DiagnosticKind::ImportCycle { modules } => {
                if let [only] = modules.as_slice() {
                    return write!(f, "module `{only}` imports itself");
                }
                f.write_str("import cycle: ")?;
                for module in modules {
                    write!(f, "{module} → ")?;
                }
                write!(f, "{}", modules[0])
            }
            DiagnosticKind::Io { message } => write!(f, "cannot read directory: {message}"),
            DiagnosticKind::UndefinedName {
                name,
                expected,
                suggestion,
            } => {
                write!(f, "undefined {} `{name}`", expected.describe())?;
                suggest(f, suggestion)
            }
            DiagnosticKind::UndefinedMember {
                name,
                owner,
                suggestion,
                ..
            } => {
                write!(f, "`{owner}` has no `{name}`")?;
                suggest(f, suggestion)
            }
            DiagnosticKind::Duplicate { name, .. } => write!(f, "duplicate `{name}`"),
            DiagnosticKind::Private { name, module } => {
                write!(f, "`{name}` is private to module `{module}`")
            }
            DiagnosticKind::OpaqueConstruction { name, module } => {
                write!(
                    f,
                    "cannot construct opaque `{name}` outside module `{module}`"
                )
            }
            DiagnosticKind::OpaquePattern { name, module } => write!(
                f,
                "cannot match on opaque `{name}` outside module `{module}`"
            ),
            DiagnosticKind::WrongKind {
                name,
                expected,
                found,
            } => write!(
                f,
                "expected {}, found {found} `{name}`",
                expected.describe()
            ),
            DiagnosticKind::Ambiguous { name } => {
                write!(f, "`{name}` refers to both a module and a trait")
            }
            DiagnosticKind::SelfOutside { self_type: false } => {
                f.write_str("`self` can only be used in handlers, impls, and traits")
            }
            DiagnosticKind::SelfOutside { self_type: true } => {
                f.write_str("`Self` can only be used in traits and impls")
            }
            DiagnosticKind::SelfValueOutside => {
                f.write_str("`Self` can only be used as a value in the `init` of a handler")
            }
            DiagnosticKind::SelfUpdate => f.write_str("cannot update `Self` with `..`"),
            DiagnosticKind::DeprecatedUse { effect, .. } => {
                write!(f, "`use {effect}` as an expression is deprecated")
            }
            DiagnosticKind::EffectAliasAsValue { name } => write!(
                f,
                "`{name}` is an effect alias, not a value: only `{name}.op(...)` and `{name}.op` can be written"
            ),
            DiagnosticKind::MemberThroughEffectAlias { alias, member, .. } => write!(
                f,
                "handler member `{member}` cannot be opened through effect alias `{alias}`"
            ),
            DiagnosticKind::EffectAsType { name } => {
                write!(f, "effect `{name}` cannot be used as a type")
            }
            DiagnosticKind::DuplicateBinding { name, .. } => {
                write!(f, "`{name}` is bound twice")
            }
            DiagnosticKind::InvalidUpdateTarget { name, found } => match name {
                Some(name) => write!(f, "{found} `{name}` cannot be updated"),
                None => write!(f, "{found} cannot be updated"),
            },
            DiagnosticKind::PositionalInUpdate => f.write_str("fields to update must be named"),
            DiagnosticKind::IntrinsicOutsideCore => {
                f.write_str("`@intrinsic` can only be used in the bundled core sources")
            }
            DiagnosticKind::MisplacedIntrinsic => {
                f.write_str("`@intrinsic` can only be used on a function without a body")
            }
            DiagnosticKind::ShadowedByTypeParam { name } => {
                write!(f, "type parameter `{name}` shadows an outer definition")
            }
            DiagnosticKind::ShadowsPrelude { name } => {
                write!(f, "`{name}` shadows the prelude's `{name}`")
            }
            DiagnosticKind::ImplHead { head } => write!(
                f,
                "impl head must be a type with distinct type parameters: `{head}`"
            ),
            DiagnosticKind::FnNamedLikeField { name, ty } => write!(
                f,
                "function `{name}` has the same name as a field of `{ty}`"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shown_path_separates_relative_paths_with_slash() {
        let nested = Path::new("src").join("http").join("client.emel");
        assert_eq!(shown_path(&nested), "src/http/client.emel");
        assert_eq!(shown_path(Path::new("main.emel")), "main.emel");
        assert_eq!(
            shown_path(&Path::new("..").join("src").join("a.emel")),
            "../src/a.emel"
        );
    }

    #[cfg(windows)]
    #[test]
    fn shown_path_replaces_the_windows_separator() {
        assert_eq!(
            shown_path(Path::new(r"src\http\client.emel")),
            "src/http/client.emel"
        );
        // An absolute path is outside the project and keeps the separators of the OS.
        assert_eq!(
            shown_path(Path::new(r"C:\p\src\a.emel")),
            r"C:\p\src\a.emel"
        );
    }
}
