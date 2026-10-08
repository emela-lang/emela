//! 型スキーム `∀ A. T`．

use crate::ty::{Ty, TyParam};

/// 型スキーム．`ty` の中の `Ty::Bound(i)` が `params[i]` を指す．
///
/// 汎化するのはトップレベルの関数だけで，ブロック内の束縛は単相のまま扱う．
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scheme {
    pub params: Vec<TyParam>,
    pub ty: Ty,
}

impl Scheme {
    pub fn new(params: Vec<TyParam>, ty: Ty) -> Self {
        Scheme { params, ty }
    }

    /// 量化しないスキーム．
    pub fn mono(ty: Ty) -> Self {
        Scheme {
            params: Vec::new(),
            ty,
        }
    }

    pub fn is_mono(&self) -> bool {
        self.params.is_empty()
    }
}

/// 汎化で付ける型引数の名前．`A` から `Z`，その後は `T26`，`T27`，…
pub(crate) fn param_name(i: usize) -> String {
    if i < 26 {
        char::from(b'A' + i as u8).to_string()
    } else {
        format!("T{i}")
    }
}
