//! 型の表現．
//!
//! 型は名前で区別する（nominal）．`type` と `enum` で宣言した型は `TyCons` に登録し，
//! `TyConId` で指す．基本型と `List` とタプルと関数は構造で持つ．

use std::ops::Index;

use emela_resolve::hir::DefId;
use ena::unify::{NoError, UnifyKey, UnifyValue};
use la_arena::{Arena, ArenaMap, Idx};
use smol_str::SmolStr;

pub use crate::row::{Row, RowElem, RowVar, SetKind};

/// 組み込みの基本型．
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Prim {
    /// 32bit 整数．
    Int,
    Int64,
    Float,
    Bool,
    String,
    /// `()`．
    Unit,
}

impl Prim {
    pub fn name(self) -> &'static str {
        match self {
            Prim::Int => "Int",
            Prim::Int64 => "Int64",
            Prim::Float => "Float",
            Prim::Bool => "Bool",
            Prim::String => "String",
            Prim::Unit => "()",
        }
    }
}

/// 単一化の型変数．表示は `?0`．
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TyVar(u32);

impl TyVar {
    pub fn index(self) -> u32 {
        self.0
    }
}

impl UnifyKey for TyVar {
    type Value = TyVarValue;

    fn index(&self) -> u32 {
        self.0
    }

    fn from_index(i: u32) -> Self {
        TyVar(i)
    }

    fn tag() -> &'static str {
        "TyVar"
    }
}

/// 型変数に束縛された型．`None` は未束縛．
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TyVarValue(pub(crate) Option<Ty>);

impl UnifyValue for TyVarValue {
    type Error = NoError;

    fn unify_values(a: &Self, b: &Self) -> Result<Self, NoError> {
        // 両方が束縛済みのまま union することはない（`InferCtx` が先に構造で単一化する）．
        Ok(match (&a.0, &b.0) {
            (Some(_), _) => a.clone(),
            (None, _) => b.clone(),
        })
    }
}

/// 名前付き型（`type` / `enum`）の宣言を指す ID．
pub type TyConId = Idx<TyConData>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TyConKind {
    /// `type`（レコード）．
    Type,
    Enum,
    /// `error`（7.1）．同じ名前の型も定義する．
    Error,
    /// handler の値（8.4）．ソースには書けず，handler の init の値と `self` の型にだけ現れる．
    Handler,
}

/// 型引数の種類．種を書く構文はなく，使われた位置から決める（9.2）．
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParamKind {
    /// ふつうの型．`Ty::Bound(i)` で指す．`fails` にも書かれていれば，1つのエラーの型
    /// （7.2 の `or_fail` の `E`）で，集合の中では `RowElem::Of(Ty::Bound(i))` になる．
    Type,
    /// 集合．`RowElem::Bound(i)` で指す．エラーの集合（`fails E` の `E`）か効果の集合
    /// （`use R` の `R`）かは，書いた位置で決まる．引数の関数型の見えない集合変数も含む．
    Set,
}

/// 型引数とその制約（`A: Ord + Show`）．
///
/// Trait は 0.21 以降なので，制約は名前を保持するだけで検査しない．
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TyParam {
    pub name: SmolStr,
    pub bounds: Vec<SmolStr>,
    pub kind: ParamKind,
}

impl TyParam {
    pub fn new(name: impl Into<SmolStr>) -> Self {
        TyParam {
            name: name.into(),
            bounds: Vec::new(),
            kind: ParamKind::Type,
        }
    }

    /// 集合（エラーか効果）の型引数．
    pub fn set(name: impl Into<SmolStr>) -> Self {
        TyParam {
            kind: ParamKind::Set,
            ..TyParam::new(name)
        }
    }

    pub fn with_bounds(
        name: impl Into<SmolStr>,
        bounds: impl IntoIterator<Item = SmolStr>,
    ) -> Self {
        TyParam {
            bounds: bounds.into_iter().collect(),
            ..TyParam::new(name)
        }
    }
}

/// 名前付き型の宣言．
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TyConData {
    pub name: SmolStr,
    pub kind: TyConKind,
    pub params: Vec<TyParam>,
}

/// 名前付き型の表．集合のタグ（error と effect の定義）の名前も持つ（表示に使う）．
#[derive(Clone, Debug, Default)]
pub struct TyCons {
    arena: Arena<TyConData>,
    tags: ArenaMap<DefId, SmolStr>,
}

impl TyCons {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn alloc(&mut self, data: TyConData) -> TyConId {
        self.arena.alloc(data)
    }

    /// 集合のタグ（error か effect の定義）の名前を登録する．
    pub fn set_tag_name(&mut self, def: DefId, name: SmolStr) {
        self.tags.insert(def, name);
    }

    /// 集合のタグの名前．登録していなければ `None`．
    pub fn tag_name(&self, def: DefId) -> Option<&SmolStr> {
        self.tags.get(def)
    }

    pub fn iter(&self) -> impl Iterator<Item = (TyConId, &TyConData)> {
        self.arena.iter()
    }
}

impl Index<TyConId> for TyCons {
    type Output = TyConData;

    fn index(&self, id: TyConId) -> &TyConData {
        &self.arena[id]
    }
}

