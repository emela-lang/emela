//! Upstream Core/Flow adaptation and validation.

use crate::{Error, ErrorKind};
use upstream::{const_origins, derived};
pub(crate) mod upstream;

/// Monomorphizes Core IR, lowers it to Flow IR, and verifies the Flow-stage invariants.
///
/// The backend normalizes the conservative failure edge of an infallible CauseRaise before
/// verifying Flow IR. This path does not call `emela_lir::to_heap`.
pub fn flow_from_core(core: &emela_core::Module) -> Result<emela_lir::Module, Vec<Error>> {
    derived::check_core_input(core)?;
    let prepared_consts = const_origins::monomorphize(core);
    let mono = prepared_consts.module;
    if let Err(errors) = emela_core::verify_monomorphic(&mono) {
        return Err(errors
            .into_iter()
            .map(|error| Error {
                kind: ErrorKind::InvalidCore,
                message: format!("invalid Core IR after monomorphization: {error}"),
            })
            .collect());
    }

    derived::check_core_input(&mono)?;
    let mut flow = emela_lir::from_core(mono).map_err(|errors| {
        errors
            .into_iter()
            .map(|error| Error {
                kind: ErrorKind::FlowLowering,
                message: format!("failed to lower Core IR to Flow IR: {error}"),
            })
            .collect::<Vec<_>>()
    })?;

    const_origins::attach(&mut flow, &prepared_consts.groups);
    const_origins::attach_function_groups(&mut flow, &prepared_consts.function_groups);
    upstream::cause::normalize_infallible_cause_raise(&mut flow);
    verify_flow(&flow)?;
    Ok(flow)
}

pub use upstream::verify::verify_flow;
