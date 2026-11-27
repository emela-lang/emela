//! Type representation.
//!
//! Types are nominal. Types declared with `type` and `enum` are registered in `TyCons` and
//! referred to by `TyConId`. Primitive types, `List`, tuples and functions are structural.
//!
//! `List` could be a named type of the Prelude, but it is built in to keep display and
//! unification simple. The Prelude's `Bool` enum (`True` / `False`) is `Prim::Bool`, and its
//! constructors have type `Bool`; the spec's enum is the meaning users see, and the
//! representation is left to the implementation (`[target.unspecified]`).
//!
//! # Sets
//!
//! Error sets E (spec chapter 7) and effect sets R (spec chapter 8) share one representation.
//!
//! A set is a union of tags ([`Tag`]: an error definition, or an effect definition with its type
//! arguments) and sets whose contents are not known (spec 9.4). There are three kinds of the
//! latter:
//!
//! - Inference set variables ([`RowVar`]). Once a body's inference is done, each is fixed to the
//!   union of its lower bounds (the least solution).
//! - Set type parameters of a type scheme (`RowElem::Bound`): the `E` of `fails E`, and the
//!   invisible set variables of function-typed parameters. Inside the definition's body they are
//!   sets with unknown contents.
//! - The type of a single error (`RowElem::Of`): a type parameter also used as a value type, like
//!   the type of `e` in `fail e` or the `E` of `or_fail`. It becomes a tag once the type is known.
//!   Appears only in E.
//!
//! An effect set has one more kind of element: a capability ([`RowElem::Cap`], `[effect.use]`).
//! It stands for one handler after its init has run and before its release. It is not widened to
//! its effect: a set that has only the effect does not contain it (`[type.rules]`).
//!
//! A capability set ([`SetKind::Caps`], `[type.rules]`) is the set that a capability type
//! `use E` or a capability-bound type records. It shares the representation: its elements are
//! capabilities, the name of an effect (a [`RowElem::Tag`], for `use E` written as an expression
//! and for a value returned by an operation called through the name of its effect), set
//! variables, and set type parameters (the set of a capability parameter). Calling an operation
//! of a value adds its capability set to R.
//!
//! # Schemes
//!
//! Type schemes `∀ A. T`, and the substitution of their parameters.
//!
//! # Display
//!
//! Displays types the way the spec writes them (`List[Int]`, `fn(Int) -> Bool fails NotFound`,
//! `(Int, String)`).
//!
//! Names of named types and error tags live in `TyCons`, so displaying takes the table. Type
//! variables are shown as `?0` without following bindings; to show the resolved form, pass the
//! type through `InferCtx::resolve` first.
//!
//! An empty error set or effect set in a function type drops the whole `fails` or `uses` clause.
//! Inside a parameter type, however, dropping them would read as "anything", so they are shown
//! as `fails {}` and `uses {}` (spec 9.2). The set of a uses clause always has braces
//! (`uses { Logger }`, `[type.inference]`). When a function returns a function type and the outer
//! function type has a `fails` or `uses` clause, the return type is parenthesized so it is clear
//! which function the clause belongs to (spec 5.5).
//!
//! Capability value types (`Ty::Cap`) cannot be written in source, so they are shown as the
//! expression that creates them, `use Logger`. Effects with type arguments are shown as
//! `State[Int]`, both in sets and as capability values (spec 8.1).

use std::fmt;
use std::ops::Index;

use emela_resolve::hir::{DefId, ExprId};
use ena::unify::{NoError, UnifyKey, UnifyValue};
use la_arena::{Arena, ArenaMap, Idx};
use smol_str::SmolStr;

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
    /// `error` (spec 7.1). Registered here because it also defines a type of the same name
    /// (`[error.decl]`). Error sets hold errors by `DefId`; the type table maps between the two.
    Error,
    /// A handler value (spec 8.4). Cannot be written in source; appears only as the value of a
    /// handler's `init` and as the type of `self`. Takes no type arguments. A handler without
    /// fields uses the same type for `self` (accessing a field is E3106).
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
    /// `fails E`) or an effect set (the `R` of `uses { R }`) depends on where it is written. Includes
    /// the invisible set variables of function-typed parameters.
    Set,
}

/// The name of the built-in Immediate bound (spec 9.5).
pub(crate) const IMMEDIATE: &str = "Immediate";

