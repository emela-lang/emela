//! 名前解決の診断．止まらずに全部集めて，呼び出し側へまとめて返す．

use std::fmt;
use std::path::PathBuf;

use rowan::TextRange;
use smol_str::SmolStr;

use crate::ModuleName;
use crate::naming::NameError;

/// 1件の診断．位置はファイルのパスと，分かるときはその中の範囲．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub file: PathBuf,
    pub range: Option<TextRange>,
    pub kind: DiagnosticKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticKind {
    /// ファイル名（拡張子を除く）が型名へ変換できない．
    InvalidFileName { name: String, error: NameError },
    /// ディレクトリ名が型名へ変換できない．その下のファイルはモジュールにしない．
    InvalidDirName { name: String, error: NameError },
    /// 予約されたモジュール名（`Prelude`）をファイルが名乗っている．
    ReservedModuleName { name: ModuleName },
    /// 別のファイルが同じモジュール名になる．`first` が対応表に残った方．
    DuplicateModule { name: ModuleName, first: PathBuf },
    /// import 先のモジュールがない．
    UndefinedModule { name: ModuleName },
    /// import の循環．`modules` は循環を1周する列（最初のモジュールへ戻る辺は省く）．
    ImportCycle { modules: Vec<ModuleName> },
    /// ディレクトリを読めなかった．
    Io { message: SmolStr },
    /// 未定義の名前（E0211）．`expected` はどの位置で引いたか．
    UndefinedName {
        name: SmolStr,
        expected: Expected,
        suggestion: Option<SmolStr>,
    },
    /// モジュール，Trait，エフェクトに，その名前の項目がない（E0212）．
    /// import の一覧，`Module.f`，`Trait.f`，handler の操作，impl の関数で使う．
    UndefinedMember {
        name: SmolStr,
        owner: SmolStr,
        owner_kind: &'static str,
        suggestion: Option<SmolStr>,
    },
    /// 同じ名前空間での重複（E0213）．`first` は同じファイルの先の定義か import．
    Duplicate {
        name: SmolStr,
        first: Option<TextRange>,
    },
    /// pub でない定義を，他のモジュールから import するか修飾して参照した（E0214）．
    Private { name: SmolStr, module: ModuleName },
    /// opaque な型を定義モジュールの外で構築した（E0215）．
    OpaqueConstruction { name: SmolStr, module: ModuleName },
    /// opaque な型を定義モジュールの外でパターンに書いた（E0216）．
    OpaquePattern { name: SmolStr, module: ModuleName },
    /// 名前は見つかったが，その位置に書けない種類だった（E0217）．
    WrongKind {
        name: SmolStr,
        expected: Expected,
        found: &'static str,
    },
    /// `.` の左の名前が，モジュールと Trait の両方を指す（E0218）．
    Ambiguous { name: SmolStr },
    /// `self` か `Self` を，使えない場所に書いた（E0219）．
    SelfOutside { self_type: bool },
    /// 1つのパターンか引数の並びで，同じ名前を2回束縛した（E0220）．
    DuplicateBinding {
        name: SmolStr,
        first: Option<TextRange>,
    },
    /// 型名の型引数が，外側の同じ名前の型を隠す（W0201，2.3）．
    ShadowedByTypeParam { name: SmolStr },
    /// モジュールの定義か import が，Prelude の同じ名前を隠す（W0202）．
    ShadowsPrelude { name: SmolStr },
}

/// 名前を引いた位置．診断の文面に使う．
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Expected {
    /// 式の小文字名と大文字名（変数，関数，const）．
    Value,
    /// 式とパターンの型名（構成子）．
    Constructor,
    /// 型の位置．
    Type,
    /// 型の位置の大文字名．
    TypeParam,
    /// `.` の左．
    ModuleOrTrait,
    /// `use X`，`implements X`，`use` 節．
    Effect,
    /// `with H`，layer の中身．
    HandlerOrLayer,
    /// `fail e`，escape の腕，`fails` 節．
    Error,
    /// 制約，derive，impl．
    Trait,
    /// パターンの大文字名．
    Const,
}

impl Expected {
    /// 英語の名前．
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
    /// 警告か．警告でなければエラー．
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
            DiagnosticKind::DuplicateBinding { name, .. } => {
                write!(f, "`{name}` を2回束縛している")
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
