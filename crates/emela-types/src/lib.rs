//! Type inference, inference of `fails` and `uses`, trait resolution, match exhaustiveness and
//! the capabilities that values hold.
//!
//! Judgements have the form `Γ ⊢ e : T ! E ; R` (spec 9.1). T is inferred with Hindley-Milner;
//! E and R are computed with set union and difference.

mod body;
mod check;
mod decl;
mod diagnostic;
mod exhaustiveness;
mod infer;
mod scopes;
mod sparse;
mod ty;

#[cfg(test)]
mod tests;

pub use check::entry_main::is_exit_code;
pub use check::export::{
    HostExports, add_entry_exports, host_exports, is_entry_adapter, is_http_serve, is_http_service,
};
pub use check::program_types::{ProgramTypes, methods_of};
pub use check::{CheckOptions, body_refs, check_program, check_program_with};
pub use decl::layer::{ArgsSource, LayerInfo, MemberProvider, Supplier, WithBind, WithInfo};
pub use decl::lower::{TyTable, is_immediate_effect};
pub use decl::signature::{InitParams, Signature, Signatures};
pub use decl::traits::{ImplHead, Traits, Unsatisfied, derivable, head_def, is_capability_type};
pub use diagnostic::{
    BoundItem, Diagnostic, DiagnosticKind, EffectContext, HandlerArgsReason, ImmediateSource,
    InnerHandlerMismatch, LabelBaseReason, LayerArg, LeakFrom, MatchOnLogical, SuspendingCall,
};
pub use exhaustiveness::{
    Arm, CtorTable, Lit, MatchCheck, Pat, TyShape, VariantShape, WITNESS_LIMIT, Witness,
    check_match,
};
pub use infer::{InferCtx, TypeError, TypeErrorKind};
pub use ty::{
    CapElem, CapId, FnTy, ParamKind, Prim, Row, RowElem, RowVar, Scheme, SchemeDisplay, SetKind,
    Tag, Ty, TyConData, TyConId, TyConKind, TyCons, TyDisplay, TyParam, TyVar, substitute_bound,
};