/// The name of the built-in trait Capability (`[trait.builtin]`).
pub(crate) const CAPABILITY: &str = "Capability";

/// A type parameter and its constraints (`A: Ord + Show`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TyParam {
    pub name: SmolStr,
    /// The names of the constraints, for display: the traits and `Immediate`.
    pub bounds: Vec<SmolStr>,
    /// The trait constraints, checked where the parameter is instantiated (E3302, spec 10.3).
    /// Supertraits are not added; they are followed when a constraint is looked up.
    pub traits: Vec<DefId>,
    pub kind: ParamKind,
    /// For the set type parameter of a capability parameter (`C(db)`, `[type.rules]`): the
    /// effects of its capability type, which decide whether its operations may suspend.
    pub caps: Option<Vec<Tag>>,
}

impl TyParam {
    pub fn new(name: impl Into<SmolStr>) -> Self {
        TyParam {
            name: name.into(),
            bounds: Vec::new(),
            traits: Vec::new(),
            kind: ParamKind::Type,
            caps: None,
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

    /// The type parameters of `id`, for adding hidden ones once the table is built.
    pub(crate) fn params_mut(&mut self, id: TyConId) -> &mut Vec<TyParam> {
        &mut self.arena[id].params
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

/// `fn(A, B) -> C fails E uses { R }`.
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

impl FnTy {
    /// Whether the function type quantifies capability sets (`[type.rules]`): its R has them.
    pub fn quantifies(&self) -> bool {
        self.uses.caps().any(|c| matches!(c.id, CapId::Quant(_)))
    }
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
    /// `SetKind` decides how it is displayed (`A | B` or `{ A, B }`).
    ///
    /// Substitution, generalization and resolution after solving treat it like the E and R of a
    /// function type. Generalization counts set variables here as negative positions, since a type
    /// argument is used in both directions.
    Set(SetKind, Row),
    /// The type of a capability value (`[effect.use]`): `use E` returned by `use E`, or
    /// `use E1 + ... + En` for a handler that implements several effects. Holds each effect
    /// definition and its type arguments (`use State[Int]`, spec 8.1).
    ///
    /// Never empty. Build it with [`Ty::cap`], which puts the effects in the order of their
    /// definitions, so that `use A + B` and `use B + A` are the same type; effects with the same
    /// definition keep the order written. That order is also the order of the operation tables of
    /// a capability of several effects at run time.
    ///
    /// The row is the set of capabilities the value may be (`[type.rules]`, [`SetKind::Caps`]). It
    /// is not written in source and not shown in the type.
    Cap(Vec<Tag>, Row),
    /// A broken type, made from a type that could not be read or a name that could not be
    /// resolved. Unification silently accepts it against any type so errors do not cascade, and
    /// type variables unified with it are bound to it. Lowering never sees it: it is not called
    /// when there is any error.
    Error,
}

impl Ty {
    /// The type of a capability of `tags` (`use E1 + ... + En`), in the order of
    /// [`Ty::Cap`]. A tag that appears twice is kept once.
    pub fn cap(tags: impl IntoIterator<Item = Tag>, caps: Row) -> Ty {
        let mut out: Vec<Tag> = Vec::new();
        for t in tags {
            if !out.contains(&t) {
                out.push(t);
            }
        }
        out.sort_by_key(|t| t.def);
        Ty::Cap(out, caps)
    }

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

    /// For a function type that quantifies capability sets (`[type.rules]`): the type with each
    /// of them replaced by `with(i, element)`. Function types inside that quantify their own are
    /// left as they are.
    pub fn open_quant(&self, with: &mut dyn FnMut(u32, &CapElem) -> Row) -> Ty {
        let Ty::Fn(f) = self else {
            return self.clone();
        };
        if !f.quantifies() {
            return self.clone();
        }
        // One replacement per quantified set, wherever it occurs.
        let mut made: Vec<(u32, Row)> = Vec::new();
        let with = &mut |i: u32, c: &CapElem| -> Row {
            if let Some((_, r)) = made.iter().find(|(j, _)| *j == i) {
                return r.clone();
            }
            let r = with(i, c);
            made.push((i, r.clone()));
            r
        };
        let row = |r: &Row, with: &mut dyn FnMut(u32, &CapElem) -> Row| {
            r.flat_map(|e| match e {
                RowElem::Cap(c) => match c.id {
                    CapId::Quant(i) => with(i, c),
                    _ => Row::from_elems([e.clone()]),
                },
                e => Row::from_elems([e.clone()]),
            })
        };
        fn inner(ty: &Ty, with: &mut dyn FnMut(u32, &CapElem) -> Row) -> Ty {
            let row = |r: &Row, with: &mut dyn FnMut(u32, &CapElem) -> Row| {
                r.flat_map(|e| match e {
                    RowElem::Cap(c) => match c.id {
                        CapId::Quant(i) => with(i, c),
                        _ => Row::from_elems([e.clone()]),
                    },
                    e => Row::from_elems([e.clone()]),
                })
            };
            match ty {
                Ty::Fn(f) if f.quantifies() => ty.clone(),
                Ty::Fn(f) => Ty::func_sets(
                    f.params.iter().map(|p| inner(p, with)).collect::<Vec<_>>(),
                    inner(&f.ret, with),
                    f.fails.clone(),
                    row(&f.uses, with),
                ),
                Ty::Cap(tags, caps) => Ty::Cap(tags.clone(), row(caps, with)),
                Ty::Set(kind, caps) => Ty::Set(*kind, row(caps, with)),
                ty => ty.map_children(|c| inner(c, with)),
            }
        }
        Ty::func_sets(
            f.params.iter().map(|p| inner(p, with)).collect::<Vec<_>>(),
            inner(&f.ret, with),
            f.fails.clone(),
            row(&f.uses, with),
        )
    }

    /// Visits this type and every type inside it.
    pub fn visit(&self, f: &mut dyn FnMut(&Ty)) {
        f(self);
        self.for_each_child(|c| c.visit(f));
    }

    /// Visits the immediate child types, including types inside a function type's error set
    /// (`RowElem::Of`) and type arguments of effect-set tags (the `Int` of `State[Int]`).
    pub fn for_each_child(&self, mut f: impl FnMut(&Ty)) {
        match self {
            Ty::Prim(_) | Ty::Never | Ty::Var(_) | Ty::Bound(_) | Ty::Error => {}
            Ty::Cap(tags, caps) => {
                tags.iter().flat_map(|t| &t.args).for_each(&mut f);
                caps.for_each_ty(f);
            }
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
            Ty::Cap(tags, caps) => Ty::Cap(
                tags.iter().map(|t| t.map_args(&mut f)).collect(),
                caps.map_tys(f),
            ),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SetKind {
    Errors,
    Effects,
    /// The capabilities that a capability type or a capability-bound type records
    /// (`[type.rules]`).
    Caps,
}

/// Displayed as `?e0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RowVar(pub(crate) u32);

impl RowVar {
    pub fn index(self) -> u32 {
        self.0
    }
}

/// For E, an error definition; for R, an effect definition and its type arguments (spec 8.1).
///
/// An effect with different type arguments is a different tag (`State[Int]` and
/// `State[String]`). `args` is empty for errors and effects without type parameters. The
/// arguments may still hold inference type variables, so resolve them with
/// `InferCtx::resolve_row` before comparing.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Tag {
    pub def: DefId,
    pub args: Vec<Ty>,
}

impl Tag {
    pub fn new(def: DefId) -> Self {
        Tag {
            def,
            args: Vec::new(),
        }
    }

    pub fn with_args(def: DefId, args: Vec<Ty>) -> Self {
        Tag { def, args }
    }

    pub fn map_args(&self, f: impl FnMut(&Ty) -> Ty) -> Tag {
        Tag {
            def: self.def,
            args: self.args.iter().map(f).collect(),
        }
    }
}

/// The `item` of [`CapId::Item`] for the handler members that an init opens through the handlers
/// of a `with` inside the init (`[effect.handler]`): they live as long as that `with`, as its
/// items do, and the `with` removes them from R.
pub const OPENED_IN_INIT: u32 = u32::MAX;

/// Which capability an element is (`[effect.with]`, `[type.rules]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CapId {
    /// The capability that the `item`-th item of the `with` expression makes. For an item
    /// without a name, it is reached only through the name of its effect.
    Item { with: ExprId, item: u32 },
    /// The name of an effect inside something that provides it by name: a `with` (`provider` is
    /// the `with` expression), or a function passed to a parameter whose function type writes
    /// the effect out while the callee does not have it (`provider` is the lambda, `[effect.escape]`).
    /// It is the effect itself in R. Outside its provider it would leave the provider, so it is
    /// told apart from the plain name of the effect ([`RowElem::Tag`]) in capability sets.
    Named { provider: ExprId, effect: DefId },
    /// `self` in an operation body or the release of the handler `handler` (`[effect.handler]`).
    SelfOf(DefId),
    /// The unknown set that a closure receives for the `index`-th quantified capability set of
    /// the function type it is checked against (`[type.rules]`). `binder` is the lambda.
    Rigid { binder: ExprId, index: u32 },
    /// The `i`-th capability set quantified at a function type (`fn(use ReadFile) -> String`,
    /// `[type.rules]`). The function type that has it in its R binds it: the R of a quantified
    /// function type always has its quantified sets. Shown as `any`.
    Quant(u32),
    /// The unknown set put for a quantified set where a function value other than a closure
    /// is placed at a quantified function type. Lives in the scope where it is made.
    Skolem(u32),
}

/// A capability as an element of a set (`uses { primary }`, `[effect.use]`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CapElem {
    pub id: CapId,
    /// How it is shown (`[lsp.hover]`): the variable of a named item, the handler of an item
    /// without a name, `self`, or `any`.
    pub name: SmolStr,
    /// The effect of the capability. In R it is the effect of the operation called, which
    /// decides whether the call may suspend (E4303); it is not widened to it.
    pub tag: Tag,
}

impl CapElem {
    /// The same capability: the tag and the name do not take part.
    pub fn same(&self, other: &CapElem) -> bool {
        self.id == other.id
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RowElem {
    /// For E, an error definition; for R, an effect definition and its type arguments.
    Tag(Tag),
    /// A set type parameter of a type scheme ([`crate::ParamKind::Set`]); an index into
    /// `Scheme::params`.
    Bound(u32),
    /// The tag of the error type `ty`, once `ty` resolves to a named error type.
    Of(Ty),
    Var(RowVar),
    /// A capability (effect sets and capability sets). Contained only in a set that has the same
    /// capability ([`CapElem::same`]).
    Cap(CapElem),
}

impl RowElem {
    /// The effect of a tag or of a capability, which decides whether it may suspend.
    pub fn effect_tag(&self) -> Option<&Tag> {
        match self {
            RowElem::Tag(t) => Some(t),
            RowElem::Cap(c) => Some(&c.tag),
            _ => None,
        }
    }
}

/// Elements are not duplicated: two capabilities with the same [`CapId`] are one element (the
/// first one inserted is kept). Order has no meaning, but for stable display the elements are
/// sorted as tags, capabilities, type parameters, types, then set variables (tags, type parameters and set
/// variables by number within their group).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Row {
    elems: Vec<RowElem>,
}

impl Row {
    pub fn empty() -> Self {
        Row::default()
    }

    pub fn tag(def: DefId) -> Self {
        Row::from_elems([RowElem::Tag(Tag::new(def))])
    }

    pub fn of_tag(tag: Tag) -> Self {
        Row::from_elems([RowElem::Tag(tag)])
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
        match elem {
            RowElem::Cap(c) => self.has_cap(c),
            e => self.elems.contains(e),
        }
    }

    /// Does nothing if the element is already present.
    pub fn insert(&mut self, elem: RowElem) {
        if self.contains(&elem) {
            return;
        }
        let at = self
            .elems
            .iter()
            .position(|e| order(e) > order(&elem))
            .unwrap_or(self.elems.len());
        self.elems.insert(at, elem);
    }

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

    pub fn tags(&self) -> impl Iterator<Item = &Tag> + '_ {
        self.elems.iter().filter_map(|e| match e {
            RowElem::Tag(t) => Some(t),
            _ => None,
        })
    }

    /// The tags, and the effects of capability elements.
    pub fn effect_tags(&self) -> impl Iterator<Item = &Tag> + '_ {
        self.elems.iter().filter_map(RowElem::effect_tag)
    }

    /// The capabilities.
    pub fn caps(&self) -> impl Iterator<Item = &CapElem> + '_ {
        self.elems.iter().filter_map(|e| match e {
            RowElem::Cap(c) => Some(c),
            _ => None,
        })
    }

    /// Whether the set has the capability `cap` ([`CapElem::same`]).
    pub fn has_cap(&self, cap: &CapElem) -> bool {
        self.caps().any(|c| c.same(cap))
    }

    /// The set with each named effect of a provider ([`CapId::Named`]) replaced by the effect:
    /// in R, the name of an effect is the effect.
    pub fn names_as_effects(&self) -> Row {
        Row::from_elems(self.elems.iter().map(|e| match e {
            RowElem::Cap(c) if matches!(c.id, CapId::Named { .. }) => RowElem::Tag(c.tag.clone()),
            e => e.clone(),
        }))
    }

    pub fn vars(&self) -> impl Iterator<Item = RowVar> + '_ {
        self.elems.iter().filter_map(|e| match e {
            RowElem::Var(v) => Some(*v),
            _ => None,
        })
    }

    /// Visits the types inside the elements (`RowElem::Of` and tag type arguments).
    pub fn for_each_ty(&self, mut f: impl FnMut(&Ty)) {
        for e in &self.elems {
            match e {
                RowElem::Of(ty) => f(ty),
                RowElem::Tag(t) | RowElem::Cap(CapElem { tag: t, .. }) => {
                    t.args.iter().for_each(&mut f)
                }
                _ => {}
            }
        }
    }

    /// Replaces the types inside the elements (`RowElem::Of` and tag type arguments) with `f`.
    pub fn map_tys(&self, mut f: impl FnMut(&Ty) -> Ty) -> Row {
        Row::from_elems(self.elems.iter().map(|e| match e {
            RowElem::Of(ty) => RowElem::Of(f(ty)),
            RowElem::Tag(t) => RowElem::Tag(t.map_args(&mut f)),
            RowElem::Cap(c) => RowElem::Cap(CapElem {
                tag: c.tag.map_args(&mut f),
                ..c.clone()
            }),
            e => e.clone(),
        }))
    }

    /// `f` may expand one element into a set.
    pub fn flat_map(&self, mut f: impl FnMut(&RowElem) -> Row) -> Row {
        let mut out = Row::empty();
        for e in &self.elems {
            out.extend(&f(e));
        }
        out
    }
}

/// Sort key. Types inside `Of`, and tags of the same definition with different type arguments,
/// keep insertion order. Capabilities come after the tags, by variable.
fn order(e: &RowElem) -> (u8, u32) {
    match e {
        RowElem::Tag(t) => (0, t.def.into_raw().into_u32()),
        RowElem::Cap(c) => (1, cap_order(c.id)),
        RowElem::Bound(i) => (2, *i),
        RowElem::Of(_) => (3, 0),
        RowElem::Var(v) => (4, v.0),
    }
}

fn cap_order(id: CapId) -> u32 {
    match id {
        CapId::Item { with, item } => with
            .into_raw()
            .into_u32()
            .wrapping_mul(8)
            .wrapping_add(item),
        CapId::Named { provider, .. } => provider.into_raw().into_u32(),
        CapId::SelfOf(def) => def.into_raw().into_u32(),
        CapId::Rigid { binder, index } => binder.into_raw().into_u32().wrapping_add(index),
        CapId::Quant(i) | CapId::Skolem(i) => i,
    }
}

/// `Ty::Bound(i)` inside `ty` refers to `params[i]`.
///
/// Only top-level functions are generalized; bindings inside blocks stay monomorphic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scheme {
    pub params: Vec<TyParam>,
    pub ty: Ty,
}

