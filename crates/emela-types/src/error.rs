//! 型の不一致の報告．
//!
//! 位置（span）はまだ持たない．構文木とつなぐときに呼び出し側が付ける．

use crate::ty::{Ty, TyVar};

/// 単一化の失敗．`expected` と `actual` は単一化に渡した型そのもの（束縛は辿った形）．
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeError {
    pub expected: Ty,
    pub actual: Ty,
    /// `Result` の `Err` を小さく保つため箱に入れる．
    pub kind: Box<TypeErrorKind>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeErrorKind {
    /// 食い違った部分．全体が食い違ったときは `TypeError` の `expected` / `actual` と同じ．
    Mismatch { expected: Ty, actual: Ty },
    /// `var` を `ty` に束縛すると無限型になる（出現検査）．
    InfiniteType { var: TyVar, ty: Ty },
}
