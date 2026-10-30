//! Pure input adaptation. All mutation is confined to owned upstream working copies.
use crate::Error;
use crate::contract::{ir, lowered::*};
use std::collections::HashSet;

mod adapter;
mod closure;
mod coerce;
mod const_bridge;
mod const_runtime;
mod discarded;
mod exports;
pub(crate) mod fiber_normalize;
mod handler;
mod reachability;
mod scope_abi;
mod stages;
mod suspend;
mod traversal;
mod type_normalize;
use reachability::runtime_reachable;
use traversal::{function_positions, runtime_statements};

pub(super) fn lower(flow: &emela_lir::Module) -> Result<LoweredModule, Vec<Error>> {
    check_input_stage(flow.stage)?;
    let mut prepared = flow.clone();
    normalize_flow_types(&mut prepared)?;
    crate::input::upstream::verify::verify_flow(&prepared)?;
    let const_groups = crate::input::upstream::const_origins::groups(&prepared)?;
    crate::input::upstream::const_origins::function_groups(&prepared)?;
    exports::normalize_exports(&mut prepared);
    const_bridge::prepare(&mut prepared, &const_groups);
    let mut runtime_consts = const_runtime::prepare(&mut prepared, runtime_reachable);
    runtime_consts.extend(
        const_groups
            .iter()
            .flat_map(|group| group.specializations.iter().copied()),
    );
    scope_abi::normalize(&mut prepared)?;
    fiber_normalize::normalize_scope_results(&mut prepared);
    let mut closures = closure::lower(&prepared)?;
    let mut prepared = closures
        .module
        .take()
        .expect("closure conversion returns its program");
    fiber_normalize::erase_inert_failures(&mut prepared, &mut closures);
    let closure_functions: HashSet<_> = closures.functions.keys().copied().collect();
    coerce::lower(&mut prepared, &closure_functions);
    let deferred_pool = fiber_normalize::needs_deferred_pool(&prepared, &closures);
    handler::lower(&mut prepared);
    let discarded_results = discarded::normalize(&mut prepared);
    let emitted_functions = runtime_reachable(&prepared);
    let const_function_groups = crate::input::upstream::const_origins::function_groups(&prepared)?;
    let const_outcomes = prepared
        .functions
        .iter()
        .filter(|(id, function)| emitted_functions.contains(id) && function.constant)
        .map(|(id, _)| {
            (
                adapter::id(id),
                crate::input::upstream::const_eval::evaluate(&prepared, id)
                    .map(|outcome| adapter::outcome(&outcome)),
            )
        })
        .collect();
    let suspensions = suspend::prepare(&prepared, &closures)
        .into_iter()
        .map(|(id, (function, error_variable, error_label))| {
            (
                adapter::id(id),
                SuspensionInput {
                    function: adapter::adapt_function(&function),
                    error_variable: adapter::id(error_variable),
                    error_label: adapter::id(error_label),
                },
            )
        })
        .collect();
    let result = LoweredModule {
        suspensions,
        lowered: adapter::program(&prepared, &const_function_groups),
        closures: ClosureMetadata {
            functions: closures
                .functions
                .iter()
                .map(|(id, environment)| (adapter::id(*id), *environment))
                .collect(),
            families: closures
                .families
                .iter()
                .map(|(id, (parent, ordinal))| (adapter::id(*id), (adapter::id(*parent), *ordinal)))
                .collect(),
            environments: closures
                .environments
                .iter()
                .map(|env| ClosureEnvironment {
                    parent: adapter::id(env.parent),
                    function_type: ir::TypeId::from_index(env.function_type.index()),
                    captures: env
                        .captures
                        .iter()
                        .map(|capture| CaptureBinding {
                            source: adapter::id(capture.source),
                            local: adapter::id(capture.local),
                            ty: ir::TypeId::from_index(capture.ty.index()),
                        })
                        .collect(),
                })
                .collect(),
        },
        const_groups: const_groups
            .iter()
            .map(|group| ConstGroup {
                canonical: adapter::id(group.canonical),
                specializations: group
                    .specializations
                    .iter()
                    .copied()
                    .map(adapter::id)
                    .collect(),
            })
            .collect(),
        runtime_consts: runtime_consts.into_iter().map(adapter::id).collect(),
        deferred_pool,
        emitted_functions: emitted_functions.into_iter().map(adapter::id).collect(),
        discarded_results: discarded_results
            .into_iter()
            .map(|(function, variables)| {
                (
                    adapter::id(function),
                    variables.into_iter().map(adapter::id).collect(),
                )
            })
            .collect(),
        const_outcomes,
    };
    Ok(result)
}

pub(in crate::compiler::lower) fn check_input_stage(
    stage: emela_lir::Stage,
) -> Result<(), Vec<Error>> {
    stages::verify(stage)
}
pub(in crate::compiler::lower) fn normalize_flow_types(
    flow: &mut emela_lir::Module,
) -> Result<(), Vec<Error>> {
    type_normalize::normalize(flow)
}