impl Scheme {
    pub fn new(params: Vec<TyParam>, ty: Ty) -> Self {
        Scheme { params, ty }
    }

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

/// Names for generalized type parameters: `A` to `Z`, then `T26`, `T27`, ...
pub(crate) fn param_name(i: usize) -> String {
    if i < 26 {
        char::from(b'A' + i as u8).to_string()
    } else {
        format!("T{i}")
    }
}

/// Names for generalized set type parameters: with prefix `E`, `E`, `E1`, `E2`, ...
pub(crate) fn set_param_name(prefix: &str, i: usize) -> String {
    if i == 0 {
        prefix.to_string()
    } else {
        format!("{prefix}{i}")
    }
}

/// Replaces `Ty::Bound(i)` with `args[i]`; used to substitute type arguments of named types. Set
/// type parameters (`RowElem::Bound(i)`) are replaced when `args[i]` is a set (`Ty::Set`), and
/// left as is otherwise.
pub fn substitute_bound(ty: &Ty, args: &[Ty]) -> Ty {
    let rows: Vec<Row> = args
        .iter()
        .enumerate()
        .map(|(i, a)| match a {
            Ty::Set(_, row) => row.clone(),
            _ => Row::from_elems([RowElem::Bound(i as u32)]),
        })
        .collect();
    substitute(ty, args, &rows)
}

/// Replaces `Ty::Bound(i)` with `tys[i]` and `RowElem::Bound(i)` in sets with `rows[i]`.
/// Out-of-range indices are left as is.
pub(crate) fn substitute(ty: &Ty, tys: &[Ty], rows: &[Row]) -> Ty {
    match ty {
        Ty::Bound(i) => tys.get(*i as usize).cloned().unwrap_or_else(|| ty.clone()),
        Ty::Fn(f) => Ty::func_sets(
            f.params
                .iter()
                .map(|p| substitute(p, tys, rows))
                .collect::<Vec<_>>(),
            substitute(&f.ret, tys, rows),
            substitute_row(&f.fails, tys, rows),
            substitute_row(&f.uses, tys, rows),
        ),
        Ty::Set(kind, row) => Ty::Set(*kind, substitute_row(row, tys, rows)),
        Ty::Cap(tags, caps) => Ty::Cap(
            tags.iter()
                .map(|t| t.map_args(|a| substitute(a, tys, rows)))
                .collect(),
            substitute_row(caps, tys, rows),
        ),
        _ => ty.map_children(|child| substitute(child, tys, rows)),
    }
}

/// Replaces type parameters inside a set like [`substitute`].
pub(crate) fn substitute_row(row: &Row, tys: &[Ty], rows: &[Row]) -> Row {
    row.flat_map(|e| match e {
        RowElem::Bound(i) => rows
            .get(*i as usize)
            .cloned()
            .unwrap_or_else(|| Row::from_elems([e.clone()])),
        RowElem::Of(t) => Row::from_elems([RowElem::Of(substitute(t, tys, rows))]),
        RowElem::Tag(t) => Row::of_tag(t.map_args(|a| substitute(a, tys, rows))),
        RowElem::Cap(c) => Row::from_elems([RowElem::Cap(CapElem {
            tag: c.tag.map_args(|a| substitute(a, tys, rows)),
            ..c.clone()
        })]),
        e => Row::from_elems([e.clone()]),
    })
}

impl Row {
    /// The `NotFound | E` form; `{}` if empty.
    pub fn display<'a>(&'a self, cons: &'a TyCons) -> RowDisplay<'a> {
        RowDisplay {
            row: self,
            effects: false,
            clause: false,
            ty: TyDisplay {
                ty: &Ty::Never,
                cons,
                bound: &[],
                in_param: false,
            },
        }
    }

