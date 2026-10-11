//! 集合の型．エラーの集合 E（7章）と効果の集合 R（8章）を同じ形で持つ．
//!
//! 集合は，タグ（error か effect の定義の [`DefId`]）と，中身の決まっていない集合の和で表す（9.4）．
//! 中身の決まっていない集合は3通りある．
//!
//! - 推論の集合変数（[`RowVar`]）．本体の推論が終わったところで，下界の和（最小の解）に決める．
//! - 型スキームの集合の型引数（`RowElem::Bound`）．`fails E` の `E` と，引数の関数型の
//!   見えない集合変数．定義の本体の中では中身の分からない集合として扱う．
//! - 1つのエラーの型（`RowElem::Of`）．`fail e` の `e` の型や，`or_fail` の `E` のように，
//!   値の型にも使う型引数．型が決まればそのタグになる．E にだけ現れる．

use emela_resolve::hir::DefId;

use crate::ty::Ty;

/// 推論の集合変数．表示は `?e0`．
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RowVar(pub(crate) u32);

impl RowVar {
    pub fn index(self) -> u32 {
        self.0
    }
}

/// 集合の要素．
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RowElem {
    /// タグ．E なら error の定義，R なら effect の定義．
    Tag(DefId),
    /// 型スキームの集合の型引数（[`crate::ParamKind::Set`]）．`Scheme::params` の添字．
    Bound(u32),
    /// エラーの型 `ty` のタグ．型が error の名前付き型に決まればそのタグになる．
    Of(Ty),
    /// 推論の集合変数．
    Var(RowVar),
}

/// 集合．要素の重複は持たない．順序は意味を持たないが，表示を安定させるため，
/// タグ，型引数，型，集合変数の順に並べる（タグと型引数と集合変数はその中で番号の順）．
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Row {
    elems: Vec<RowElem>,
}

impl Row {
    /// 空の集合．
    pub fn empty() -> Self {
        Row::default()
    }

    pub fn tag(def: DefId) -> Self {
        Row::from_elems([RowElem::Tag(def)])
    }

    pub fn var(v: RowVar) -> Self {
        Row::from_elems([RowElem::Var(v)])
    }

    pub fn from_elems(elems: impl IntoIterator<Item = RowElem>) -> Self {
        let mut row = Row::empty();
        for e in elems {
            row.insert(e);
        }
        row
    }

    pub fn is_empty(&self) -> bool {
        self.elems.is_empty()
    }

    pub fn elems(&self) -> &[RowElem] {
        &self.elems
    }

    pub fn contains(&self, elem: &RowElem) -> bool {
        self.elems.contains(elem)
    }

    /// 要素を足す．すでにあれば何もしない．
    pub fn insert(&mut self, elem: RowElem) {
        if self.elems.contains(&elem) {
            return;
        }
        let at = self
            .elems
            .iter()
            .position(|e| order(e) > order(&elem))
            .unwrap_or(self.elems.len());
        self.elems.insert(at, elem);
    }

    /// 和集合．
    pub fn union(&self, other: &Row) -> Row {
        let mut out = self.clone();
        out.extend(other);
        out
    }

    pub fn extend(&mut self, other: &Row) {
        for e in &other.elems {
            self.insert(e.clone());
        }
    }

    /// タグ．
    pub fn tags(&self) -> impl Iterator<Item = DefId> + '_ {
        self.elems.iter().filter_map(|e| match e {
            RowElem::Tag(t) => Some(*t),
            _ => None,
        })
    }

    /// 推論の集合変数．
    pub fn vars(&self) -> impl Iterator<Item = RowVar> + '_ {
        self.elems.iter().filter_map(|e| match e {
            RowElem::Var(v) => Some(*v),
            _ => None,
        })
    }

    /// 要素の中の型（`RowElem::Of`）を順に渡す．
    pub fn for_each_ty(&self, mut f: impl FnMut(&Ty)) {
        for e in &self.elems {
            if let RowElem::Of(ty) = e {
                f(ty);
            }
        }
    }

    /// 要素の中の型を `f` で置き換えた集合．
    pub fn map_tys(&self, mut f: impl FnMut(&Ty) -> Ty) -> Row {
        Row::from_elems(self.elems.iter().map(|e| match e {
            RowElem::Of(ty) => RowElem::Of(f(ty)),
            e => e.clone(),
        }))
    }

    /// 要素ごとに置き換えた集合．1つの要素を集合に広げてよい．
    pub fn flat_map(&self, mut f: impl FnMut(&RowElem) -> Row) -> Row {
        let mut out = Row::empty();
        for e in &self.elems {
            out.extend(&f(e));
        }
        out
    }
}

/// 並べる順の鍵．`Of` の中の型どうしは足した順のまま．
fn order(e: &RowElem) -> (u8, u32) {
    match e {
        RowElem::Tag(def) => (0, def.into_raw().into_u32()),
        RowElem::Bound(i) => (1, *i),
        RowElem::Of(_) => (2, 0),
        RowElem::Var(v) => (3, v.0),
    }
}
