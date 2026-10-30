//! C formatting of a complete, immutable backend plan.
use crate::contract::abi::{AggregateKind, CLayoutPlan as TypeLayouts};
use crate::contract::ir::*;
use crate::contract::lowered::ConstOutcome;
use crate::contract::strings::StringBuffers as StringBufferPlan;
use crate::semantics::abi::{LayoutRead, option_payload_type};
use crate::semantics::c::variable_name;
use crate::semantics::diagnostics::{
    self, invalid_flow_ir, statement_name, term_name, unsupported, value_name,
};
use crate::{Error, ErrorKind, Options};
use std::collections::{HashMap, HashSet};

use crate::contract::planned::PlannedModule;

pub(super) fn render(plan: &PlannedModule<'_>) -> Result<Output, Vec<Error>> {
    let options = &plan.options;
    let compiled = plan.input;
    let flow = &compiled.lowered;
    let closures = &compiled.closures;
    let runtime_consts = &compiled.runtime_consts;
    let layouts = &plan.layouts;
    let external_bindings = &plan.external_bindings;
    let const_bridges = &plan.const_bridges;
    let derived_plan = &plan.derived_plan;
    let emitted_functions = &plan.emitted_functions;
    let heap_functions = &plan.heap_functions;
    let string_output_functions = &plan.string_output_functions;
    let const_outcomes = &plan.const_outcomes;
    let names = &plan.names;
    let public_names = &plan.public_names;
    let runtime = &plan.runtime;
    let has_scheduler = plan.has_scheduler;
    let has_tasks = plan.has_tasks;
    let task_value_types = &plan.task_value_types;
    let heap_required = !heap_functions.is_empty();
    let main_entry = plan.main_entry.map(|id| (id, &flow.functions[id]));
    let target_headers = services::headers(flow, &plan.services)?;
    let function_ref_adapters = &plan.function_ref_adapters;
    let header = header::emit(header::Context {
        flow,
        layouts,
        emitted_functions,
        external_bindings,
        names,
        public_names,
        string_output_functions,
        task_value_types,
        has_scheduler,
        has_tasks,
        has_failing_tasks: plan.has_failing_tasks,
        heap_required,
        entry_declaration: main_entry.map(|_| "void emela_embedded_start(void);"),
    });

    let platform_header = platform::header(platform::Requirements {
        defects: runtime.defects,
        const_defects: !runtime_consts.is_empty(),
        lifecycle: main_entry.is_some(),
        debug: runtime.debug,
        event_lock: has_tasks,
    });

    let mut source = Writer::new();
    source.line(&format!("#include \"{HEADER_FILE}\""));
    source.line(&format!("#include \"{}\"", platform_header.path));
    source.line("#include <math.h>");
    source.line("#include <stddef.h>");
    for file in &target_headers {
        source.line(&format!("#include \"{}\"", file.path));
    }
    source.blank();
    c::emit_unused_attribute(&mut source);
    source.blank();
    if !runtime_consts.is_empty() {
        crate::compiler::emit::const_runtime::emit_runtime(
            plan.const_emergency_bytes,
            options,
            &mut source,
        )?;
    }
    if has_scheduler {
        if !task_value_types.is_empty() || plan.has_failing_tasks {
            source.line("#define EMELA_TASK_CONTEXT_RUNTIME 1");
        }
        if !task_value_types.is_empty() {
            task_values::emit_union(task_value_types, &mut source);
        }
        if plan.has_failing_tasks {
            source.line("#define EMELA_TASK_FAILURE_RUNTIME 1");
        }
        suspend::emit_runtime(
            &mut source,
            options,
            has_tasks,
            layouts.runtime.has_resumes,
            layouts.runtime.has_unwind,
            layouts.runtime.has_scopes,
            layouts.runtime.deferred_pool,
            layouts.runtime.has_error_abi,
        );
    }
    if layouts.runtime.has_scopes {
        if layouts.has_child_waits() {
            source.line("#define EMELA_CHILD_WAIT_RUNTIME");
        }
        source.line(include_str!("runtime/scope_types.c"));
    }
    if layouts.runtime.deferred_pool {
        source.line(include_str!("runtime/deferred_types.c"));
    }
    cause::emit_types(layouts, &mut source);
    error_runtime::emit_types(layouts, &mut source);
    emit_closure_environment_declarations(layouts, closures, &mut source);
    layouts.emit_runtime_value_declarations(&mut source);
    if heap_required {
        layouts.emit_list_node_declarations(&mut source);
        crate::compiler::emit::heap::emit_runtime(
            &mut source,
            options.heap_bytes(),
            layouts.has_cell_cycles(),
        );
        emit_closure_string_copy_helper(layouts, &mut source);
        bytes_runtime::emit(layouts, &mut source);
        error_runtime::emit_ownership_declarations(layouts, &mut source);
        emit_any_ownership_declarations(layouts, &mut source);
        layouts.emit_heap_ownership_helpers(&mut source);
        owned::emit_helpers(layouts, &mut source);
        error_runtime::emit_ownership(layouts, true, &mut source);
        emit_any_ownership(layouts, true, &mut source);
        if layouts.runtime.deferred_pool {
            source.line(&format!(
                "#define EMELA_DEFERRED_KIND UINT32_C({})",
                layouts.deferred_kind()
            ));
            source.line(include_str!("runtime/deferred.c"));
        }
        layouts.emit_heap_destroy_switch(&closures.environments, &mut source);
        layouts.emit_public_release_helpers(&mut source);
        layouts.emit_public_function_ownership_helpers(&mut source);
        crate::compiler::emit::heap::emit_public_retain(&mut source);
        crate::compiler::emit::heap::emit_public_release(&mut source);
        emit_public_string_ownership(&mut source);
        external::emit_ownership_helpers(flow, external_bindings, layouts, &mut source);
    } else {
        source.line(
            "static inline EMELA_UNUSED void emela_heap_retain(void *value) { (void)value; }",
        );
        source.line(
            "static inline EMELA_UNUSED void emela_heap_release(void *value) { (void)value; }",
        );
        error_runtime::emit_ownership(layouts, false, &mut source);
        emit_any_ownership(layouts, false, &mut source);
        layouts.emit_noheap_ownership_stubs(&mut source);
        source.blank();
    }
    if plan.has_failing_tasks {
        suspend::emit_task_failure_ownership(layouts, &mut source);
    }
    task_values::emit_dispatch(layouts, task_value_types, &mut source);
    cause::emit_runtime(layouts, &mut source);
    if layouts.runtime.has_resumes {
        suspend::emit_resume_runtime(layouts, &mut source);
    }
    if layouts.runtime.has_unwind {
        source.line(include_str!("runtime/unwind.c"));
    }
    if layouts.runtime.has_scopes {
        if layouts.has_child_waits() {
            source.line(include_str!("runtime/child_wait.c"));
            child::emit_runtime(layouts, &mut source)?;
        }
        suspend::emit_scope_runtime(layouts, &mut source)?;
    }
    cause::emit_attempts(layouts, &mut source)?;
    cause::emit_infallible_raise(layouts, &mut source);
    if layouts.runtime.deferred_pool {
        deferred::emit_runtime(layouts, &mut source)?;
    }
    runtime.emit_helpers(&mut source);
    derived_plan.emit(layouts, &mut source);

    for (id, function) in flow
        .functions
        .iter()
        .filter(|(id, _)| emitted_functions.contains(id))
    {
        let name = &names[&id];
        let takes_string_output = string_output_functions.contains(&id);
        if function.suspends {
            suspend::emit_declarations(
                layouts,
                function,
                name,
                closures.functions.get(&id).copied(),
                &mut source,
            )?;
        } else if function.external.is_some() {
            let prefix = if external_bindings[&id].adapted {
                ""
            } else {
                "extern "
            };
            source.line(&format!(
                "{prefix}{};",
                signature(
                    layouts,
                    function,
                    name,
                    external_bindings[&id].adapted,
                    function.fails && external_bindings[&id].adapted,
                    string_output_functions.contains(&id)
                )
            ));
        } else if !function.exported || function.fails {
            source.line(&format!(
                "{};",
                if closures.functions.contains_key(&id) {
                    closure_signature(layouts, function, name, true)
                } else {
                    signature(
                        layouts,
                        function,
                        name,
                        true,
                        function.fails,
                        takes_string_output,
                    )
                }
            ));
        }
    }
    for adapter in function_ref_adapters {
        let target = adapter.target;
        let function_type = adapter.function_type;
        source.line(&format!(
            "{};",
            function_ref_adapter_signature(
                layouts,
                function_type,
                &function_ref_adapter_name(target, function_type),
            )
        ));
    }
    for (index, adapter) in layouts.shapes.function_adapters.iter().enumerate() {
        source.line(&format!(
            "{};",
            function_ref_adapter_signature(
                layouts,
                adapter.destination,
                &function_coerce_adapter_name(index),
            )
        ));
    }
    source.blank();
    const_bridges.emit(flow, layouts, names, heap_required, &mut source)?;
    error_runtime::emit_display(
        layouts,
        names,
        string_output_functions,
        options.string_capacity(),
        &mut source,
    )?;

    for (id, function) in flow
        .functions
        .iter()
        .filter(|(id, _)| emitted_functions.contains(id))
    {
        if function.suspends {
            suspend::emit_function(
                flow,
                layouts,
                id,
                function,
                &names[&id],
                names,
                string_output_functions,
                closures.functions.get(&id).copied(),
                options,
                &mut source,
                plan.suspensions.get(&id),
            )?;
            continue;
        }

        if function.external.is_some() {
            if function.body.is_some() {
                return Err(vec![Error {
                    kind: ErrorKind::InvalidFlowIr,
                    message: format!(
                        "invalid Flow IR in function `{}`: external function has a body",
                        function.name
                    ),
                }]);
            }
            if function.derivation.is_some() {
                derive_runtime::emit_adapter(layouts, function, &names[&id], &mut source);
                continue;
            }
            if let Some(operation) = plan.services.get(&id) {
                services::emit_adapter(
                    flow,
                    operation,
                    layouts,
                    function,
                    &names[&id],
                    options.peripheral_buffer_capacity(),
                    &mut source,
                )?;
                continue;
            }
            external::emit_adapter(
                layouts,
                function,
                &external_bindings[&id],
                &names[&id],
                &mut source,
            );
            continue;
        }
        let Some(body) = function.body else {
            return Err(unsupported(&function.name, "function without a body"));
        };
        let sequence = &function.seqs[body];
        if const_bridges.getters.contains_key(&id) {
            const_bridges.emit_getter(id, flow, layouts, names, &mut source);
            continue;
        }
        let initializer_name = format!("{}_const_init", names[&id]);
        let is_runtime_const = runtime_consts.contains(&id);
        let name = if is_runtime_const {
            &initializer_name
        } else {
            &names[&id]
        };
        let is_static = is_runtime_const || !function.exported || function.fails;
        let takes_string_output = string_output_functions.contains(&id);
        let function_signature = if closures.functions.contains_key(&id) {
            closure_signature(layouts, function, name, is_static)
        } else {
            signature(
                layouts,
                function,
                name,
                is_static,
                function.fails,
                takes_string_output,
            )
        };
        source.begin_block(&function_signature);
        if function.constant {
            match &const_outcomes[&id] {
                ConstOutcome::Value(crate::contract::ir::Const::Unit) => source.ret(None),
                ConstOutcome::Value(value) => {
                    let expression = constant_expression(value).map_err(|kind| {
                        unsupported(&function.name, &format!("{kind} in a const value"))
                    })?;
                    let Some(expression) = expression else {
                        return Err(invalid_flow_ir(
                            &function.name,
                            "non-Unit const value has no C expression",
                        ));
                    };
                    source.ret(Some(&expression));
                }
                ConstOutcome::Defect { kind, message } => {
                    let length = u16::try_from(message.len()).map_err(|_| {
                        unsupported(
                            &function.name,
                            "a const defect message longer than the embedded C ABI supports",
                        )
                    })?;
                    source.line(&format!(
                        "emela_board_defect(EMELA_DEFECT_{kind}, {}, UINT16_C({length}));",
                        c_string_literal(message.as_bytes())
                    ));
                }
                ConstOutcome::SelfReference { message } => {
                    let length = u16::try_from(message.len()).map_err(|_| {
                        unsupported(
                            &function.name,
                            "a const defect message longer than the embedded C ABI supports",
                        )
                    })?;
                    source.line(&format!(
                        "{}(EMELA_DEFECT_PANIC, {}, UINT16_C({length}));",
                        if runtime_consts.is_empty() {
                            "emela_board_defect"
                        } else {
                            "emela_const_self_reference"
                        },
                        c_string_literal(message.as_bytes())
                    ));
                }
            }
            source.end_block();
            source.blank();
            continue;
        }
        if layouts.runtime.has_cause_raise && layouts.runtime.has_unwind {
            source.line("unsigned emela_entry_holds = emela_current_fiber == NULL ? 0 : emela_current_fiber->holds;");
            source.line("(void)emela_entry_holds;");
        }
        if function.fails {
            source.line("*emela_error_out = (EmelaError){0};");
        }
        if takes_string_output {
            source.decl(
                "uint16_t",
                "emela_emb_string_output_capacity",
                Some(&format!(
                    "emela_string_capacity < UINT16_C({}) ? emela_string_capacity : UINT16_C({})",
                    options.string_capacity(),
                    options.string_capacity()
                )),
            );
        }
        let string_buffers = &plan.strings[&id];
        emit_local_declarations(layouts, function, string_buffers, &mut source);
        if let Some(&environment) = layouts.functions.closure_functions.get(&id) {
            emit_closure_capture_loads(layouts, function, environment, &mut source);
        }
        let mut loops = Vec::new();
        let mut blocks = Vec::new();
        let mut protects = Vec::new();
        let mut next_temp = 0;
        let mut next_label = 0;
        for &statement in &sequence.stmts {
            emit_statement(
                layouts,
                function,
                &function.stmts[statement],
                &mut source,
                names,
                string_buffers,
                &mut loops,
                &mut blocks,
                &mut protects,
                &mut next_temp,
                &mut next_label,
            )?;
        }
        emit_return(
            layouts,
            function,
            &sequence.term,
            string_buffers,
            &mut source,
        )?;
        source.end_block();
        source.blank();
        if is_runtime_const {
            emit_const_getter(
                layouts,
                id,
                function,
                &names[&id],
                &initializer_name,
                &mut source,
            );
        }
    }
    for adapter in function_ref_adapters {
        let target = adapter.target;
        let function_type = adapter.function_type;
        let takes_output = adapter.takes_output;
        let adapter_capacity = adapter.capacity;
        emit_function_ref_adapter(
            layouts,
            function_type,
            &names[&target],
            &function_ref_adapter_name(target, function_type),
            takes_output,
            adapter_capacity,
            &mut source,
        );
        source.blank();
    }
    for (index, adapter) in layouts.shapes.function_adapters.iter().enumerate() {
        emit_function_coerce_adapter(layouts, adapter, index, &mut source);
        source.blank();
    }
    for (id, function) in flow
        .functions
        .iter()
        .filter(|(id, f)| emitted_functions.contains(id) && f.exported && f.fails && !f.suspends)
    {
        emit_fallible_wrapper(
            layouts,
            function,
            &public_names[&id],
            &names[&id],
            string_output_functions.contains(&id),
            &mut source,
        );
        source.blank();
    }
    if let Some((id, _)) = main_entry {
        entry::emit(plan, id, &mut source);
    }
    let mut output = Output {
        exports: plan.metadata.exports.clone(),
        files: vec![
            header,
            platform_header,
            GeneratedFile {
                path: SOURCE_FILE.to_owned(),
                contents: source.finish(),
            },
        ]
        .into_iter()
        .chain(target_headers)
        .collect(),
    };
    let mut alias_header = String::from(
        "#ifndef EMELA_EMBEDDED_EXPORTS_H\n#define EMELA_EMBEDDED_EXPORTS_H\n\n#include \"emela_embedded.h\"\n",
    );
    for (alias, symbol) in &plan.aliases {
        alias_header.push_str(&format!("\n#ifdef {alias}\n#error \"embedded export alias {alias} conflicts with an existing macro\"\n#endif\n#define {alias} {symbol}\n"));
    }
    alias_header.push_str("\n#endif\n");
    output.files.push(GeneratedFile {
        path: crate::artifact::ALIASES_HEADER.into(),
        contents: alias_header,
    });
    let mut manifest = crate::artifact::Manifest::from_files(&output.files)
        .map_err(|error| vec![Error::from(error)])?;
    manifest.metadata = plan.metadata.clone();
    output.files.push(GeneratedFile {
        path: crate::artifact::MANIFEST_FILE.into(),
        contents: manifest
            .to_json()
            .map_err(|error| vec![Error::from(error)])?,
    });
    output
        .refresh_manifest()
        .map_err(|error| vec![Error::from(error)])?;
    Ok(output)
}

