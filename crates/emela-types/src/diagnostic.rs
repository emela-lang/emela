//! 型推論の診断のうち，単一化の失敗（[`crate::TypeError`]）でないもの．
//!
//! 文面とエラーコードは driver が決める．ここでは中身だけを持つ．

use emela_resolve::hir::Span;
use smol_str::SmolStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Prelude の宣言の中の型はソースがないので `None`．
    pub span: Option<Span>,
    pub kind: DiagnosticKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticKind {
    /// 型引数の数が合わない（E0304）．`List[Int, Int]`，型引数を書いていない `Option`．
    WrongTypeArgCount {
        name: SmolStr,
        expected: usize,
        found: usize,
    },
    /// alpha.1 ではまだ扱えない構文．`what` は構文の名前（`Self` など）．
    Unsupported { what: &'static str },
}
