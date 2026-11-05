//! Name-resolution diagnostics. Resolution does not stop at the first one; all are collected and
//! returned together.

use std::fmt;
use std::path::PathBuf;

use rowan::TextRange;
use smol_str::SmolStr;

use crate::ModuleName;
use crate::naming::NameError;

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
    /// E0239. A module of the source root imports a module of the test root or a
    /// `[dev-dependencies]` Pome, which only the test root may import (spec 4.7, 13.1).
    TestOnlyModule { name: ModuleName },
    /// `modules` goes around the cycle once (the edge back to the first module is omitted).
    ImportCycle { modules: Vec<ModuleName> },
    /// A directory could not be read.
    Io { message: SmolStr },
    /// E0211. `expected` is the position the name was looked up in.
    UndefinedName {
        name: SmolStr,
        expected: Expected,
        suggestion: Option<SmolStr>,
    },
    /// A module, trait or effect has no item with that name (E0212). Used for import lists,
    /// `Module.f`, `Trait.f`, handler operations and impl functions.
    UndefinedMember {
        name: SmolStr,
        owner: SmolStr,
        owner_kind: &'static str,
        suggestion: Option<SmolStr>,
    },
    /// A duplicate in the same namespace (E0213). `first` is the earlier definition or import in
    /// the same file.
    Duplicate {
        name: SmolStr,
        first: Option<TextRange>,
    },
    /// A non-pub definition imported or referenced qualified from another module (E0214).
    Private { name: SmolStr, module: ModuleName },
    /// An opaque type constructed outside its defining module (E0215).
    OpaqueConstruction { name: SmolStr, module: ModuleName },
    /// An opaque type used in a pattern outside its defining module (E0216).
    OpaquePattern { name: SmolStr, module: ModuleName },
    /// The name was found, but its kind cannot be written in that position (E0217).
    WrongKind {
        name: SmolStr,
        expected: Expected,
        found: &'static str,
    },
    /// The name left of `.` refers to both a module and a trait (E0218).
    Ambiguous { name: SmolStr },
    /// `self` or `Self` where it cannot be used (E0219).
    SelfOutside { self_type: bool },
    /// An effect name in a type position (E0221). The type of a capability value cannot be
    /// written in source (spec 8.2).
    EffectAsType { name: SmolStr },
    /// The same name bound twice in one pattern or parameter list (E0220).
    DuplicateBinding {
        name: SmolStr,
        first: Option<TextRange>,
    },
    /// The target of a record update `C(f: e, ..r)` is not something with a single constructor
    /// (a type, an error, or a handler with fields) (E0222, spec 5.3). `found` is the kind of
    /// name that was found.
    InvalidUpdateTarget {
        name: Option<SmolStr>,
        found: &'static str,
    },
    /// A record update gives a changed field by position (E0223, spec 5.3).
    PositionalInUpdate,
    /// `@intrinsic` used outside the core sources bundled with the compiler (E0224).
    IntrinsicOutsideCore,
    /// `@intrinsic` on something other than an fn without a body (E0225).
    MisplacedIntrinsic,
    /// A type parameter hides an outer type of the same name (W0201, spec 2.3).
    ShadowedByTypeParam { name: SmolStr },
    /// A module definition or import hides a Prelude name of the same name (W0202).
    ShadowsPrelude { name: SmolStr },
    /// An impl head that is not a type name with distinct type parameters (E0328, spec 10.2):
    /// `impl List[Int]: Show`, `impl Pair[A, A]: Eq`. `head` is the head as written.
    ImplHead { head: SmolStr },
    /// In the module of a type, an fn whose first parameter has that type at its head has the
    /// name of one of its fields (E0334, `[data.type]`). A method call would find both.
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
    /// `use X`, `handler H: X`, the `use` clause.
    Effect,
    /// The contents of a layer.
    HandlerOrLayer,
    /// `with X`: a handler, a layer, or a label. Any effect fits here: whether it is a label is
    /// left to type checking (E0430).
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

impl DiagnosticKind {
    /// Errors otherwise.
    pub fn is_warning(&self) -> bool {
        matches!(
            self,
            DiagnosticKind::ShadowedByTypeParam { .. } | DiagnosticKind::ShadowsPrelude { .. }
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
                first.display()
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
