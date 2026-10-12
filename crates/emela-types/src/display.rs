//! Displays types the way the spec writes them (`List[Int]`, `fn(Int) -> Bool fails NotFound`,
//! `(Int, String)`).
//!
//! Names of named types and error tags live in `TyCons`, so displaying takes the table. Type
//! variables are shown as `?0` without following bindings; to show the resolved form, pass the
//! type through `InferCtx::resolve` first.
//!
//! An empty error set or effect set in a function type drops the whole `fails` or `use` clause.
//! Inside a parameter type, however, dropping them would read as "anything", so they are shown
//! as `fails {}` and `use {}` (spec 9.2). When a function returns a function type and the outer
//! function type has a `fails` or `use` clause, the return type is parenthesized so it is clear
//! which function the clause belongs to (spec 5.5).
//!
//! Capability value types (`Ty::Cap`) cannot be written in source, so they are shown as the
//! expression that creates them, `use Logger`. Effects with type arguments are shown as
//! `State[Int]`, both in sets and as capability values (spec 8.1).

use std::fmt;

use crate::row::{Row, RowElem, SetKind, Tag};
use crate::scheme::Scheme;
use crate::ty::{Ty, TyCons, TyParam};

impl Row {
    /// The `NotFound | E` form; `{}` if empty.
    pub fn display<'a>(&'a self, cons: &'a TyCons) -> RowDisplay<'a> {
        RowDisplay {
            row: self,
            effects: false,
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
    /// The effect-set form (spec 17.4): `Logger` for one element, `{ Logger, Clock }` for two or
    /// more, `{}` if empty. Takes the names of `Bound(i)` from `bound`.
    pub fn display_effects<'a>(&'a self, cons: &'a TyCons, bound: &'a [TyParam]) -> RowDisplay<'a> {
        RowDisplay {
            effects: true,
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
/// in braces as `{ A, B }` and separated with `, `.
pub struct RowDisplay<'a> {
    row: &'a Row,
    /// Whether to use the effect-set form.
    effects: bool,
    /// Context for displaying types inside elements (its `ty` is unused).
    ty: TyDisplay<'a>,
}

impl fmt::Display for RowDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.row.is_empty() {
            return f.write_str("{}");
        }
        let braces = self.effects && self.row.elems().len() > 1;
        if braces {
            f.write_str("{ ")?;
        }
        for (i, e) in self.row.elems().iter().enumerate() {
            if i > 0 {
                f.write_str(if self.effects { ", " } else { " | " })?;
            }
            match e {
                RowElem::Tag(tag) => self.ty.tag(f, tag)?,
                RowElem::Bound(i) => match self.ty.bound.get(*i as usize) {
                    Some(p) => f.write_str(&p.name)?,
                    None => write!(f, "'{i}")?,
                },
                RowElem::Of(ty) => write!(f, "{}", self.ty.child(ty))?,
                RowElem::Var(v) => write!(f, "?e{}", v.index())?,
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
        if !tag.args.is_empty() {
            f.write_str("[")?;
            self.list(f, &tag.args)?;
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
            Ty::Cap(tag) => {
                f.write_str("use ")?;
                self.tag(f, tag)
            }
            Ty::Fn(fun) => {
                f.write_str("fn(")?;
                self.params(f, &fun.params)?;
                let show_fails = !fun.fails.is_empty() || self.in_param;
                let show_uses = !fun.uses.is_empty() || self.in_param;
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
                        ty: self.child(&fun.ret),
                    };
                    write!(f, " fails {row}")?;
                }
                if show_uses {
                    let row = RowDisplay {
                        row: &fun.uses,
                        effects: true,
                        ty: self.child(&fun.ret),
                    };
                    write!(f, " use {row}")?;
                }
                Ok(())
            }
            // Set type arguments of a named type (`Handler[NotFound | Timeout, { Db, Logger }]`).
            Ty::Set(kind, row) => {
                let row = RowDisplay {
                    row,
                    effects: *kind == SetKind::Effects,
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
                if !args.is_empty() {
                    f.write_str("[")?;
                    self.list(f, args)?;
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
        let params = &self.scheme.params;
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
            bound: params,
            in_param: false,
        };
        write!(f, "{ty}")
    }
}
