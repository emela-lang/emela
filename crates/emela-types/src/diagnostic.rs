//! 型推論の診断．単一化の失敗（[`crate::TypeError`]）には位置を付けてここに入れる．
//!
//! 文面とエラーコードは driver が決める．ここでは中身だけを持つ．

use emela_resolve::hir::Span;
use smol_str::SmolStr;

use crate::error::TypeError;
use crate::ty::Ty;

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
    /// 単一化の失敗（E0301 の型が合わない，E0302 の無限型）．
    Mismatch(TypeError),
    /// 呼び出しの引数の数が合わない（E0305）．`callee` は呼んだ関数か構成子の名前．
    /// 名前で指していない値（局所変数，ラムダ）を呼んだときは `None`．
    WrongArgCount {
        callee: Option<SmolStr>,
        expected: usize,
        found: usize,
    },
    /// そのラベルの引数がない（E0306）．
    UnknownLabel { callee: SmolStr, label: SmolStr },
    /// 同じ引数を2回渡した（E0307）．位置引数とラベルで同じ引数を指したときも含む．
    DuplicateArg { label: SmolStr },
    /// ラベルを持たない値（局所変数，ラムダ）の呼び出しで名前付き引数を書いた（E0306）．
    LabelOnUnnamedCallee { label: SmolStr },
    /// 構成子のパターンのフィールドの数が合わない（E0309）．多すぎる（`Rect(w, h, d)`），
    /// または位置だけのフィールドの構成子で `..` なしに足りない（`Rect(w)`）．
    PatternFieldCount {
        ctor: SmolStr,
        expected: usize,
        found: usize,
    },
    /// 名前付きフィールドの構成子のパターンで，`..` を書かずにフィールドを省いた（E0308，6.5）．
    /// `fields` は省いたフィールドの全部（宣言の順）．
    MissingPatternFields { ctor: SmolStr, fields: Vec<SmolStr> },
    /// 数の型でない（E0310）．`integer` なら Int か Int64 が要る（整数リテラル），
    /// そうでなければ Int，Int64，Float のどれかが要る（算術演算子，6.6）．
    NotNumeric { found: Ty, integer: bool },
    /// 整数リテラルが型の範囲を超える（E0311，2.4）．
    IntLiteralOutOfRange { literal: SmolStr, ty: Ty },
}
