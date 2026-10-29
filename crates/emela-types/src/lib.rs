//! Type inference, inference of `fails` and `use`, trait resolution, match exhaustiveness and
//! capture sets.
//!
//! Judgements have the form `Γ ⊢ e : T ! E ; R` (spec 9.1). T is inferred with Hindley-Milner;
//! E and R are computed with set union and difference.

mod body;
mod check;
mod const_cycle;
mod coverage;
mod diagnostic;
mod display;
mod error;
mod exhaustiveness;
mod export;
mod fiber;
mod infer;
mod layer;
mod lower;
mod methods;
mod row;
mod scheme;
mod signature;
mod sparse;
mod traits;
mod ty;

#[cfg(test)]
mod tests;

pub use check::{CheckOptions, ProgramTypes, body_refs, check_program, check_program_with};
pub use diagnostic::{
    Diagnostic, DiagnosticKind, EffectContext, ImmediateSource, LabelBaseReason, LayerArg,
    SuspendingCall,
};
pub use display::{SchemeDisplay, TyDisplay};
pub use error::{TypeError, TypeErrorKind};
pub use exhaustiveness::{
    Arm, CtorTable, Lit, MatchCheck, Pat, TyShape, VariantShape, WITNESS_LIMIT, Witness,
    check_match,
};
pub use export::{
    HostExports, add_entry_exports, host_exports, is_entry_adapter, is_http_serve, is_http_service,
};
pub use infer::{InferCtx, substitute_bound};
pub use layer::{ArgsSource, LayerInfo, Supplier, WithBind, WithInfo};
pub use lower::TyTable;
pub use methods::methods_of;
pub use scheme::Scheme;
pub use signature::{InitParams, Signature, Signatures};
pub use traits::{ImplHead, Traits, Unsatisfied, head_def};
pub use ty::{
    FnTy, ParamKind, Prim, Row, RowElem, RowVar, SetKind, Tag, Ty, TyConData, TyConId, TyConKind,
    TyCons, TyParam, TyVar,
};