    /// Like [`Self::display`], taking the names of `Bound(i)` from `bound`.
    pub fn display_with<'a>(&'a self, cons: &'a TyCons, bound: &'a [TyParam]) -> RowDisplay<'a> {
        RowDisplay {
            row: self,
            effects: false,
            clause: false,
            ty: TyDisplay {
                ty: &Ty::Never,
                cons,
                bound,
                in_param: false,
            },
        }
    }
}

impl Row {
    /// The effect-set form of a set type argument (spec 17.4): `Logger` for one element,
    /// `{ Logger, Clock }` for two or more, `{}` if empty. Also names one effect alone
    /// (`State[Int]`). Takes the names of `Bound(i)` from `bound`.
    pub fn display_effects<'a>(&'a self, cons: &'a TyCons, bound: &'a [TyParam]) -> RowDisplay<'a> {
        RowDisplay {
            effects: true,
            ..self.display_with(cons, bound)
        }
    }

    /// The set of a uses clause (`[type.inference]`): always in braces, `{ Logger }`,
    /// `{ Logger, Clock }`, `{}`. The caller writes `uses` before it. Takes the names of
    /// `Bound(i)` from `bound`.
    pub fn display_uses<'a>(&'a self, cons: &'a TyCons, bound: &'a [TyParam]) -> RowDisplay<'a> {
        RowDisplay {
            effects: true,
            clause: true,
            ..self.display_with(cons, bound)
        }
    }
}

