//! Set types. Error sets E (spec chapter 7) and effect sets R (spec chapter 8) share one
//! representation.
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

use emela_resolve::hir::{DefId, ExprId};
use smol_str::SmolStr;

use crate::ty::Ty;

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
}

/// A capability as an element of a set (`uses { primary }`, `[effect.use]`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CapElem {
    pub id: CapId,
    /// How it is shown (`[lsp.hover]`): the variable of a named item, the handler of an item
    /// without a name, `self`, or `any`.
    pub name: SmolStr,
    /// The effect of the capability. In R it is the effect of the operation called, which
    /// decides whether the call may suspend (E0417); it is not widened to it.
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
    }
}
