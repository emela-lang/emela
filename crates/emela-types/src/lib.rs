//! 型推論，fails / use の推論，Trait の解決，match の網羅性，capture set．
//!
//! 判定の形は `Γ ⊢ e : T ! E ; R`（仕様 9.1）．T は Hindley-Milner で推論し，
//! E と R は集合の和と差で求める．

mod display;
mod error;
mod exhaustiveness;
mod infer;
mod scheme;
mod ty;

#[cfg(test)]
mod tests;

pub use display::{SchemeDisplay, TyDisplay};
pub use error::{TypeError, TypeErrorKind};
pub use exhaustiveness::{
    Arm, CtorTable, Lit, MatchCheck, Pat, TyShape, VariantShape, WITNESS_LIMIT, Witness,
    check_match,
};
pub use infer::{InferCtx, substitute_bound};
pub use scheme::Scheme;
pub use ty::{FnTy, Prim, Ty, TyConData, TyConId, TyConKind, TyCons, TyParam, TyVar};
