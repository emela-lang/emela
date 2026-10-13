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

use emela_resolve::hir::DefId;

use crate::ty::Ty;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SetKind {
    Errors,
    Effects,
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
}

/// Elements are not duplicated. Order has no meaning, but for stable display the elements are
/// sorted as tags, type parameters, types, then set variables (tags, type parameters and set
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
        self.elems.contains(elem)
    }

    /// Does nothing if the element is already present.
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
                RowElem::Tag(t) => t.args.iter().for_each(&mut f),
                _ => {}
            }
        }
    }

    /// Replaces the types inside the elements (`RowElem::Of` and tag type arguments) with `f`.
    pub fn map_tys(&self, mut f: impl FnMut(&Ty) -> Ty) -> Row {
        Row::from_elems(self.elems.iter().map(|e| match e {
            RowElem::Of(ty) => RowElem::Of(f(ty)),
            RowElem::Tag(t) => RowElem::Tag(t.map_args(&mut f)),
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
/// keep insertion order.
fn order(e: &RowElem) -> (u8, u32) {
    match e {
        RowElem::Tag(t) => (0, t.def.into_raw().into_u32()),
        RowElem::Bound(i) => (1, *i),
        RowElem::Of(_) => (2, 0),
        RowElem::Var(v) => (3, v.0),
    }
}
