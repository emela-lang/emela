//! Type inference, inference of `fails` and `uses`, trait resolution, match exhaustiveness and
//! the capabilities that values hold.
//!
//! Judgements have the form `Γ ⊢ e : T ! E ; R` (spec 9.1). T is inferred with Hindley-Milner;
//! E and R are computed with set union and difference.

mod body;
mod builtin;
mod check;
mod const_cycle;
mod coverage;
mod diagnostic;
mod display;
mod entry_main;
mod error;
mod exhaustiveness;
mod export;
mod infer;
mod layer;
mod lower;
mod methods;
mod row;
mod scheme;
mod scopes;
mod signature;
mod sparse;
mod traits;
mod ty;
mod unused_type_params;

#[cfg(test)]
mod tests;

pub use check::{CheckOptions, ProgramTypes, body_refs, check_program, check_program_with};
pub use diagnostic::{
    BoundItem, Diagnostic, DiagnosticKind, EffectContext, HandlerArgsReason, ImmediateSource,
    InnerHandlerMismatch, LabelBaseReason, LayerArg, LeakFrom, SuspendingCall,
};
pub use display::{SchemeDisplay, TyDisplay};
pub use entry_main::is_exit_code;
pub use error::{TypeError, TypeErrorKind};
pub use exhaustiveness::{
    Arm, CtorTable, Lit, MatchCheck, Pat, TyShape, VariantShape, WITNESS_LIMIT, Witness,
    check_match,
};
pub use export::{
    HostExports, add_entry_exports, host_exports, is_entry_adapter, is_http_serve, is_http_service,
};
pub use infer::{InferCtx, substitute_bound};
pub use layer::{ArgsSource, LayerInfo, MemberProvider, Supplier, WithBind, WithInfo};
pub use lower::{TyTable, is_immediate_effect};
pub use methods::methods_of;
pub use scheme::Scheme;
pub use signature::{InitParams, Signature, Signatures};
pub use traits::{ImplHead, Traits, Unsatisfied, derivable, head_def, is_capability_type};
pub use ty::{
    CapElem, CapId, FnTy, ParamKind, Prim, Row, RowElem, RowVar, SetKind, Tag, Ty, TyConData,
    TyConId, TyConKind, TyCons, TyParam, TyVar,
};
