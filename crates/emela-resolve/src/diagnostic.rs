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
    /// `use X`, `implements X`, the `use` clause.
    Effect,
    /// `with H`, and the contents of a layer.
    HandlerOrLayer,
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
            Expected::Error => "error",
            Expected::Trait => "trait",
            Expected::Const => "constant",
        }
    }

    fn describe_ja(self) -> &'static str {
        match self {
            Expected::Value => "値",
            Expected::Constructor => "構成子",
            Expected::Type => "型",
            Expected::TypeParam => "型引数",
            Expected::ModuleOrTrait => "モジュールか Trait",
            Expected::Effect => "エフェクト",
            Expected::HandlerOrLayer => "ハンドラか layer",
            Expected::Error => "エラー",
            Expected::Trait => "Trait",
            Expected::Const => "const",
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
        Some(s) => write!(f, "（もしかして `{s}`）"),
        None => Ok(()),
    }
}

impl fmt::Display for DiagnosticKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DiagnosticKind::InvalidFileName { name, error } => {
                write!(f, "ファイル名 `{name}` をモジュール名にできない: {error}")
            }
            DiagnosticKind::InvalidDirName { name, error } => {
                write!(
                    f,
                    "ディレクトリ名 `{name}` をモジュール名にできない: {error}"
                )
            }
            DiagnosticKind::ReservedModuleName { name } => {
                write!(f, "モジュール名 `{name}` は予約されている")
            }
            DiagnosticKind::DuplicateModule { name, first } => write!(
                f,
                "モジュール `{name}` が重複している（先に `{}` がある）",
                first.display()
            ),
            DiagnosticKind::UndefinedModule { name } => {
                write!(f, "未定義のモジュール `{name}`")
            }
            DiagnosticKind::ImportCycle { modules } => {
                if let [only] = modules.as_slice() {
                    return write!(f, "モジュール `{only}` が自分自身を import している");
                }
                f.write_str("import が循環している: ")?;
                for module in modules {
                    write!(f, "{module} → ")?;
                }
                write!(f, "{}", modules[0])
            }
            DiagnosticKind::Io { message } => write!(f, "ディレクトリを読めない: {message}"),
            DiagnosticKind::UndefinedName {
                name,
                expected,
                suggestion,
            } => {
                write!(f, "未定義の{} `{name}`", expected.describe_ja())?;
                suggest(f, suggestion)
            }
            DiagnosticKind::UndefinedMember {
                name,
                owner,
                suggestion,
                ..
            } => {
                write!(f, "`{owner}` に `{name}` がない")?;
                suggest(f, suggestion)
            }
            DiagnosticKind::Duplicate { name, .. } => write!(f, "`{name}` が重複している"),
            DiagnosticKind::Private { name, module } => {
                write!(f, "`{module}` の `{name}` は pub でない")
            }
            DiagnosticKind::OpaqueConstruction { name, module } => {
                write!(f, "opaque な `{name}` は `{module}` の外で構築できない")
            }
            DiagnosticKind::OpaquePattern { name, module } => write!(
                f,
                "opaque な `{name}` は `{module}` の外でパターンに書けない"
            ),
            DiagnosticKind::WrongKind {
                name,
                expected,
                found,
            } => write!(
                f,
                "`{name}` は {found} で，{}ではない",
                expected.describe_ja()
            ),
            DiagnosticKind::Ambiguous { name } => {
                write!(f, "`{name}` がモジュールと Trait の両方を指す")
            }
            DiagnosticKind::SelfOutside { self_type: false } => {
                f.write_str("`self` は handler，impl，trait の中でしか使えない")
            }
            DiagnosticKind::SelfOutside { self_type: true } => {
                f.write_str("`Self` は impl と trait の中でしか使えない")
            }
            DiagnosticKind::EffectAsType { name } => {
                write!(f, "エフェクト `{name}` は型として使えない")
            }
            DiagnosticKind::DuplicateBinding { name, .. } => {
                write!(f, "`{name}` を2回束縛している")
            }
            DiagnosticKind::InvalidUpdateTarget { name, found } => match name {
                Some(name) => write!(f, "`{name}` は {found} で，部分更新できない"),
                None => write!(f, "{found} は部分更新できない"),
            },
            DiagnosticKind::PositionalInUpdate => {
                f.write_str("部分更新では変えるフィールドを名前で書く")
            }
            DiagnosticKind::IntrinsicOutsideCore => {
                f.write_str("`@intrinsic` は同梱の core のソースの中でしか使えない")
            }
            DiagnosticKind::MisplacedIntrinsic => {
                f.write_str("`@intrinsic` は本体のない fn にしか付けられない")
            }
            DiagnosticKind::ShadowedByTypeParam { name } => {
                write!(f, "型引数 `{name}` が外側の同じ名前の定義を隠す")
            }
            DiagnosticKind::ShadowsPrelude { name } => {
                write!(f, "`{name}` が Prelude の同じ名前を隠す")
            }
        }
    }
}