impl Ty {
    /// Like [`Self::display`], taking the names of `Bound(i)` from `bound`.
    pub fn display_with<'a>(&'a self, cons: &'a TyCons, bound: &'a [TyParam]) -> TyDisplay<'a> {
        TyDisplay {
            ty: self,
            cons,
            bound,
            in_param: false,
        }
    }
}

/// Error sets separate elements with ` | `. Effect sets with two or more elements are wrapped
/// in braces as `{ A, B }` and separated with `, `; the set of a uses clause always is.
pub struct RowDisplay<'a> {
    row: &'a Row,
    /// Whether to use the effect-set form.
    effects: bool,
    /// The set of a uses clause, in braces even with one element.
    clause: bool,
    /// Context for displaying types inside elements (its `ty` is unused).
    ty: TyDisplay<'a>,
}

impl fmt::Display for RowDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.row.is_empty() {
            return f.write_str("{}");
        }
        let braces = self.effects && (self.clause || self.row.elems().len() > 1);
        if braces {
            f.write_str("{ ")?;
        }
        let mut first = true;
        for e in self.row.elems() {
            // The name of an effect inside its provider is the effect (`[type.rules]`): shown
            // once.
            if let RowElem::Cap(c) = e
                && matches!(c.id, CapId::Named { .. })
                && self.row.tags().any(|t| t.def == c.tag.def)
            {
                continue;
            }
            if !first {
                f.write_str(if self.effects { ", " } else { " | " })?;
            }
            first = false;
            match e {
                RowElem::Tag(tag) => self.ty.tag(f, tag)?,
                // The set of a capability parameter is named by the parameter (`[lsp.hover]`).
                RowElem::Bound(i) => match self.ty.bound.get(*i as usize) {
                    Some(p) if p.caps.is_some() => f.write_str(cap_param_name(&p.name))?,
                    Some(p) => f.write_str(&p.name)?,
                    None => write!(f, "'{i}")?,
                },
                RowElem::Of(ty) => write!(f, "{}", self.ty.child(ty))?,
                RowElem::Var(v) => write!(f, "?e{}", v.index())?,
                RowElem::Cap(c) => f.write_str(&c.name)?,
            }
        }
        if braces {
            f.write_str(" }")?;
        }
        Ok(())
    }
}

