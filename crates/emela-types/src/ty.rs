//! Type representation.
//!
//! Types are nominal. Types declared with `type` and `enum` are registered in `TyCons` and
//! referred to by `TyConId`. Primitive types, `List`, tuples and functions are structural.

use std::ops::Index;

use emela_resolve::hir::DefId;
use ena::unify::{NoError, UnifyKey, UnifyValue};
use la_arena::{Arena, ArenaMap, Idx};
use smol_str::SmolStr;

pub use crate::row::{Row, RowElem, RowVar, SetKind, Tag};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Prim {
    /// 32-bit.
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

/// Displayed as `?0`.
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

/// `None` means unbound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TyVarValue(pub(crate) Option<Ty>);

impl UnifyValue for TyVarValue {
    type Error = NoError;

    fn unify_values(a: &Self, b: &Self) -> Result<Self, NoError> {
        // Never unioned with both sides bound: `InferCtx` unifies the structures first.
        Ok(match (&a.0, &b.0) {
            (Some(_), _) => a.clone(),
            (None, _) => b.clone(),
        })
    }
}

pub type TyConId = Idx<TyConData>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TyConKind {
    /// A record.
    Type,
    Enum,
    /// `error` (spec 7.1). Also defines a type of the same name.
    Error,
    /// A handler value (spec 8.4). Cannot be written in source; appears only as the value of a
    /// handler's `init` and as the type of `self`.
    Handler,
}

/// There is no syntax for kinds; the kind is determined by where the parameter is used
/// (spec 9.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParamKind {
    /// An ordinary type, referred to by `Ty::Bound(i)`. If it is also written in `fails`, it is
    /// the type of a single error (the `E` of `or_fail`, spec 7.2) and appears in sets as
    /// `RowElem::Of(Ty::Bound(i))`.
    Type,
    /// A set, referred to by `RowElem::Bound(i)`. Whether it is an error set (the `E` of
    /// `fails E`) or an effect set (the `R` of `use R`) depends on where it is written. Includes
    /// the invisible set variables of function-typed parameters.
    Set,
}

/// A type parameter and its constraints (`A: Ord + Show`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TyParam {
    pub name: SmolStr,
    /// The names of the constraints, for display: the traits and `Immediate`.
    pub bounds: Vec<SmolStr>,
    /// The trait constraints, checked where the parameter is instantiated (E0326, spec 10.3).
    /// Supertraits are not added; they are followed when a constraint is looked up.
    pub traits: Vec<DefId>,
    pub kind: ParamKind,
}

impl TyParam {
    pub fn new(name: impl Into<SmolStr>) -> Self {
        TyParam {
            name: name.into(),
            bounds: Vec::new(),
            traits: Vec::new(),
            kind: ParamKind::Type,
        }
    }

    /// A set (error or effect) type parameter.
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TyConData {
    pub name: SmolStr,
    pub kind: TyConKind,
    pub params: Vec<TyParam>,
}

/// Also holds the names of set tags (error and effect definitions), for display.
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

    pub fn set_tag_name(&mut self, def: DefId, name: SmolStr) {
        self.tags.insert(def, name);
    }

