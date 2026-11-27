//! Type schemes `∀ A. T`, and the substitution of their parameters.

use crate::row::{CapElem, Row, RowElem};
use crate::ty::{Ty, TyParam};

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
