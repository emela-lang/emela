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
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
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
        }
    }
}