impl Ty {
    pub fn display<'a>(&'a self, cons: &'a TyCons) -> TyDisplay<'a> {
        TyDisplay {
            ty: self,
            cons,
            bound: &[],
            in_param: false,
        }
    }
}

impl Scheme {
    pub fn display<'a>(&'a self, cons: &'a TyCons) -> SchemeDisplay<'a> {
        SchemeDisplay { scheme: self, cons }
    }
}

pub struct TyDisplay<'a> {
    ty: &'a Ty,
    cons: &'a TyCons,
    /// Names for `Ty::Bound(i)`. Out of range is shown as `'i`.
    bound: &'a [TyParam],
    /// Inside a parameter type of a function type, where an empty error set is shown as
    /// `fails {}`.
    in_param: bool,
}

impl TyDisplay<'_> {
    fn child<'b>(&'b self, ty: &'b Ty) -> TyDisplay<'b> {
        TyDisplay {
            ty,
            cons: self.cons,
            bound: self.bound,
            in_param: self.in_param,
        }
    }

    fn list(&self, f: &mut fmt::Formatter<'_>, tys: &[Ty]) -> fmt::Result {
        for (i, ty) in tys.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{}", self.child(ty))?;
        }
        Ok(())
    }

    /// `State[Int]` if the tag has type arguments (spec 8.1).
    fn tag(&self, f: &mut fmt::Formatter<'_>, tag: &Tag) -> fmt::Result {
        match self.cons.tag_name(tag.def) {
            Some(name) => f.write_str(name)?,
            None => write!(f, "#{}", tag.def.into_raw())?,
        }
        // The capability sets an error records are not written (`[type.rules]`).
        let args: Vec<Ty> = tag
            .args
            .iter()
            .filter(|a| !matches!(a, Ty::Set(SetKind::Caps, _)))
            .cloned()
            .collect();
        if !args.is_empty() {
            f.write_str("[")?;
            self.list(f, &args)?;
            f.write_str("]")?;
        }
        Ok(())
    }

    fn params(&self, f: &mut fmt::Formatter<'_>, tys: &[Ty]) -> fmt::Result {
        for (i, ty) in tys.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            let child = TyDisplay {
                in_param: true,
                ..self.child(ty)
            };
            write!(f, "{child}")?;
        }
        Ok(())
    }
}