mod control;
mod functions;
mod header;
mod values;

use control::{
    BlockTarget, ProtectTarget, emit_call_failure_edge, emit_compound_sequence,
    emit_error_assignment, emit_failure_return, emit_pending_cleanups, emit_return, emit_statement,
    emit_sync_interrupt_exit,
};
use functions::signature;
use functions::{
    closure_signature, emit_any_ownership, emit_any_ownership_declarations,
    emit_closure_capture_loads, emit_closure_environment_declarations,
    emit_closure_string_copy_helper, emit_const_getter, emit_fallible_wrapper,
    emit_function_coerce_adapter, emit_function_ref_adapter, emit_local_declarations,
    emit_owned_cleanup, emit_public_string_ownership, emit_release_if_owned, emit_retain_operand,
    function_ref_adapter_signature,
};
use values::{
    constant_expression, emit_aggregate_field_store, emit_builtin, emit_runtime,
    operand_expression, operand_is_float, operand_is_int, operand_is_int64, operand_is_string,
    operand_is_string_list, option_variant_expression, primitive_expression,
};

pub(super) mod layouts;

pub(super) mod string_buffers;

pub(super) mod const_bridge;
use const_bridge::Emission as ConstBridgePlanEmission;

pub(super) mod derive_runtime;
use derive_runtime::Emission as DerivationPlanEmission;

