//! Capability sets (`[type.rules]`): the capabilities that `use E` and the capability types in
//! signatures stand for, and the names of effects provided by name.
//!
//! A capability value is an ordinary value (`[effect.use]`): it can be passed as an argument,
//! stored in a data structure and returned. Leaving the `with` that made it is checked with the
//! scopes of type and set variables (`crate::scopes`, `body/errors.rs`).

use emela_resolve::hir::ExprId;
use smol_str::SmolStr;

use super::BodyInfer;
use crate::infer::{Origin, SetKind};
use crate::row::{CapElem, CapId, Row, RowElem, Tag};
use crate::ty::Ty;

/// Capability sets (`[type.rules]`).
impl BodyInfer<'_> {
    /// The capability set of `use E` written as the expression `expr`. An effect alias (and
    /// `(use E).op`) is the effect itself, resolved at the call. A value of `use E` (W0403) is a
    /// capability of the name E ([`Self::anchor_name`]).
    pub(super) fn use_caps(&mut self, expr: ExprId, tag: &Tag) -> Row {
        if self.program.alias_uses.get(expr).is_some() {
            return Row::of_tag(tag.clone());
        }
        Row::from_elems([self.anchor_name(tag)])
    }

    /// The name of the effect of `tag` at the expression being inferred: the named effect of the
    /// innermost `with` that provides it by name in the same function ([`CapId::Named`]), or the
    /// effect itself when no `with` of the function provides it (the caller does).
    pub(super) fn anchor_name(&mut self, tag: &Tag) -> RowElem {
        let providers = self.name_providers.clone();
        for p in providers.iter().rev() {
            let Some((provider, provides)) = p else {
                break;
            };
            if provides.iter().any(|p| self.ctx.same_tag(p, tag)) {
                return RowElem::Cap(CapElem {
                    id: CapId::Named {
                        provider: *provider,
                        effect: tag.def,
                    },
                    name: self.program.defs[tag.def].name.clone(),
                    tag: tag.clone(),
                });
            }
        }
        RowElem::Tag(tag.clone())
    }

    /// The capability that the `item`-th item of the `with` expression `with` makes, named by
    /// `name` (`[effect.with]`). `tag` is its first effect.
    pub(super) fn item_cap(&self, with: ExprId, item: usize, name: SmolStr, tag: Tag) -> RowElem {
        RowElem::Cap(CapElem {
            id: CapId::Item {
                with,
                item: item as u32,
            },
            name,
            tag,
        })
    }

    /// `ty` with each capability set in a position that only gives values (the type itself, the
    /// elements of lists and tuples, the type arguments of named types) replaced by a fresh set
    /// variable that contains it. A variable then fits where capabilities of other `with`s are
    /// put together with it (`[x, y]`): unification takes the union of sets (`[type.rules]`).
    /// Positions inside function types are kept as they are.
    pub(super) fn fresh_caps(&mut self, ty: &Ty) -> Ty {
        match self.ctx.shallow_resolve(ty) {
            Ty::Cap(tags, caps) => {
                let v = Row::var(self.ctx.new_row_var());
                self.ctx
                    .add_subset_of(SetKind::Caps, caps, v.clone(), Origin::Lambda);
                Ty::Cap(tags, v)
            }
            Ty::Set(SetKind::Caps, caps) => {
                let v = Row::var(self.ctx.new_row_var());
                self.ctx
                    .add_subset_of(SetKind::Caps, caps, v.clone(), Origin::Lambda);
                Ty::Set(SetKind::Caps, v)
            }
            ty @ (Ty::Fn(_) | Ty::Set(..)) => ty,
            ty => ty.map_children(|child| self.fresh_caps(child)),
        }
    }

    /// `ty`, written where omitted clauses are empty, with each capability set given a fresh set
    /// variable: a capability type written in an expression records the capabilities it is
    /// used with (`[type.rules]`).
    pub(super) fn infer_written_caps(&mut self, ty: &Ty) -> Ty {
        match ty {
            Ty::Cap(tags, caps) if caps.is_empty() => Ty::Cap(
                tags.iter()
                    .map(|t| t.map_args(|a| self.infer_written_caps(a)))
                    .collect(),
                Row::var(self.ctx.new_row_var()),
            ),
            ty => ty.map_children(|child| self.infer_written_caps(child)),
        }
    }

    /// `ty`, the type of a reference to a definition, with each name of an effect in a capability
    /// set given to the `with` that provides the effect by name at the reference
    /// ([`Self::anchor_name`]). A name in a signature stands for the effect that the caller
    /// provides.
    pub(super) fn anchor_names(&mut self, ty: &Ty) -> Ty {
        match ty {
            Ty::Cap(tags, caps) => {
                let tags = tags
                    .iter()
                    .map(|t| t.map_args(|a| self.anchor_names(a)))
                    .collect();
                Ty::Cap(tags, self.anchor_row(caps))
            }
            Ty::Set(SetKind::Caps, caps) => Ty::Set(SetKind::Caps, self.anchor_row(caps)),
            ty => ty.map_children(|child| self.anchor_names(child)),
        }
    }

    fn anchor_row(&mut self, caps: &Row) -> Row {
        if self.name_providers.last().is_none_or(|p| p.is_none()) {
            return caps.clone();
        }
        Row::from_elems(
            caps.elems()
                .iter()
                .map(|e| match e {
                    RowElem::Tag(t) => self.anchor_name(t),
                    e => e.clone(),
                })
                .collect::<Vec<_>>(),
        )
    }

    /// `ty`, the type of the lambda `binder`, with its unknown sets ([`CapId::Rigid`]) quantified
    /// again, and in its R (`[type.rules]`).
    pub(super) fn requantify(&mut self, ty: &Ty, binder: ExprId) -> Ty {
        let Ty::Fn(f) = ty else {
            return ty.clone();
        };
        let mut quantified = Row::empty();
        let params: Vec<Ty> = f
            .params
            .iter()
            .map(|p| requantify_ty(&self.ctx.resolve(p), binder, &mut quantified))
            .collect();
        let mut uses = f.uses.clone();
        uses.extend(&quantified);
        Ty::func_sets(params, f.ret.clone(), f.fails.clone(), uses)
    }
}

fn requantify_row(row: &Row, binder: ExprId, quantified: &mut Row) -> Row {
    Row::from_elems(row.elems().iter().map(|e| match e {
        RowElem::Cap(c) => match c.id {
            CapId::Rigid { binder: b, index } if b == binder => {
                let q = RowElem::Cap(CapElem {
                    id: CapId::Quant(index),
                    ..c.clone()
                });
                quantified.insert(q.clone());
                q
            }
            _ => e.clone(),
        },
        e => e.clone(),
    }))
}

fn requantify_ty(ty: &Ty, binder: ExprId, quantified: &mut Row) -> Ty {
    match ty {
        Ty::Cap(tags, caps) => Ty::Cap(tags.clone(), requantify_row(caps, binder, quantified)),
        Ty::Set(kind, caps) => Ty::Set(*kind, requantify_row(caps, binder, quantified)),
        Ty::Fn(_) => ty.clone(),
        ty => ty.map_children(|c| requantify_ty(c, binder, quantified)),
    }
}