impl fmt::Display for TyDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.ty {
            Ty::Prim(p) => f.write_str(p.name()),
            Ty::Never => f.write_str("Never"),
            Ty::Error => f.write_str("{unknown}"),
            Ty::Var(v) => write!(f, "?{}", v.index()),
            Ty::Bound(i) => match self.bound.get(*i as usize) {
                Some(p) => f.write_str(&p.name),
                None => write!(f, "'{i}"),
            },
            Ty::Cap(tags, _) => {
                f.write_str("use ")?;
                for (i, tag) in tags.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" + ")?;
                    }
                    self.tag(f, tag)?;
                }
                Ok(())
            }
            Ty::Fn(fun) => {
                f.write_str("fn(")?;
                self.params(f, &fun.params)?;
                // The quantified sets are in the R of their function type without being written
                // (`[type.rules]`).
                let uses = Row::from_elems(
                    fun.uses
                        .elems()
                        .iter()
                        .filter(
                            |e| !matches!(e, RowElem::Cap(c) if matches!(c.id, CapId::Quant(_))),
                        )
                        .cloned(),
                );
                let show_fails = !fun.fails.is_empty() || self.in_param;
                let show_uses = !uses.is_empty() || self.in_param;
                match &fun.ret {
                    Ty::Fn(_) if show_fails || show_uses => {
                        write!(f, ") -> ({})", self.child(&fun.ret))?
                    }
                    ret => write!(f, ") -> {}", self.child(ret))?,
                }
                if show_fails {
                    let row = RowDisplay {
                        row: &fun.fails,
                        effects: false,
                        clause: false,
                        ty: self.child(&fun.ret),
                    };
                    write!(f, " fails {row}")?;
                }
                if show_uses {
                    let row = RowDisplay {
                        row: &uses,
                        effects: true,
                        clause: true,
                        ty: self.child(&fun.ret),
                    };
                    write!(f, " uses {row}")?;
                }
                Ok(())
            }
            // Set type arguments of a named type (`Handler[NotFound | Timeout, { Db, Logger }]`).
            Ty::Set(kind, row) => {
                let row = RowDisplay {
                    row,
                    effects: *kind == SetKind::Effects,
                    clause: false,
                    ty: self.child(self.ty),
                };
                write!(f, "{row}")
            }
            Ty::Tuple(elems) => {
                f.write_str("(")?;
                self.list(f, elems)?;
                f.write_str(")")
            }
            Ty::List(elem) => write!(f, "List[{}]", self.child(elem)),
            Ty::Named(id, args) => {
                f.write_str(&self.cons[*id].name)?;
                // The hidden capability sets of a declaration are not written (`[type.rules]`).
                let params = &self.cons[*id].params;
                let visible: Vec<Ty> = args
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| params.get(*i).is_none_or(|p| p.caps.is_none()))
                    .map(|(_, a)| a.clone())
                    .collect();
                if !visible.is_empty() {
                    f.write_str("[")?;
                    self.list(f, &visible)?;
                    f.write_str("]")?;
                }
                Ok(())
            }
        }
    }
}