pub(super) mod runtime_helpers;
use runtime_helpers::RuntimeEmission;

pub(super) mod external;

mod features;
use crate::writer::Writer;
use crate::{GeneratedFile, Output};
use features::builtins::string_split::emit_string_split_push;
use features::values::coerce::emit_coerce;
mod c;
mod cause;
mod child;
mod deferred;
mod error_runtime;
mod numeric_runtime;
mod owned;
mod platform;
mod string_runtime;
mod suspend;
mod task_values;
use crate::contract::operations as analysis;
use c::*;
use layouts::LayoutEmission;
use string_buffers::StringBufferEmission;
const HEADER_FILE: &str = "emela_embedded.h";
const SOURCE_FILE: &str = "emela_embedded.c";

mod entry;
mod services;

mod bytes_runtime;
mod heap;

mod float_parse;

mod float_to_string;

mod const_runtime;

use crate::semantics::operands::operand_type;

mod unicode_grapheme;

#[cfg(test)]
mod tests {
    #[test]
    fn renders_a_contract_fixture_without_invoking_plan_or_lower() {
        let input = crate::contract::fixtures::lowered();
        let plan = crate::contract::fixtures::planned(&input);
        let first = super::render(&plan).unwrap();
        let second = super::render(&plan).unwrap();
        assert_eq!(first.files, second.files);
        assert!(
            first
                .files
                .iter()
                .any(|file| file.path == crate::artifact::MANIFEST_FILE)
        );
        assert!(first.exports.is_empty());
    }
}
