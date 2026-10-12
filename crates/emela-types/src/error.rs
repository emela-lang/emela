//! Type mismatch reports.
//!
//! These carry no span yet; the caller attaches one when connecting to the syntax tree.

use crate::ty::{Ty, TyVar};

/// A unification failure. `expected` and `actual` are the types passed to unification, with
/// bindings followed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeError {
    pub expected: Ty,
    pub actual: Ty,
    /// Boxed to keep the `Err` of a `Result` small.
    pub kind: Box<TypeErrorKind>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeErrorKind {
    /// The part that differs. Same as the `TypeError`'s `expected` and `actual` when the whole
    /// type differs.
    Mismatch { expected: Ty, actual: Ty },
    /// Binding `var` to `ty` would create an infinite type (occurs check).
    InfiniteType { var: TyVar, ty: Ty },
}