/// `∀ A: Ord + Show, B. fn(A) -> B`. A scheme without quantifiers shows only the type.
pub struct SchemeDisplay<'a> {
    scheme: &'a Scheme,
    cons: &'a TyCons,
}

impl fmt::Display for SchemeDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The sets of capability parameters are named after the parameters in R, not
        // quantified in the display (`[lsp.hover]`).
        let params: Vec<&TyParam> = self
            .scheme
            .params
            .iter()
            .filter(|p| p.caps.is_none())
            .collect();
        if !params.is_empty() {
            f.write_str("∀ ")?;
            for (i, p) in params.iter().enumerate() {
                if i > 0 {
                    f.write_str(", ")?;
                }
                f.write_str(&p.name)?;
                for (j, b) in p.bounds.iter().enumerate() {
                    f.write_str(if j == 0 { ": " } else { " + " })?;
                    f.write_str(b)?;
                }
            }
            f.write_str(". ")?;
        }
        let ty = TyDisplay {
            ty: &self.scheme.ty,
            cons: self.cons,
            bound: &self.scheme.params,
            in_param: false,
        };
        write!(f, "{ty}")
    }
}

/// The parameter that the set type parameter of a capability parameter belongs to: `db` for
/// `C(db)` (`[lsp.hover]`).
pub(crate) fn cap_param_name(name: &str) -> &str {
    name.strip_prefix("C(")
        .and_then(|n| n.strip_suffix(')'))
        .map(|n| n.split(',').next().unwrap_or(n))
        .unwrap_or(name)
}
