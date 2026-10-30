//! Resource and ABI planning over the independent lower contract.
use crate::contract::ExportedFunction;
use crate::contract::abi::{AggregateKind, FunctionAdapterLayout, LayoutBuilder as TypeLayouts};
use crate::contract::ir::*;
use crate::contract::lowered::{ConstOutcome, LoweredModule as CompiledModule};
use crate::contract::operations::*;
use crate::contract::strings::StringBuffers as StringBufferPlan;
use crate::semantics::abi::{LayoutRead, option_payload_type};
use crate::semantics::c::{self, function_name, is_c_identifier};
use crate::semantics::diagnostics::{self, invalid_flow_ir, unsupported, value_name};
use crate::{Error, ErrorKind, Options};
use std::collections::{HashMap, HashSet};
fn function_positions(function: &Function) -> (HashMap<StmtId, usize>, HashMap<SeqId, usize>) {
    (
        function.statement_positions.clone(),
        function.sequence_positions.clone(),
    )
}
fn runtime_statements(function: &Function) -> HashSet<StmtId> {
    function.active_statements.clone()
}

mod layouts;
use layouts::LayoutPreparation;

pub(in crate::compiler::plan) mod operations;
use operations as features;
pub(in crate::compiler::plan) mod analysis;
use analysis::{HeapContext, StringBehaviorRead};
pub(in crate::compiler::plan) mod heap;
pub(in crate::compiler::plan) mod suspend;

pub(in crate::compiler::plan) mod strings;
use strings::{StringPlanning, functions_with_string_output};

pub(in crate::compiler::plan) mod const_bridge;
use const_bridge::Preparation as ConstBridgePlanPreparation;
pub(in crate::compiler::plan) mod derive_runtime;
use derive_runtime::Preparation as DerivationPlanPreparation;
pub(in crate::compiler::plan) mod runtime;
use runtime::RuntimePreparation;
pub(in crate::compiler::plan) mod external;

pub(in crate::compiler::plan) mod validation;
use validation::{
    directly_calls, validate_external, validate_function_refs, validate_function_value_types,
};

mod adapters;
mod const_runtime;
pub(in crate::compiler::plan) mod task_values;

use crate::contract::auxiliary::{ConstBridgePlan, DerivationPlan};
use crate::contract::planned::{FunctionRefAdapter, PlannedModule};
use crate::contract::runtime::{RuntimeHelper, RuntimeRequirements};
use heap::function_needs_heap;

