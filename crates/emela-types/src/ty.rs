//! 型の表現．
//!
//! 型は名前で区別する（nominal）．`type` と `enum` で宣言した型は `TyCons` に登録し，
//! `TyConId` で指す．基本型と `List` とタプルと関数は構造で持つ．

use std::ops::Index;

use ena::unify::{NoError, UnifyKey, UnifyValue};
use la_arena::{Arena, Idx};
use smol_str::SmolStr;

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
}

/// 型引数とその制約（`A: Ord + Show`）．
///
/// Trait は 0.21 以降なので，制約は名前を保持するだけで検査しない．
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TyParam {
    pub name: SmolStr,
    pub bounds: Vec<SmolStr>,
}

impl TyParam {
    pub fn new(name: impl Into<SmolStr>) -> Self {
        TyParam {
            name: name.into(),
            bounds: Vec::new(),
        }
    }

    pub fn with_bounds(
        name: impl Into<SmolStr>,
        bounds: impl IntoIterator<Item = SmolStr>,
    ) -> Self {
        TyParam {
            name: name.into(),
            bounds: bounds.into_iter().collect(),
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

/// 名前付き型の表．
#[derive(Clone, Debug, Default)]
pub struct TyCons {
    arena: Arena<TyConData>,
}

impl TyCons {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn alloc(&mut self, data: TyConData) -> TyConId {
        self.arena.alloc(data)
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

/// 関数型 `fn(A, B) -> C`．
///
/// `fails E` と `use R` はまだ持たない．足すときはこの構造体にフィールドを加える
/// （`Ty::Fn` の形は変えない）．
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FnTy {
    pub params: Vec<Ty>,
    pub ret: Ty,
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
}

impl Ty {
    pub const INT: Ty = Ty::Prim(Prim::Int);
    pub const INT64: Ty = Ty::Prim(Prim::Int64);
    pub const FLOAT: Ty = Ty::Prim(Prim::Float);
    pub const BOOL: Ty = Ty::Prim(Prim::Bool);
    pub const STRING: Ty = Ty::Prim(Prim::String);
    pub const UNIT: Ty = Ty::Prim(Prim::Unit);

    pub fn func(params: impl IntoIterator<Item = Ty>, ret: Ty) -> Ty {
        Ty::Fn(Box::new(FnTy {
            params: params.into_iter().collect(),
            ret,
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

    /// 直下の子の型を順に渡す．
    pub fn for_each_child(&self, mut f: impl FnMut(&Ty)) {
        match self {
            Ty::Prim(_) | Ty::Never | Ty::Var(_) | Ty::Bound(_) => {}
            Ty::Fn(fun) => {
                fun.params.iter().for_each(&mut f);
                f(&fun.ret);
            }
            Ty::Tuple(elems) | Ty::Named(_, elems) => elems.iter().for_each(f),
            Ty::List(elem) => f(elem),
        }
    }

    /// 直下の子の型を `f` で置き換えた型を作る．
    pub fn map_children(&self, mut f: impl FnMut(&Ty) -> Ty) -> Ty {
        match self {
            Ty::Prim(_) | Ty::Never | Ty::Var(_) | Ty::Bound(_) => self.clone(),
            Ty::Fn(fun) => Ty::Fn(Box::new(FnTy {
                params: fun.params.iter().map(&mut f).collect(),
                ret: f(&fun.ret),
            })),
            Ty::Tuple(elems) => Ty::Tuple(elems.iter().map(f).collect()),
            Ty::Named(id, args) => Ty::Named(*id, args.iter().map(f).collect()),
            Ty::List(elem) => Ty::List(Box::new(f(elem))),
        }
    }
}
