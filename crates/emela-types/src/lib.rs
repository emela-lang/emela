//! 型推論，fails / use の推論，Trait の解決，match の網羅性，capture set．
//!
//! 判定の形は `Γ ⊢ e : T ! E ; R`（仕様 9.1）．T は Hindley-Milner で推論し，
//! E と R は集合の和と差で求める．

mod body;
mod check;
mod coverage;
mod diagnostic;
mod display;
mod error;
mod exhaustiveness;
mod infer;
mod lower;
mod row;
mod scheme;
mod signature;
mod ty;

#[cfg(test)]
mod tests;

pub use check::{CheckOptions, ProgramTypes, body_refs, check_program, check_program_with};
pub use diagnostic::{Diagnostic, DiagnosticKind, EffectContext, ImmediateSource};
pub use display::{SchemeDisplay, TyDisplay};
pub use error::{TypeError, TypeErrorKind};
pub use exhaustiveness::{
    Arm, CtorTable, Lit, MatchCheck, Pat, TyShape, VariantShape, WITNESS_LIMIT, Witness,
    check_match,
};
pub use infer::{InferCtx, substitute_bound};
pub use lower::TyTable;
pub use scheme::Scheme;
pub use signature::{Signature, Signatures};
pub use ty::{
    FnTy, ParamKind, Prim, Row, RowElem, RowVar, SetKind, Ty, TyConData, TyConId, TyConKind,
    TyCons, TyParam, TyVar,
};