pub(super) fn prepare<'a>(
    compiled: &'a CompiledModule,
    options: &Options,
) -> Result<PlannedModule<'a>, Vec<Error>> {
    if options.peripheral_buffer_capacity() == 0 {
        return Err(vec![Error {
            kind: ErrorKind::InvalidOptions,
            message: "peripheral_buffer_capacity must be positive (uint16_t words)".into(),
        }]);
    }
    if !(1..=512).contains(&options.string_capacity()) {
        return Err(vec![Error {
            kind: ErrorKind::InvalidOptions,
            message: format!(
                "string capacity must be between 1 and 512 bytes, found {}",
                options.string_capacity()
            ),
        }]);
    }
    let lowered_flow = &compiled.lowered;
    let closures = &compiled.closures;
    let const_groups = &compiled.const_groups;
    let runtime_consts = &compiled.runtime_consts;
    let deferred_pool = compiled.deferred_pool;
    if deferred_pool && options.deferred_capacity() == 0 {
        return Err(vec![Error {
            kind: ErrorKind::InvalidOptions,
            message: "deferred_capacity must be positive for Deferred operations".into(),
        }]);
    }
    let discarded_results = compiled.discarded_results.clone();
    let flow = lowered_flow;
    let emitted_functions = compiled.emitted_functions.clone();
    let has_scheduler = suspend::validate(flow, &emitted_functions, options)?;
    let has_tasks = suspend::has_tasks(flow, &emitted_functions);
    let task_value_types = task_values::result_types(flow, &emitted_functions);
    let mut layouts = TypeLayouts::new(flow, &emitted_functions, closures, deferred_pool)?;
    layouts.functions.discarded_results = discarded_results;
    layouts.functions.runtime_consts = runtime_consts.clone();
    let const_bridges = ConstBridgePlan::new(flow, const_groups, closures, &mut layouts)?;
    for &id in runtime_consts {
        if matches!(layouts.shapes.types[flow.functions[id].ret], Type::String) {
            layouts
                .functions
                .owned_string_returns
                .insert(flow.functions[id].name.clone());
        }
    }
    let services = flow
        .functions
        .iter()
        .filter(|(id, _)| emitted_functions.contains(id))
        .filter_map(|(id, f)| match services::resolve(flow, f) {
            Ok(Some(operation)) => Some(Ok((id, operation))),
            Ok(None) => None,
            Err(errors) => Some(Err(errors)),
        })
        .collect::<Result<HashMap<_, _>, Vec<Error>>>()?;
    let external_bindings =
        external::resolve(flow, &emitted_functions, &mut layouts, options, &services)?;
    let derived_plan = DerivationPlan::new(flow, &emitted_functions, &layouts)?;
    task_values::validate_types(flow, &emitted_functions, &layouts)?;
    validate_function_value_types(&layouts, &emitted_functions, flow)?;
    validate_function_refs(flow, &emitted_functions, &layouts)?;
    let mut string_output_functions =
        functions_with_string_output(flow, &emitted_functions, &layouts);
    external::extend_string_output(
        flow,
        &emitted_functions,
        &external_bindings,
        &layouts,
        &mut string_output_functions,
    );
    layouts.functions.owned_aggregate_returns = flow
        .functions
        .iter()
        .filter(|(id, f)| {
            emitted_functions.contains(id)
                && !f.suspends
                && !f.constant
                && f.external.is_none()
                && !matches!(layouts.shapes.types[f.ret], Type::String)
                && layouts.has_borrowed_views(f.ret)
        })
        .filter(|(id, f)| {
            runtime_consts.contains(id)
                || layouts.has_byte_views(f.ret)
                || !StringBufferPlan::new(
                    &layouts,
                    *id,
                    f,
                    options.string_capacity(),
                    &string_output_functions,
                    false,
                )
                .declarations
                .is_empty()
        })
        .map(|(_, f)| f.name.clone())
        .collect();
    let empty_list_functions = const_runtime::empty_list_functions(flow);
    let heap_functions: HashSet<_> = flow
        .functions
        .iter()
        .filter_map(|(id, function)| {
            (emitted_functions.contains(&id)
                && (function_needs_heap(
                    flow,
                    id,
                    function,
                    &layouts,
                    closures,
                    &string_output_functions,
                    &empty_list_functions,
                ) || external::needs_heap(
                    function,
                    external_bindings.get(&id),
                    &layouts,
                    options,
                )))
            .then_some(id)
        })
        .collect();
    let heap_required = !heap_functions.is_empty();
    if options.heap_bytes() == 0 && heap_required {
        let (_, function) = flow
            .functions
            .iter()
            .find(|(id, _)| heap_functions.contains(id))
            .expect("heap requirement came from an emitted function");
        return Err(unsupported(
            &function.name,
            "heap-backed List, recursive value, shared Cell, closure environment, handler, or evidence table with heap_bytes = 0; configure Options::with_heap_bytes",
        ));
    }
    let mut const_outcomes = HashMap::new();
    for (id, function) in flow
        .functions
        .iter()
        .filter(|(id, function)| emitted_functions.contains(id) && function.constant)
    {
        match &compiled.const_outcomes[&id] {
            Ok(outcome) => {
                const_outcomes.insert(id, outcome.clone());
            }
            Err(reason) => {
                let caller = flow.functions.iter().find_map(|(caller_id, caller)| {
                    (emitted_functions.contains(&caller_id)
                        && !caller.constant
                        && directly_calls(caller, id))
                    .then_some(caller)
                });
                let location = caller.map_or(function.name.as_str(), |caller| caller.name.as_str());
                return Err(unsupported(
                    location,
                    &format!(
                        "const `{}` cannot be evaluated statically: {reason}",
                        function.name
                    ),
                ));
            }
        }
    }

    let mut names = HashMap::new();
    let mut public_names = HashMap::new();
    for (id, function) in flow
        .functions
        .iter()
        .filter(|(id, _)| emitted_functions.contains(id))
    {
        let name = if function.external.is_some() {
            if function.exported {
                return Err(unsupported(&function.name, "exported external functions"));
            }
            if function.suspends {
                suspend::external_symbol(function, options)?
            } else {
                let binding = &external_bindings[&id];
                if binding.adapted {
                    function_name(id, function)
                } else {
                    binding.symbol.clone()
                }
            }
        } else {
            function_name(id, function)
        };
        public_names.insert(id, name.clone());
        let implementation_name = if function.exported && function.fails && !function.suspends {
            format!("{name}_impl")
        } else {
            name
        };
        names.insert(id, implementation_name);
    }
    let isr_safe_symbols = interrupts::validate_isr_entries(
        flow,
        &emitted_functions,
        &heap_functions,
        &layouts,
        &names,
        options,
    )?;
    let main_entry = flow.functions.iter().find(|(id, function)| {
        emitted_functions.contains(id) && function.exported && function.name == options.entry()
    });
    if let Some((_, function)) = main_entry
        && !function.params.is_empty()
    {
        return Err(unsupported(
            &function.name,
            "an embedded main function with parameters",
        ));
    }

    let mut aliases = Vec::new();
    let mut alias_names = HashSet::new();
    for (alias, target) in options.export_aliases() {
        if !is_c_identifier(alias)
            || c::is_reserved_alias(alias)
            || c::is_c_keyword(alias)
            || !alias_names.insert(alias)
        {
            return Err(vec![Error {
                kind: ErrorKind::InvalidOptions,
                message: format!(
                    "invalid export alias `{alias}`: conflicts with a reserved C or generated identifier"
                ),
            }]);
        }
        let candidates: Vec<_> = flow
            .functions
            .iter()
            .filter(|(id, f)| emitted_functions.contains(id) && f.exported && f.name == *target)
            .collect();
        if candidates.len() != 1 {
            return Err(vec![Error {
                kind: ErrorKind::InvalidOptions,
                message: format!(
                    "export alias `{alias}` refers to {} public function `{target}`",
                    if candidates.is_empty() {
                        "unknown"
                    } else {
                        "ambiguous"
                    }
                ),
            }]);
        }
        if flow.functions.iter().any(|(_, f)| {
            f.external
                .as_ref()
                .and_then(|external| external.binding("c"))
                .is_some_and(|binding| crate::semantics::host_refs::symbol(&binding.host) == alias)
        }) || options
            .external_adapters()
            .iter()
            .any(|(source, adapter)| source == alias || adapter.symbol() == alias)
            || options
                .task_adapters()
                .iter()
                .any(|(source, symbol)| source == alias || symbol == alias)
        {
            return Err(vec![Error {
                kind: ErrorKind::InvalidOptions,
                message: format!("export alias `{alias}` conflicts with a board external symbol"),
            }]);
        }
        let (id, f) = candidates[0];
        aliases.push((
            alias.clone(),
            format!(
                "{}{}",
                public_names[&id],
                if f.suspends { "_start" } else { "" }
            ),
        ));
    }
    let mut runtime = RuntimeRequirements::collect(flow, &emitted_functions);
    derived_plan.require_runtime(&layouts, &mut runtime);
    if !string_output_functions.is_empty() {
        runtime.require(RuntimeHelper::StringOutput);
        runtime.defects = true;
    }
    runtime.defects |= !runtime_consts.is_empty()
        || has_scheduler
        || heap_required
        || external_bindings
            .iter()
            .any(|(&id, binding)| binding.adapted && flow.functions[id].fails)
        || external_bindings
            .values()
            .any(|binding| binding.result == crate::ExternalResult::StringBuffer)
        || const_outcomes.values().any(|outcome| {
            matches!(
                outcome,
                ConstOutcome::Defect { .. } | ConstOutcome::SelfReference { .. }
            )
        });

    let const_emergency_bytes = runtime_consts
        .iter()
        .map(|&id| "while evaluating const : ".len() + flow.functions[id].name.len())
        .sum::<usize>()
        + 64;
    if !runtime_consts.is_empty()
        && (const_emergency_bytes > usize::from(u16::MAX) || options.const_diagnostic_bytes() == 0)
    {
        return Err(vec![Error {kind:ErrorKind::InvalidOptions,message:"runtime const diagnostics require a nonzero diagnostic capacity and const context names fitting uint16_t".into()}]);
    }
    let exports: Vec<_> = flow
        .functions
        .iter()
        .filter(|(id, function)| emitted_functions.contains(id) && function.exported)
        .map(|(id, function)| ExportedFunction {
            source_name: function.name.clone(),
            c_symbol: if function.suspends {
                format!("{}_start", public_names[&id])
            } else {
                public_names[&id].clone()
            },
            suspends: function.suspends,
        })
        .collect();

    let mut required_services: Vec<_> = services
        .values()
        .map(|op| format!("service.{}", op.peripheral))
        .collect();
    if main_entry.is_some() {
        required_services.push("runtime.lifecycle".into());
    }
    if runtime.defects {
        required_services.push("runtime.defect".into());
    }
    if runtime.debug {
        required_services.push("runtime.debug".into());
    }
    if has_tasks {
        required_services.push("runtime.tasks".into());
    }
    required_services.sort();
    required_services.dedup();
    let mut external_symbols: Vec<_> = flow
        .functions
        .iter()
        .filter(|(id, f)| {
            emitted_functions.contains(id)
                && f.external.is_some()
                && !services.contains_key(id)
                && f.derivation.is_none()
                && !f.infallible_cause_fail
        })
        .map(|(id, f)| {
            if f.suspends {
                names[&id].clone()
            } else {
                external_bindings[&id].symbol.clone()
            }
        })
        .collect();
    external_symbols.sort();
    external_symbols.dedup();
    let metadata = crate::contract::artifact::Metadata {
        entry: main_entry.map(|_| "emela_embedded_start".into()),
        exports,
        services: required_services,
        external_symbols,
        isr_entries: options.isr_entries().to_vec(),
        isr_safe_symbols,
        task_capacity: if has_tasks {
            options.task_capacity()
        } else {
            0
        },
    };

    let mut strings = HashMap::new();
    for (id, function) in flow
        .functions
        .iter()
        .filter(|(id, _)| emitted_functions.contains(id))
    {
        let takes_output = string_output_functions.contains(&id);
        let buffers = StringBufferPlan::new(
            &layouts,
            id,
            function,
            options.string_capacity(),
            &string_output_functions,
            takes_output && !function.suspends,
        );
        if !function.suspends
            && !function.constant
            && function.external.is_none()
            && !const_bridges.getters.contains_key(&id)
            && !buffers.declarations.is_empty()
            && layouts.has_borrowed_views(function.ret)
            && !layouts
                .functions
                .owned_string_returns
                .contains(&function.name)
            && !layouts
                .functions
                .owned_aggregate_returns
                .contains(&function.name)
            && !takes_output
        {
            return Err(unsupported(
                &function.name,
                "a String scratch buffer that could outlive its function stack frame",
            ));
        }
        strings.insert(id, buffers);
    }
    let function_ref_adapters = adapters::function_ref_adapters(flow, &emitted_functions, &layouts)
        .into_iter()
        .map(|(target, function_type)| {
            let takes_output = string_output_functions.contains(&target);
            let function = &flow.functions[target];
            let capacity = if takes_output {
                let buffers = &strings[&target];
                function
                    .seqs
                    .iter()
                    .filter_map(|(_, seq)| match &seq.term {
                        Term::Return(Some(Operand::Var(variable))) => Some(
                            buffers
                                .buffers
                                .get(variable)
                                .map_or(options.string_capacity(), |(_, size)| *size),
                        ),
                        Term::Return(Some(Operand::Const(Const::String(value)))) => Some(
                            u16::try_from(value.len())
                                .unwrap_or(u16::MAX)
                                .min(options.string_capacity()),
                        ),
                        _ => None,
                    })
                    .max()
                    .unwrap_or(options.string_capacity())
                    .max(1)
            } else {
                options.string_capacity()
            };
            FunctionRefAdapter {
                target,
                function_type,
                takes_output,
                capacity,
            }
        })
        .collect();
    let main_entry = main_entry.map(|(id, _)| id);
    let has_failing_tasks = suspend::has_failing_tasks(flow, &emitted_functions);
    let mut suspensions = HashMap::new();
    for (id, function) in flow.functions.iter().filter(|(id, function)| {
        emitted_functions.contains(id) && function.suspends && function.external.is_none()
    }) {
        let _ = function;
        suspensions.insert(
            id,
            suspend::prepare_frame(
                &compiled.suspensions[&id],
                id,
                &layouts,
                &string_output_functions,
                closures.functions.get(&id).copied(),
                options,
            )?,
        );
    }
    let layouts = layouts::freeze(layouts);
    Ok(PlannedModule {
        metadata,
        aliases,
        const_emergency_bytes,
        options: options.clone(),
        services,
        suspensions,
        input: compiled,
        layouts,
        external_bindings,
        const_bridges,
        derived_plan,
        emitted_functions,
        heap_functions,
        string_output_functions,
        const_outcomes,
        names,
        public_names,
        strings,
        function_ref_adapters,
        main_entry,
        has_scheduler,
        has_tasks,
        has_failing_tasks,
        task_value_types,
        runtime,
    })
}

mod interrupts;
mod services;

use crate::semantics::c::type_name;
use crate::semantics::{child, deferred, operands::operand_type};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plans_an_independent_contract_without_flow_or_lower_implementation() {
        let mut input = crate::contract::fixtures::lowered();
        let mut function = Function::new("answer");
        function.exported = true;
        function.ret = TypeId::INT;
        function.body = Some(function.seqs.alloc(Seq {
            stmts: vec![],
            term: Term::Return(Some(Operand::Const(Const::Int(42)))),
        }));
        let id = input.lowered.functions.alloc(function);
        input.emitted_functions.insert(id);
        let planned = prepare(&input, &Options::default()).unwrap();
        assert_eq!(planned.public_names.len(), 1);
        assert!(!planned.has_scheduler);
        assert!(planned.heap_functions.is_empty());
        assert!(planned.layouts.properties.is_some());
    }
}