    /// `None` if not registered.
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

/// `fn(A, B) -> C fails E use R`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FnTy {
    pub params: Vec<Ty>,
    pub ret: Ty,
    /// The error set E (spec chapter 7, 9.2). Empty means the function cannot fail.
    pub fails: Row,
    /// The effect set R (spec chapter 8, 9.2). Tags are effect definitions. Empty means the
    /// function does not depend on its environment.
    pub uses: Row,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Ty {
    Prim(Prim),
    /// The type with no values. Joins with any type.
    Never,
    Var(TyVar),
    /// A type parameter quantified by a type scheme; an index into `Scheme::params`.
    Bound(u32),
    Fn(Box<FnTy>),
    /// At least two elements.
    Tuple(Vec<Ty>),
    List(Box<Ty>),
    /// An application of a named type (`Pair[A, B]`, `Option[A]`).
    Named(TyConId, Vec<Ty>),
    /// A set passed as a set type argument of a named type (the `E` and `R` of `Handler[E, R]`,
    /// spec 9.2). Appears only in type argument positions of named types; not a value type.
    Set(SetKind, Row),
    /// The type of an effect's capability value (spec 8.2), returned by `use X`. Holds the
    /// effect definition and its type arguments (`use State[Int]`, spec 8.1). Cannot be written in
    /// source, and cannot be passed as an argument, stored in a data structure or returned.
    Cap(Tag),
    /// A broken type, made from a type that could not be read or a name that could not be
    /// resolved. Unification silently accepts it against any type so errors do not cascade.
    Error,
}

impl Ty {
    pub const INT: Ty = Ty::Prim(Prim::Int);
    pub const INT64: Ty = Ty::Prim(Prim::Int64);
    pub const FLOAT: Ty = Ty::Prim(Prim::Float);
    pub const BOOL: Ty = Ty::Prim(Prim::Bool);
    pub const STRING: Ty = Ty::Prim(Prim::String);
    pub const UNIT: Ty = Ty::Prim(Prim::Unit);

    pub fn func(params: impl IntoIterator<Item = Ty>, ret: Ty) -> Ty {
        Ty::func_fails(params, ret, Row::empty())
    }

    pub fn func_fails(params: impl IntoIterator<Item = Ty>, ret: Ty, fails: Row) -> Ty {
        Ty::func_sets(params, ret, fails, Row::empty())
    }

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
        debug_assert!(elems.len() >= 2, "a tuple has at least two elements");
        Ty::Tuple(elems)
    }

    pub fn list(elem: Ty) -> Ty {
        Ty::List(Box::new(elem))
    }

    pub fn named(id: TyConId, args: impl IntoIterator<Item = Ty>) -> Ty {
        Ty::Named(id, args.into_iter().collect())
    }

    /// Visits the immediate child types, including types inside a function type's error set
    /// (`RowElem::Of`) and type arguments of effect-set tags (the `Int` of `State[Int]`).
    pub fn for_each_child(&self, mut f: impl FnMut(&Ty)) {
        match self {
            Ty::Prim(_) | Ty::Never | Ty::Var(_) | Ty::Bound(_) | Ty::Error => {}
            Ty::Cap(tag) => tag.args.iter().for_each(f),
            Ty::Fn(fun) => {
                fun.params.iter().for_each(&mut f);
                f(&fun.ret);
                fun.fails.for_each_ty(&mut f);
                fun.uses.for_each_ty(f);
            }
            Ty::Set(_, row) => row.for_each_ty(f),
            Ty::Tuple(elems) | Ty::Named(_, elems) => elems.iter().for_each(f),
            Ty::List(elem) => f(elem),
        }
    }

    /// Replaces the immediate child types with `f`, including types inside a function type's
    /// error set and type arguments of effect-set tags.
    pub fn map_children(&self, mut f: impl FnMut(&Ty) -> Ty) -> Ty {
        match self {
            Ty::Prim(_) | Ty::Never | Ty::Var(_) | Ty::Bound(_) | Ty::Error => self.clone(),
            Ty::Cap(tag) => Ty::Cap(tag.map_args(f)),
            Ty::Fn(fun) => Ty::Fn(Box::new(FnTy {
                params: fun.params.iter().map(&mut f).collect(),
                ret: f(&fun.ret),
                fails: fun.fails.map_tys(&mut f),
                uses: fun.uses.map_tys(&mut f),
            })),
            Ty::Set(kind, row) => Ty::Set(*kind, row.map_tys(f)),
            Ty::Tuple(elems) => Ty::Tuple(elems.iter().map(f).collect()),
            Ty::Named(id, args) => Ty::Named(*id, args.iter().map(f).collect()),
            Ty::List(elem) => Ty::List(Box::new(f(elem))),
        }
    }
}