/// 関数型 `fn(A, B) -> C fails E use R`．
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FnTy {
    pub params: Vec<Ty>,
    pub ret: Ty,
    /// エラーの集合 E（7章，9.2）．空なら失敗しない．
    pub fails: Row,
    /// 効果の集合 R（8章，9.2）．タグは effect の定義．空なら環境に依存しない．
    pub uses: Row,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Ty {
    Prim(Prim),
    /// 値を持たない型．どの型とも `join` できる．
    Never,
    /// 単一化の型変数．
    Var(TyVar),
    /// 型スキームが量化した型引数．`Scheme::params` の添字．
    Bound(u32),
    Fn(Box<FnTy>),
    /// 要素は2つ以上．
    Tuple(Vec<Ty>),
    List(Box<Ty>),
    /// 名前付き型の適用（`Pair[A, B]`，`Option[A]`）．
    Named(TyConId, Vec<Ty>),
    /// 名前付き型の集合の型引数に渡した集合（9.2 の `Handler[E, R]` の `E` と `R`）．
    /// 名前付き型の型引数の位置にだけ現れる．値の型ではない．
    Set(SetKind, Row),
    /// エフェクトの能力の値の型（8.2）．`use X` が返す．中身は effect の定義．
    /// ソースには書けず，引数に渡す，データ構造に入れる，返すことはできない．
    Cap(DefId),
    /// 壊れた型．読めなかった型や解決できなかった名前から作る．
    /// 単一化ではどの型とも黙って一致し，エラーを重ねて出さない．
    Error,
}

impl Ty {
    pub const INT: Ty = Ty::Prim(Prim::Int);
    pub const INT64: Ty = Ty::Prim(Prim::Int64);
    pub const FLOAT: Ty = Ty::Prim(Prim::Float);
    pub const BOOL: Ty = Ty::Prim(Prim::Bool);
    pub const STRING: Ty = Ty::Prim(Prim::String);
    pub const UNIT: Ty = Ty::Prim(Prim::Unit);

    /// 失敗しない関数型．
    pub fn func(params: impl IntoIterator<Item = Ty>, ret: Ty) -> Ty {
        Ty::func_fails(params, ret, Row::empty())
    }

    /// エラーの集合 `fails` を持ち，効果を持たない関数型．
    pub fn func_fails(params: impl IntoIterator<Item = Ty>, ret: Ty, fails: Row) -> Ty {
        Ty::func_sets(params, ret, fails, Row::empty())
    }

    /// エラーの集合 `fails` と効果の集合 `uses` を持つ関数型．
    pub fn func_sets(params: impl IntoIterator<Item = Ty>, ret: Ty, fails: Row, uses: Row) -> Ty {
        Ty::Fn(Box::new(FnTy {
            params: params.into_iter().collect(),
            ret,
            fails,
            uses,
        }))
    }

    pub fn tuple(elems: impl IntoIterator<Item = Ty>) -> Ty {
        let elems: Vec<Ty> = elems.into_iter().collect();
        debug_assert!(elems.len() >= 2, "タプルの要素は2つ以上");
        Ty::Tuple(elems)
    }

    pub fn list(elem: Ty) -> Ty {
        Ty::List(Box::new(elem))
    }

    pub fn named(id: TyConId, args: impl IntoIterator<Item = Ty>) -> Ty {
        Ty::Named(id, args.into_iter().collect())
    }

    /// 直下の子の型を順に渡す．関数型のエラーの集合の中の型（`RowElem::Of`）も含む．
    pub fn for_each_child(&self, mut f: impl FnMut(&Ty)) {
        match self {
            Ty::Prim(_) | Ty::Never | Ty::Var(_) | Ty::Bound(_) | Ty::Cap(_) | Ty::Error => {}
            Ty::Fn(fun) => {
                fun.params.iter().for_each(&mut f);
                f(&fun.ret);
                fun.fails.for_each_ty(f);
            }
            Ty::Set(_, row) => row.for_each_ty(f),
            Ty::Tuple(elems) | Ty::Named(_, elems) => elems.iter().for_each(f),
            Ty::List(elem) => f(elem),
        }
    }

    /// 直下の子の型を `f` で置き換えた型を作る．関数型のエラーの集合の中の型も置き換える．
    /// 効果の集合は型を含まないのでそのまま．
    pub fn map_children(&self, mut f: impl FnMut(&Ty) -> Ty) -> Ty {
        match self {
            Ty::Prim(_) | Ty::Never | Ty::Var(_) | Ty::Bound(_) | Ty::Cap(_) | Ty::Error => {
                self.clone()
            }
            Ty::Fn(fun) => Ty::Fn(Box::new(FnTy {
                params: fun.params.iter().map(&mut f).collect(),
                ret: f(&fun.ret),
                fails: fun.fails.map_tys(&mut f),
                uses: fun.uses.clone(),
            })),
            Ty::Set(kind, row) => Ty::Set(*kind, row.map_tys(f)),
            Ty::Tuple(elems) => Ty::Tuple(elems.iter().map(f).collect()),
            Ty::Named(id, args) => Ty::Named(*id, args.iter().map(f).collect()),
            Ty::List(elem) => Ty::List(Box::new(f(elem))),
        }
    }
}
