//! Type schemes `∀ A. T`.

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
