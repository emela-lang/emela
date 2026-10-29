use std::collections::{HashMap, HashSet};

use emela_core::{Builtin, CtorRef, EffectId, EnumId, Type, TypeId, TypeTable};
use emela_lir::{
    Callee, CaseKey, Const, FnId, Function, LabelId, OnFail, Operand, PrimOp, PrimTy, Repr,
    RuntimeOp, SeqId, Stmt, StmtId, SwitchOn, Term, Value, VarId,
};

use crate::const_eval::{self, Outcome as ConstOutcome};
use crate::writer::Writer;
use crate::{Error, ErrorKind, ExportedFunction, GeneratedFile, Options, Output};

#[path = "cause.rs"]
mod cause;
#[path = "child.rs"]
mod child;
#[path = "const_bridge.rs"]
mod const_bridge;
#[path = "deferred.rs"]
mod deferred;
#[path = "derive_runtime.rs"]
mod derive_runtime;
#[path = "error.rs"]
mod error_runtime;
#[path = "external.rs"]
mod external;
#[path = "owned.rs"]
mod owned;
#[path = "peripheral.rs"]
mod peripheral;
#[path = "suspend.rs"]
mod suspend;
#[path = "task_values.rs"]
mod task_values;

const HEADER_FILE: &str = "emela_embedded.h";
const SOURCE_FILE: &str = "emela_embedded.c";
const BOARD_HEADER_FILE: &str = "emela_board.h";

pub(crate) fn emit(flow: &emela_lir::Module, options: &Options) -> Result<Output, Vec<Error>> {
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
    // The input Flow IR was verified by compile_flow before backend lowering. Lifted closure
    // captures live in backend metadata, so the transformed module no longer satisfies Flow IR's
    // function-local variable invariant and cannot be verified at the Flow stage again.
    let const_groups = crate::const_origins::groups(flow)?;
    crate::const_origins::function_groups(flow)?;
    let mut prepared_flow = flow.clone();
    external::normalize_exports(&mut prepared_flow);
    const_bridge::prepare(&mut prepared_flow, &const_groups);
    let mut runtime_consts = crate::const_runtime::prepare(&mut prepared_flow, runtime_reachable);
    runtime_consts.extend(
        const_groups
            .iter()
            .flat_map(|group| group.specializations.iter().copied()),
    );

    crate::scope_abi::normalize(&mut prepared_flow)?;
    crate::fiber::normalize_scope_results(&mut prepared_flow);
    let mut closures = crate::static_closure::lower(&prepared_flow)?;
    let mut lowered_flow = closures
        .module
        .take()
        .expect("closure lowering returns its Flow IR module");
    crate::fiber::erase_inert_failures(&mut lowered_flow, &mut closures);
    let closure_functions: HashSet<_> = closures.functions.keys().copied().collect();
    crate::function_coerce::lower(&mut lowered_flow, &closure_functions);
    let deferred_pool = crate::fiber::needs_deferred_pool(&lowered_flow, &closures);
    if deferred_pool && options.deferred_capacity == 0 {
        return Err(vec![Error {
            kind: ErrorKind::InvalidOptions,
            message: "deferred_capacity must be positive for Deferred operations".into(),
        }]);
    }
    crate::static_handler::lower(&mut lowered_flow);
    let discarded_results = crate::discarded::normalize(&mut lowered_flow);
    let flow = &lowered_flow;
    let emitted_functions = runtime_reachable(flow);
    let has_scheduler = suspend::validate(flow, &emitted_functions, options)?;
    let has_tasks = suspend::has_tasks(flow, &emitted_functions);
    let task_value_types = task_values::result_types(flow, &emitted_functions);
    let mut layouts = TypeLayouts::new(flow, &emitted_functions, &closures, deferred_pool)?;
    layouts.discarded_results = discarded_results;
    layouts.runtime_consts = runtime_consts.clone();
    let const_bridges = const_bridge::Bridges::new(flow, &const_groups, &closures, &mut layouts)?;
    for &id in &runtime_consts {
        if matches!(layouts.types[flow.functions[id].ret], Type::String) {
            layouts
                .owned_string_returns
                .insert(flow.functions[id].name.clone());
        }
    }
    let external_bindings = external::resolve(flow, &emitted_functions, &mut layouts, options)?;
    let peripheral_headers = peripheral::headers(flow, &emitted_functions)?;
    let derived_plan = derive_runtime::Plan::new(flow, &emitted_functions, &layouts)?;
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
    layouts.owned_aggregate_returns = flow
        .functions
        .iter()
        .filter(|(id, f)| {
            emitted_functions.contains(id)
                && !f.suspends
                && !f.constant
                && f.external.is_none()
                && !matches!(layouts.types[f.ret], Type::String)
                && layouts.has_inline_string(f.ret)
        })
        .filter(|(id, f)| {
            runtime_consts.contains(id)
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
    let empty_list_functions = crate::const_runtime::empty_list_functions(flow);
    let heap_functions: HashSet<_> = flow
        .functions
        .iter()
        .filter_map(|(id, function)| {
            (emitted_functions.contains(&id)
                && (function_needs_heap(
                    id,
                    function,
                    &layouts,
                    &closures,
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
        match const_eval::evaluate(flow, id) {
            Ok(outcome) => {
                const_outcomes.insert(id, outcome);
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
    validate_isr_entries(
        flow,
        &emitted_functions,
        &heap_functions,
        &layouts,
        &names,
        options,
    )?;
    let main_entry = flow.functions.iter().find(|(id, function)| {
        emitted_functions.contains(id) && function.exported && function.name == "main"
    });
    if let Some((_, function)) = main_entry
        && !function.params.is_empty()
    {
        return Err(unsupported(
            &function.name,
            "an embedded main function with parameters",
        ));
    }

    let mut header = Writer::new();
    header.line("#ifndef EMELA_EMBEDDED_H");
    header.line("#define EMELA_EMBEDDED_H");
    header.blank();
    header.line("#include <stdint.h>");
    header.blank();
    layouts.emit_string_declaration(&mut header);
    if has_scheduler
        || layouts
            .function_types
            .iter()
            .any(|&ty| matches!(&layouts.types[ty], Type::Fn(ft) if ft.suspends))
    {
        header.line("typedef struct emela_fiber emela_fiber;");
    }
    if layouts.has_error_abi || layouts.any_abi {
        header.line("typedef struct EmelaError EmelaError;");
        if layouts.any_abi {
            header.line("typedef struct EmelaError EmelaAny;");
        }
        header.blank();
    }
    layouts.emit_declarations(&mut header);
    external::emit_header(
        flow,
        &external_bindings,
        &layouts,
        heap_required,
        &mut header,
    );
    if has_scheduler {
        header.line(include_str!("runtime/scheduler.h"));
    }
    if has_tasks {
        header.line("#define EMELA_HAS_TASK_RUNTIME 1");
        header.line(include_str!("runtime/task.h"));
        if !task_value_types.is_empty() || suspend::has_failing_tasks(flow, &emitted_functions) {
            header.line(include_str!("runtime/task_context.h"));
        }
        if suspend::has_failing_tasks(flow, &emitted_functions) {
            header.line(include_str!("runtime/task_failure.h"));
        }
        task_values::emit_header(&layouts, &task_value_types, &mut header);
        for (id, function) in flow
            .functions
            .iter()
            .filter(|(id, f)| emitted_functions.contains(id) && f.suspends && f.external.is_some())
        {
            if !suspend::scalar(&layouts.types, function.ret) {
                header.line(&format!("/* Success: emela_task_complete_value_{}. Arguments are borrowed only during start. */", function.ret.index()));
            }
            header.line(&format!(
                "{};",
                suspend::task_signature(&layouts, function, &names[&id])
            ));
        }
    }
    if !layouts.aggregates.is_empty() {
        header.blank();
    }

    for (id, function) in flow
        .functions
        .iter()
        .filter(|(id, f)| emitted_functions.contains(id) && f.exported)
    {
        if function.suspends {
            header.line(&format!(
                "{};",
                suspend::start_signature(&layouts, function, &public_names[&id])
            ));
            continue;
        }
        let name = &public_names[&id];
        let takes_string_output = string_output_functions.contains(&id);
        if takes_string_output {
            header.line("/* Caller provides this output buffer; the returned EmelaString points into it. */");
        }
        header.line(&format!(
            "{};",
            signature(&layouts, function, name, false, false, takes_string_output)
        ));
    }
    if main_entry.is_some() {
        header.line("void emela_embedded_start(void);");
    }
    if heap_required {
        header.line("/* Retains a value allocated from the fixed heap. */");
        header.line("void emela_embedded_retain(void *value);");
        header.line("/* Releases a value allocated from the fixed heap. */");
        header.line("void emela_embedded_release(void *value);");
        header.line("/* Retains the backing owner of an EmelaString value. */");
        header.line("void emela_embedded_retain_string(const EmelaString *value);");
        header.line("/* Releases the backing owner of an EmelaString value. */");
        header.line("void emela_embedded_release_string(EmelaString *value);");
        layouts.emit_public_release_declarations(&mut header);
        layouts.emit_public_function_ownership_declarations(&mut header);
    }

    header.blank();
    header.line("#endif");

    let mut board_header = Writer::new();
    board_header.line("#ifndef EMELA_BOARD_H");
    board_header.line("#define EMELA_BOARD_H");
    board_header.blank();
    board_header.line("#include <stdint.h>");
    board_header.blank();
    board_header.line("typedef enum {");
    for (index, kind) in [
        "PANIC",
        "TODO",
        "DIVISION_BY_ZERO",
        "NO_MATCH",
        "FLOAT_TO_INT",
        "NEGATIVE_SHIFT",
        "HEAP_EXHAUSTED",
        "STRING_CAPACITY",
        "UNSUPPORTED_UNICODE",
        "UNHANDLED_ERROR",
        "ASSERT",
    ]
    .iter()
    .enumerate()
    {
        let suffix = if index == 10 { "" } else { "," };
        board_header.line(&format!("  EMELA_DEFECT_{kind}{suffix}"));
    }
    board_header.line("} EmelaDefectKind;");
    board_header.blank();
    let needs_defect_hook = !runtime_consts.is_empty()
        || has_scheduler
        || external_bindings
            .iter()
            .any(|(&id, binding)| binding.adapted && flow.functions[id].fails)
        || external_bindings
            .values()
            .any(|binding| binding.result == crate::ExternalResult::StringBuffer)
        || heap_required
        || has_prim(
            flow,
            &emitted_functions,
            PrimTy::I32,
            &[PrimOp::Div, PrimOp::Rem],
        )
        || has_prim(
            flow,
            &emitted_functions,
            PrimTy::I64,
            &[PrimOp::Div, PrimOp::Rem],
        )
        || const_outcomes.values().any(|outcome| {
            matches!(
                outcome,
                ConstOutcome::Defect { .. } | ConstOutcome::SelfReference { .. }
            )
        })
        || has_runtime_string_append(flow, &emitted_functions)
        || has_runtime_string_show(flow, &emitted_functions)
        || has_runtime_int_string(flow, &emitted_functions)
        || has_runtime_float_to_string(flow, &emitted_functions)
        || !string_output_functions.is_empty()
        || has_builtin(flow, &emitted_functions, &[Builtin::StringLength])
        || has_builtin(flow, &emitted_functions, &[Builtin::StringJoin])
        || has_indirect_call(flow, &emitted_functions)
        || has_runtime_string_from_code_point(flow, &emitted_functions)
        || has_builtin(
            flow,
            &emitted_functions,
            &[
                Builtin::FloatFloor,
                Builtin::FloatCeil,
                Builtin::FloatRound,
                Builtin::FloatTruncate,
            ],
        )
        || has_builtin(
            flow,
            &emitted_functions,
            &[
                Builtin::IntShiftLeft,
                Builtin::IntShiftRight,
                Builtin::IntShiftRightUnsigned,
                Builtin::Int64ShiftLeft,
                Builtin::Int64ShiftRight,
                Builtin::Int64ShiftRightUnsigned,
            ],
        )
        || flow.functions.iter().any(|(id, function)| {
            if !emitted_functions.contains(&id) {
                return false;
            }
            if function.exported && function.fails && !function.suspends {
                return true;
            }
            !function.constant
                && (function.seqs.iter().any(|(_, sequence)| {
                    matches!(&sequence.term, Term::Defect(_, _) | Term::Unreachable)
                }) || function.stmts.iter().any(|(_, statement)| {
                    matches!(
                        statement,
                        Stmt::Switch {
                            on: SwitchOn::Tag(_),
                            default: None,
                            ..
                        }
                    )
                }))
        });
    if needs_defect_hook {
        board_header.line(
            "_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length);",
        );
    }
    if !runtime_consts.is_empty() {
        board_header.line("/* Reports a defect through active const contexts; direct board hook calls bypass them. */");
        board_header.line("_Noreturn void emela_embedded_report_defect(EmelaDefectKind kind, const char *message, uint16_t length);");
    }
    if main_entry.is_some() {
        board_header.line("void emela_board_init(void);");
        board_header.line("void emela_board_idle(void);");
    }
    if has_runtime_op(flow, &emitted_functions, RuntimeOp::DebugPrint) {
        board_header.line("void emela_board_debug(const char *bytes, uint16_t length);");
    }
    if has_tasks {
        board_header.line("/* Save/restore interrupt state and serialize task/event access; callable from ISR. */");
        board_header.line("uint32_t emela_board_event_lock(void);");
        board_header.line("void emela_board_event_unlock(uint32_t previous_state);");
    }
    board_header.blank();
    board_header.line("#endif");

    let mut source = Writer::new();
    source.line(&format!("#include \"{HEADER_FILE}\""));
    source.line(&format!("#include \"{BOARD_HEADER_FILE}\""));
    source.line("#include <math.h>");
    source.line("#include <stddef.h>");
    for file in &peripheral_headers {
        source.line(&format!("#include \"{}\"", file.path));
    }
    source.blank();
    emit_unused_attribute(&mut source);

    source.blank();
    if !runtime_consts.is_empty() {
        crate::const_runtime::emit_runtime(flow, &runtime_consts, options, &mut source)?;
    }
    if has_scheduler {
        if !task_value_types.is_empty() || suspend::has_failing_tasks(flow, &emitted_functions) {
            source.line("#define EMELA_TASK_CONTEXT_RUNTIME 1");
        }
        if !task_value_types.is_empty() {
            task_values::emit_union(&task_value_types, &mut source);
        }
        if suspend::has_failing_tasks(flow, &emitted_functions) {
            source.line("#define EMELA_TASK_FAILURE_RUNTIME 1");
        }
        suspend::emit_runtime(
            &mut source,
            options,
            has_tasks,
            layouts.has_resumes,
            layouts.has_unwind,
            layouts.has_scopes,
            layouts.deferred_pool,
            layouts.has_error_abi,
        );
    }
    if layouts.has_scopes {
        if layouts.has_child_waits() {
            source.line("#define EMELA_CHILD_WAIT_RUNTIME");
        }
        source.line(include_str!("runtime/scope_types.c"));
    }
    if layouts.deferred_pool {
        source.line(include_str!("runtime/deferred_types.c"));
    }
    cause::emit_types(&layouts, &mut source);
    error_runtime::emit_types(&layouts, &mut source);
    emit_closure_environment_declarations(&layouts, &closures, &mut source);
    layouts.emit_runtime_value_declarations(&mut source);
    if heap_required {
        layouts.emit_list_node_declarations(&mut source);
        crate::heap::emit_runtime(&mut source, options.heap_bytes(), layouts.has_cell_cycles());
        emit_closure_string_copy_helper(&layouts, &mut source);
        error_runtime::emit_ownership_declarations(&layouts, &mut source);
        emit_any_ownership_declarations(&layouts, &mut source);
        layouts.emit_heap_ownership_helpers(&mut source);
        owned::emit_helpers(&layouts, &mut source);
        error_runtime::emit_ownership(&layouts, true, &mut source);
        emit_any_ownership(&layouts, true, &mut source);
        if layouts.deferred_pool {
            source.line(&format!(
                "#define EMELA_DEFERRED_KIND UINT32_C({})",
                layouts.deferred_kind()
            ));
            source.line(include_str!("runtime/deferred.c"));
        }
        layouts.emit_heap_destroy_switch(&closures.environments, &mut source);
        layouts.emit_public_release_helpers(&mut source);
        layouts.emit_public_function_ownership_helpers(&mut source);
        crate::heap::emit_public_retain(&mut source);
        crate::heap::emit_public_release(&mut source);
        emit_public_string_ownership(&mut source);
        external::emit_ownership_helpers(flow, &external_bindings, &layouts, &mut source);
    } else {
        source.line(
            "static inline EMELA_UNUSED void emela_heap_retain(void *value) { (void)value; }",
        );
        source.line(
            "static inline EMELA_UNUSED void emela_heap_release(void *value) { (void)value; }",
        );
        error_runtime::emit_ownership(&layouts, false, &mut source);
        emit_any_ownership(&layouts, false, &mut source);
        layouts.emit_noheap_ownership_stubs(&mut source);
        source.blank();
    }
    if suspend::has_failing_tasks(flow, &emitted_functions) {
        suspend::emit_task_failure_ownership(&layouts, &mut source);
    }
    task_values::emit_dispatch(&layouts, &task_value_types, &mut source);
    cause::emit_runtime(&layouts, &mut source);
    if layouts.has_resumes {
        suspend::emit_resume_runtime(&layouts, &mut source);
    }
    if layouts.has_unwind {
        source.line(include_str!("runtime/unwind.c"));
    }
    if layouts.has_scopes {
        if layouts.has_child_waits() {
            source.line(include_str!("runtime/child_wait.c"));
            child::emit_runtime(&layouts, &mut source)?;
        }
        suspend::emit_scope_runtime(&layouts, &mut source)?;
    }
    cause::emit_attempts(&layouts, &mut source)?;
    cause::emit_infallible_raise(&layouts, &mut source);
    if layouts.deferred_pool {
        deferred::emit_runtime(&layouts, &mut source)?;
    }
    if has_runtime_string_comparison(flow, &emitted_functions) {
        emit_string_comparison_helper(&mut source);
    }
    let has_int_shift = has_builtin(
        flow,
        &emitted_functions,
        &[
            Builtin::IntShiftLeft,
            Builtin::IntShiftRight,
            Builtin::IntShiftRightUnsigned,
        ],
    );
    let has_i64_shift = has_builtin(
        flow,
        &emitted_functions,
        &[
            Builtin::Int64ShiftLeft,
            Builtin::Int64ShiftRight,
            Builtin::Int64ShiftRightUnsigned,
        ],
    );
    let has_shift = has_int_shift || has_i64_shift;
    if has_prim(
        flow,
        &emitted_functions,
        PrimTy::I32,
        &[PrimOp::Add, PrimOp::Sub, PrimOp::Mul, PrimOp::Neg],
    ) || has_int_shift
        || has_builtin(flow, &emitted_functions, &[Builtin::Int64ToInt])
    {
        source.begin_block("static inline EMELA_UNUSED int32_t emela_i32_from_bits(uint32_t bits)");
        source.begin_block("if (bits <= (uint32_t)INT32_MAX)");
        source.line("return (int32_t)bits;");
        source.end_block();
        source.line("return -INT32_C(1) - (int32_t)(UINT32_MAX - bits);");
        source.end_block();
        source.blank();
    }
    if has_int_shift {
        emit_i32_shift_helpers(&mut source);
    }
    if has_shift {
        emit_negative_shift_helper(&mut source);
    }
    if has_prim(
        flow,
        &emitted_functions,
        PrimTy::I32,
        &[PrimOp::Div, PrimOp::Rem],
    ) {
        emit_i32_division_helpers(&mut source);
    }
    if has_prim(
        flow,
        &emitted_functions,
        PrimTy::I64,
        &[PrimOp::Add, PrimOp::Sub, PrimOp::Mul, PrimOp::Neg],
    ) || has_i64_shift
    {
        emit_i64_wrap_helper(&mut source);
    }
    if has_i64_shift {
        emit_i64_shift_helpers(&mut source);
    }
    if has_prim(
        flow,
        &emitted_functions,
        PrimTy::I64,
        &[PrimOp::Div, PrimOp::Rem],
    ) {
        emit_i64_division_helpers(&mut source);
    }
    if has_builtin(
        flow,
        &emitted_functions,
        &[
            Builtin::FloatFloor,
            Builtin::FloatCeil,
            Builtin::FloatRound,
            Builtin::FloatTruncate,
        ],
    ) {
        emit_float_to_int_helper(&mut source);
    }
    if has_dynamic_defect(flow, &emitted_functions) {
        emit_dynamic_defect_kind_helper(&mut source);
    }
    if has_builtin(
        flow,
        &emitted_functions,
        &[
            Builtin::StringContains,
            Builtin::StringStartsWith,
            Builtin::StringEndsWith,
        ],
    ) {
        emit_string_matches_at_helper(&mut source);
    }
    if has_builtin(flow, &emitted_functions, &[Builtin::StringContains]) {
        emit_string_contains_helper(&mut source);
    }
    if has_builtin(flow, &emitted_functions, &[Builtin::StringStartsWith]) {
        emit_string_starts_with_helper(&mut source);
    }
    if has_builtin(flow, &emitted_functions, &[Builtin::StringEndsWith]) {
        emit_string_ends_with_helper(&mut source);
    }
    let grapheme_runtime = has_grapheme_runtime(flow, &emitted_functions);
    if grapheme_runtime
        || has_builtin(
            flow,
            &emitted_functions,
            &[Builtin::StringTrim, Builtin::StringCodePoints],
        )
    {
        emit_utf8_decode_helper(&mut source);
    }
    if grapheme_runtime {
        crate::unicode_grapheme::emit_runtime(&mut source);
    }
    if has_builtin(flow, &emitted_functions, &[Builtin::StringTrim]) {
        emit_string_trim_helper(&mut source);
    }
    if has_builtin(flow, &emitted_functions, &[Builtin::StringJoin]) {
        emit_string_join_helper(&mut source);
    }
    if has_runtime_string_append(flow, &emitted_functions) {
        emit_string_append_helper(&mut source);
    }
    if has_runtime_string_show(flow, &emitted_functions) || derived_plan.show {
        emit_string_show_helpers(&mut source);
    }
    if has_runtime_int_string(flow, &emitted_functions) || derived_plan.show {
        emit_int_to_string_helper(&mut source);
    }
    if has_runtime_int_parse(flow, &emitted_functions) {
        emit_int_parse_helper(&mut source);
    }
    if has_runtime_float_parse(flow, &emitted_functions) {
        crate::float_parse::emit_runtime(&mut source);
    }
    if has_runtime_float_to_string(flow, &emitted_functions) || derived_plan.needs_float(&layouts) {
        crate::float_to_string::emit_runtime(&mut source);
    }
    if !string_output_functions.is_empty() {
        emit_string_output_helper(&mut source);
    }
    derived_plan.emit(&layouts, &mut source);

    for (id, function) in flow
        .functions
        .iter()
        .filter(|(id, _)| emitted_functions.contains(id))
    {
        let name = &names[&id];
        let takes_string_output = string_output_functions.contains(&id);
        if function.suspends {
            suspend::emit_declarations(
                &layouts,
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
                    &layouts,
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
                    closure_signature(&layouts, function, name, true)
                } else {
                    signature(
                        &layouts,
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
    let function_ref_adapters = function_ref_adapters(flow, &emitted_functions, &layouts);
    for &(target, function_type) in &function_ref_adapters {
        source.line(&format!(
            "{};",
            function_ref_adapter_signature(
                &layouts,
                function_type,
                &function_ref_adapter_name(target, function_type),
            )
        ));
    }
    for (index, adapter) in layouts.function_adapters.iter().enumerate() {
        source.line(&format!(
            "{};",
            function_ref_adapter_signature(
                &layouts,
                adapter.destination,
                &function_coerce_adapter_name(index),
            )
        ));
    }
    source.blank();
    const_bridges.emit(flow, &layouts, &names, heap_required, &mut source)?;
    error_runtime::emit_display(
        &layouts,
        &names,
        &string_output_functions,
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
                &layouts,
                id,
                function,
                &names[&id],
                &names,
                &string_output_functions,
                closures.functions.get(&id).copied(),
                options,
                &mut source,
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
            if crate::derived::kind(function).is_some() {
                derive_runtime::emit_adapter(&layouts, function, &names[&id], &mut source);
                continue;
            }
            if peripheral::emit_adapter(
                flow,
                &layouts,
                function,
                &names[&id],
                options.peripheral_buffer_capacity(),
                &mut source,
            )? {
                continue;
            }
            external::emit_adapter(
                &layouts,
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
            const_bridges.emit_getter(id, flow, &layouts, &names, &mut source);
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
            closure_signature(&layouts, function, name, is_static)
        } else {
            signature(
                &layouts,
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
                ConstOutcome::Value(emela_lir::Const::Unit) => source.ret(None),
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
        if layouts.has_cause_raise && layouts.has_unwind {
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
        let string_buffers = StringBufferPlan::new(
            &layouts,
            id,
            function,
            options.string_capacity(),
            &string_output_functions,
            takes_string_output,
        );
        if !string_buffers.declarations.is_empty()
            && layouts.has_inline_string(function.ret)
            && !layouts.owned_string_returns.contains(&function.name)
            && !layouts.owned_aggregate_returns.contains(&function.name)
            && !takes_string_output
        {
            return Err(unsupported(
                &function.name,
                "a String scratch buffer that could outlive its function stack frame",
            ));
        }
        emit_local_declarations(&layouts, function, &string_buffers, &mut source);
        if let Some(&environment) = layouts.closure_functions.get(&id) {
            emit_closure_capture_loads(&layouts, function, environment, &mut source);
        }
        let mut loops = Vec::new();
        let mut blocks = Vec::new();
        let mut protects = Vec::new();
        let mut next_temp = 0;
        let mut next_label = 0;
        for &statement in &sequence.stmts {
            emit_statement(
                &layouts,
                function,
                &function.stmts[statement],
                &mut source,
                &names,
                &string_buffers,
                &mut loops,
                &mut blocks,
                &mut protects,
                &mut next_temp,
                &mut next_label,
            )?;
        }
        emit_return(
            &layouts,
            function,
            &sequence.term,
            &string_buffers,
            &mut source,
        )?;
        source.end_block();
        source.blank();
        if is_runtime_const {
            emit_const_getter(
                &layouts,
                id,
                function,
                &names[&id],
                &initializer_name,
                &mut source,
            );
        }
    }
    for &(target, function_type) in &function_ref_adapters {
        let target_function = &flow.functions[target];
        let takes_output = string_output_functions.contains(&target);
        let adapter_capacity = if takes_output {
            let plan = StringBufferPlan::new(
                &layouts,
                target,
                target_function,
                options.string_capacity(),
                &string_output_functions,
                true,
            );
            target_function
                .seqs
                .iter()
                .filter_map(|(_, seq)| match &seq.term {
                    Term::Return(Some(Operand::Var(var))) => Some(
                        plan.buffers
                            .get(var)
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
        emit_function_ref_adapter(
            &layouts,
            function_type,
            &names[&target],
            &function_ref_adapter_name(target, function_type),
            takes_output,
            adapter_capacity,
            &mut source,
        );
        source.blank();
    }
    for (index, adapter) in layouts.function_adapters.iter().enumerate() {
        emit_function_coerce_adapter(&layouts, adapter, index, &mut source);
        source.blank();
    }
    for (id, function) in flow
        .functions
        .iter()
        .filter(|(id, f)| emitted_functions.contains(id) && f.exported && f.fails && !f.suspends)
    {
        emit_fallible_wrapper(
            &layouts,
            function,
            &public_names[&id],
            &names[&id],
            string_output_functions.contains(&id),
            &mut source,
        );
        source.blank();
    }
    if let Some((id, function)) = main_entry {
        let main_name = &public_names[&id];
        source.begin_block("void emela_embedded_start(void)");
        source.line("emela_board_init();");
        if function.suspends {
            let args = if layouts.type_repr(function.ret) == Repr::Void {
                String::new()
            } else {
                source.decl(&layouts.c_type(function.ret), "result", Some("{0}"));
                "&result".to_owned()
            };
            source.line(&format!(
                "EmelaFiber main_fiber = {main_name}_start({args});"
            ));
            source.begin_block("while (!emela_fiber_done(main_fiber))");
            source.line("(void)emela_scheduler_run(UINT32_C(64));");
            source.line("emela_board_idle();");
            source.end_block();
            source.line("emela_fiber_release(main_fiber);");
        } else {
            source.line(&format!("(void){main_name}();"));
        }
        source.begin_block("for (;;)");
        source.line("emela_board_idle();");
        source.end_block();
        source.end_block();
        source.blank();
    }

    Ok(Output {
        exports: flow
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
            .collect(),
        files: vec![
            GeneratedFile {
                path: HEADER_FILE.to_owned(),
                contents: header.finish(),
            },
            GeneratedFile {
                path: BOARD_HEADER_FILE.to_owned(),
                contents: board_header.finish(),
            },
            GeneratedFile {
                path: SOURCE_FILE.to_owned(),
                contents: source.finish(),
            },
        ]
        .into_iter()
        .chain(peripheral_headers)
        .collect(),
    })
}

/// Defines `EMELA_UNUSED` for the runtime's static inline helpers. They are emitted
/// whether or not the program uses them, and clang (unlike gcc) warns on unused
/// static inline functions in the main file.
pub(crate) fn emit_unused_attribute(writer: &mut Writer) {
    writer.line("#if defined(__GNUC__) || defined(__clang__)");
    writer.line("#define EMELA_UNUSED __attribute__((unused))");
    writer.line("#else");
    writer.line("#define EMELA_UNUSED");
    writer.line("#endif");
}

fn emit_local_declarations(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    string_buffers: &StringBufferPlan,
    writer: &mut Writer,
) {
    for (id, variable) in function.vars.iter() {
        if !function.params.contains(&id) {
            if layouts.is_managed_value(variable.ty) {
                let initial = if layouts.is_managed_list(variable.ty) {
                    "NULL"
                } else {
                    "{0}"
                };
                writer.decl(
                    &layouts.c_type(variable.ty),
                    &variable_name(id),
                    Some(initial),
                );
                writer.decl("int", &owned_variable_name(id), Some("INT32_C(0)"));
            } else {
                writer.decl(&layouts.c_type(variable.ty), &variable_name(id), None);
            }
        }
    }
    for (name, capacity) in &string_buffers.declarations {
        writer.line(&format!("char {name}[{capacity}];"));
    }
    for (variable, _) in function.vars.iter() {
        writer.line(&format!("(void)&{};", variable_name(variable)));
    }
}

fn emit_closure_environment_declarations(
    layouts: &TypeLayouts<'_>,
    closures: &crate::static_closure::ClosureLowering,
    writer: &mut Writer,
) {
    for (index, environment) in closures.environments.iter().enumerate() {
        writer.line("typedef struct {");
        let mut represented = false;
        for (capture_index, capture) in environment.captures.iter().enumerate() {
            if layouts.type_repr(capture.ty) == Repr::Void {
                continue;
            }
            represented = true;
            writer.line(&format!(
                "  {} c{capture_index};",
                layouts.c_type(capture.ty)
            ));
        }
        if !represented {
            writer.line("  uint8_t _unit;");
        }
        writer.line(&format!("}} emela_closure_env_{index};"));
        writer.blank();
    }
}

fn emit_closure_string_copy_helper(layouts: &TypeLayouts<'_>, writer: &mut Writer) {
    let kind = layouts.string_owner_kind();
    writer.begin_block(
        "static inline EMELA_UNUSED EmelaString emela_heap_copy_string(EmelaString value)",
    );
    writer.line("if (value.length == UINT16_C(0)) return (EmelaString){ .data = \"\" };");
    writer.line(&format!(
        "char *bytes = (char *)emela_heap_allocate((size_t)value.length, UINT32_C({kind}));"
    ));
    writer.line("for (uint32_t index = UINT32_C(0); index < (uint32_t)value.length; ++index)");
    writer.begin_block("");
    writer.line("bytes[index] = value.data[index];");
    writer.end_block();
    writer.line("EmelaString copy = { bytes, value.length, bytes };");
    writer.ret(Some("copy"));
    writer.end_block();
    writer.blank();
    writer.begin_block(
        "static inline EMELA_UNUSED EmelaString emela_heap_own_string(EmelaString value)",
    );
    writer.line("if (value.length == UINT16_C(0)) return (EmelaString){ .data = \"\" };");
    writer.line("if (value.owner == NULL) return emela_heap_copy_string(value);");
    writer.line("emela_heap_retain(value.owner);");
    writer.line("return value;");
    writer.end_block();
}

fn emit_closure_capture_loads(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    environment: usize,
    writer: &mut Writer,
) {
    let captures = &layouts.closure_environments[environment].captures;
    writer.begin_block("if (emela_env == NULL)");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_PANIC, \"null closure environment\", UINT16_C(24));",
    );
    writer.end_block();
    for (capture_index, capture) in captures.iter().enumerate() {
        if layouts.type_repr(capture.ty) == Repr::Void {
            continue;
        }
        let source = format!("((emela_closure_env_{environment} *)emela_env)->c{capture_index}");
        if layouts.is_managed_value(capture.ty) {
            layouts.emit_retain_value_expression(capture.ty, &source, writer);
        }
        writer.assign(&variable_name(capture.local), &source);
        if layouts.is_managed_value(function.vars[capture.local].ty) {
            writer.assign(&owned_variable_name(capture.local), "INT32_C(1)");
        }
    }
}

fn emit_release_if_owned(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    variable: VarId,
    writer: &mut Writer,
) {
    let ty = function.vars[variable].ty;
    if !layouts.is_managed_value(ty) {
        return;
    }
    writer.begin_block(&format!(
        "if ({} != INT32_C(0))",
        owned_variable_name(variable)
    ));
    layouts.emit_release_value_expression(ty, &variable_name(variable), writer);
    writer.end_block();
    writer.assign(&owned_variable_name(variable), "INT32_C(0)");
}

fn emit_retain_operand(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    operand: &Operand,
    writer: &mut Writer,
) {
    if let Operand::Var(variable) = operand
        && layouts.is_managed_value(function.vars[*variable].ty)
    {
        layouts.emit_retain_value_expression(
            function.vars[*variable].ty,
            &variable_name(*variable),
            writer,
        );
    }
}

fn emit_owned_cleanup(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    except: Option<VarId>,
    writer: &mut Writer,
) {
    for (variable, data) in function.vars.iter() {
        if except == Some(variable)
            || function.params.contains(&variable)
            || !layouts.is_managed_value(data.ty)
        {
            continue;
        }
        emit_release_if_owned(layouts, function, variable, writer);
    }
    if layouts.has_cause_raise && layouts.has_unwind && !function.suspends && !function.constant {
        writer.line(
            "if (emela_current_fiber != NULL) emela_current_fiber->holds = emela_entry_holds;",
        );
    }
}

struct StringBufferPlan {
    discarded_results: HashSet<VarId>,
    buffers: HashMap<VarId, (Option<usize>, u16)>,
    declarations: Vec<(String, usize)>,
    output_calls: HashSet<FnId>,
    returns_to_caller: bool,
    capacity: u16,
}

impl StringBufferPlan {
    fn new(
        layouts: &TypeLayouts<'_>,
        function_id: FnId,
        function: &Function,
        capacity: u16,
        output_calls: &HashSet<FnId>,
        returns_to_caller: bool,
    ) -> Self {
        let (statement_positions, sequence_positions) = function_positions(function);
        let mut append_args = HashMap::new();
        let mut buffer_producers = HashSet::new();
        let mut producer_positions = HashMap::new();
        for (statement_id, statement) in function
            .stmts
            .iter()
            .filter(|(id, _)| statement_positions.contains_key(id))
        {
            let Stmt::Let {
                dst: Some(dst),
                value,
                ..
            } = statement
            else {
                continue;
            };
            match value {
                Value::Builtin {
                    op: Builtin::StringAppend,
                    args,
                } if args.len() == 2
                    && !append_is_static(args)
                    && !append_has_empty_constant(args)
                    && matches!(layouts.types[function.vars[*dst].ty], Type::String) =>
                {
                    append_args.insert(*dst, args.clone());
                    buffer_producers.insert(*dst);
                    producer_positions.insert(*dst, statement_id);
                }
                Value::Runtime {
                    op: RuntimeOp::ShowString,
                    args,
                } if args.len() == 1
                    && !matches!(args[0], Operand::Const(Const::String(_)))
                    && matches!(layouts.types[function.vars[*dst].ty], Type::String) =>
                {
                    buffer_producers.insert(*dst);
                    producer_positions.insert(*dst, statement_id);
                }
                Value::Builtin {
                    op: Builtin::IntToString | Builtin::Int64ToString,
                    args,
                } if args.len() == 1
                    && matches!(args[0], Operand::Var(_))
                    && matches!(layouts.types[function.vars[*dst].ty], Type::String) =>
                {
                    buffer_producers.insert(*dst);
                    producer_positions.insert(*dst, statement_id);
                }
                Value::Builtin {
                    op: Builtin::FloatToString,
                    args,
                } if args.len() == 1
                    && matches!(args[0], Operand::Var(_))
                    && matches!(layouts.types[function.vars[*dst].ty], Type::String) =>
                {
                    buffer_producers.insert(*dst);
                    producer_positions.insert(*dst, statement_id);
                }
                Value::Builtin {
                    op: Builtin::StringFromCodePoint,
                    args,
                } if args.len() == 1
                    && matches!(args[0], Operand::Var(_))
                    && option_payload_type(layouts, function.vars[*dst].ty)
                        .is_some_and(|ty| matches!(layouts.types[ty], Type::String)) =>
                {
                    buffer_producers.insert(*dst);
                    producer_positions.insert(*dst, statement_id);
                }
                Value::Builtin {
                    op: Builtin::StringJoin,
                    args,
                } if args.len() == 2
                    && matches!(layouts.types[function.vars[*dst].ty], Type::String) =>
                {
                    buffer_producers.insert(*dst);
                    producer_positions.insert(*dst, statement_id);
                }
                Value::Fiber {
                    op: emela_core::FiberOp::CauseToString,
                    ..
                } => {
                    buffer_producers.insert(*dst);
                    producer_positions.insert(*dst, statement_id);
                }
                Value::Call {
                    callee: Callee::Direct(id),
                    ..
                } if output_calls.contains(id)
                    && matches!(layouts.types[function.vars[*dst].ty], Type::String) =>
                {
                    buffer_producers.insert(*dst);
                    producer_positions.insert(*dst, statement_id);
                }
                _ => {}
            }
        }

        let mut uses = HashMap::new();
        let mut count = |variable| *uses.entry(variable).or_insert(0usize) += 1;
        for (_, statement) in function
            .stmts
            .iter()
            .filter(|(id, _)| statement_positions.contains_key(id))
        {
            match statement {
                Stmt::Let { value, .. } => for_each_string_plan_var(layouts, value, &mut count),
                Stmt::If { cond, .. } => count_operand(cond, &mut count),
                Stmt::Switch { on, .. } => match on {
                    SwitchOn::Tag(variable) | SwitchOn::Lit(variable) => count(*variable),
                },
                Stmt::Loop { inits, .. } => {
                    for operand in inits {
                        count_operand(operand, &mut count);
                    }
                }
                Stmt::Dup(variable) | Stmt::Drop(variable) => count(*variable),
                Stmt::Block { .. } | Stmt::Protect { .. } => {}
            }
        }
        for (_, sequence) in function
            .seqs
            .iter()
            .filter(|(id, _)| sequence_positions.contains_key(id))
        {
            match &sequence.term {
                Term::Yield(Some(operand))
                | Term::Return(Some(operand))
                | Term::Fail(operand)
                | Term::Defect(operand, _) => count_operand(operand, &mut count),
                Term::Yield(None) | Term::Return(None) => {}
                Term::Break { value, .. } => {
                    if let Some(value) = value {
                        count_operand(value, &mut count);
                    }
                }
                Term::Recur(args) => {
                    for operand in args {
                        count_operand(operand, &mut count);
                    }
                }
                Term::Unreachable | Term::Unwind => {}
            }
        }

        let mut last_uses = HashMap::new();
        let mut non_reusable_variables = HashSet::new();
        for (statement_id, statement) in function
            .stmts
            .iter()
            .filter(|(id, _)| statement_positions.contains_key(id))
        {
            let position = statement_positions.get(&statement_id).copied().unwrap_or(0);
            let mut mark_use = |variable| {
                last_uses
                    .entry(variable)
                    .and_modify(|last: &mut usize| *last = (*last).max(position))
                    .or_insert(position);
            };
            match statement {
                Stmt::Let { value, .. } => {
                    for_each_string_plan_var(layouts, value, &mut mark_use);
                    let reusable_consumer = match value {
                        Value::Builtin {
                            op:
                                Builtin::StringByteSize
                                | Builtin::StringContains
                                | Builtin::StringStartsWith
                                | Builtin::StringEndsWith
                                | Builtin::StringAppend,
                            args,
                        } => !append_has_empty_constant(args),
                        Value::Builtin {
                            op: Builtin::StringJoin,
                            ..
                        } => true,
                        Value::Runtime {
                            op: RuntimeOp::ShowString | RuntimeOp::DebugPrint,
                            ..
                        } => true,
                        _ => false,
                    };
                    if !reusable_consumer {
                        for_each_string_plan_var(layouts, value, |variable| {
                            non_reusable_variables.insert(variable);
                        });
                    }
                    if let Value::Builtin {
                        op: Builtin::StringAppend,
                        args,
                    } = value
                        && append_has_empty_constant(args)
                    {
                        for_each_string_plan_var(layouts, value, |variable| {
                            non_reusable_variables.insert(variable);
                        });
                    }
                }
                Stmt::If { cond, .. } => {
                    count_operand(cond, &mut mark_use);
                    if let Operand::Var(variable) = cond {
                        non_reusable_variables.insert(*variable);
                    }
                }
                Stmt::Switch { on, .. } => {
                    let variable = match on {
                        SwitchOn::Tag(variable) | SwitchOn::Lit(variable) => *variable,
                    };
                    mark_use(variable);
                    non_reusable_variables.insert(variable);
                }
                Stmt::Loop { inits, .. } => {
                    for operand in inits {
                        count_operand(operand, &mut mark_use);
                        if let Operand::Var(variable) = operand {
                            non_reusable_variables.insert(*variable);
                        }
                    }
                }
                Stmt::Dup(variable) | Stmt::Drop(variable) => {
                    mark_use(*variable);
                    non_reusable_variables.insert(*variable);
                }
                Stmt::Block { .. } | Stmt::Protect { .. } => {}
            }
        }
        for (sequence_id, sequence) in function
            .seqs
            .iter()
            .filter(|(id, _)| sequence_positions.contains_key(id))
        {
            let position = sequence_positions.get(&sequence_id).copied().unwrap_or(0);
            let mut mark_use = |variable| {
                last_uses
                    .entry(variable)
                    .and_modify(|last: &mut usize| *last = (*last).max(position))
                    .or_insert(position);
                non_reusable_variables.insert(variable);
            };
            match &sequence.term {
                Term::Yield(Some(operand))
                | Term::Return(Some(operand))
                | Term::Fail(operand)
                | Term::Defect(operand, _) => count_operand(operand, &mut mark_use),
                Term::Yield(None) | Term::Return(None) => {}
                Term::Break { value, .. } => {
                    if let Some(value) = value {
                        count_operand(value, &mut mark_use);
                    }
                }
                Term::Recur(args) => {
                    for operand in args {
                        count_operand(operand, &mut mark_use);
                    }
                }
                Term::Unreachable | Term::Unwind => {}
            }
        }

        let mut bounds: HashMap<VarId, usize> = function
            .vars
            .iter()
            .filter_map(|(id, variable)| {
                matches!(layouts.types[variable.ty], Type::String)
                    .then_some((id, usize::from(capacity)))
            })
            .collect();
        for _ in 0..=function.vars.len() {
            let mut changed = false;
            for (_, statement) in function
                .stmts
                .iter()
                .filter(|(id, _)| statement_positions.contains_key(id))
            {
                let Stmt::Let {
                    dst: Some(dst),
                    value,
                    ..
                } = statement
                else {
                    continue;
                };
                let is_string = matches!(layouts.types[function.vars[*dst].ty], Type::String);
                let is_codepoint_option = matches!(
                    value,
                    Value::Builtin {
                        op: Builtin::StringFromCodePoint,
                        ..
                    }
                ) && option_payload_type(layouts, function.vars[*dst].ty)
                    .is_some_and(|ty| matches!(layouts.types[ty], Type::String));
                if !is_string && !is_codepoint_option {
                    continue;
                }
                let bound = match value {
                    Value::Const(Const::String(value)) => value.len(),
                    Value::Builtin {
                        op: Builtin::StringTrim,
                        args,
                    } if args.len() == 1 => operand_string_bound(&args[0], &bounds, capacity),
                    Value::Builtin {
                        op: Builtin::StringAppend,
                        args,
                    } if args.len() == 2 => {
                        let left = operand_string_bound(&args[0], &bounds, capacity);
                        let right = operand_string_bound(&args[1], &bounds, capacity);
                        if append_is_static(args) {
                            left.saturating_add(right)
                        } else if append_has_empty_constant(args) {
                            left.max(right)
                        } else {
                            left.saturating_add(right).min(usize::from(capacity))
                        }
                    }
                    Value::Builtin {
                        op: op @ (Builtin::IntToString | Builtin::Int64ToString),
                        args,
                    } if args.len() == 1 => match (&args[0], op) {
                        (Operand::Const(Const::Int(value)), _) => value.to_string().len(),
                        (Operand::Const(Const::Int64(value)), _) => value.to_string().len(),
                        (_, Builtin::IntToString) => 11.min(usize::from(capacity)),
                        (_, Builtin::Int64ToString) => 20.min(usize::from(capacity)),
                        _ => unreachable!(),
                    },
                    Value::Builtin {
                        op: Builtin::FloatToString,
                        args,
                    } if args.len() == 1 => match &args[0] {
                        Operand::Const(Const::Float(value)) => {
                            const_eval::format_float(*value).len()
                        }
                        _ => crate::float_to_string::MAX_OUTPUT_BYTES.min(usize::from(capacity)),
                    },
                    Value::Builtin {
                        op: Builtin::StringFromCodePoint,
                        ..
                    } => 4.min(usize::from(capacity)),
                    Value::Builtin {
                        op: Builtin::StringJoin,
                        ..
                    } => usize::from(capacity),
                    _ => usize::from(capacity),
                };
                let bound = match value {
                    Value::Runtime {
                        op: RuntimeOp::ShowString,
                        args,
                    } if args.len() == 1 => match &args[0] {
                        Operand::Const(Const::String(value)) => show_string(value).len(),
                        operand => operand_string_bound(operand, &bounds, capacity)
                            .saturating_mul(6)
                            .saturating_add(2)
                            .min(usize::from(capacity)),
                    },
                    _ => bound,
                };
                if bounds.get(dst).copied() != Some(bound) {
                    bounds.insert(*dst, bound);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        let mut roots: HashMap<VarId, VarId> =
            buffer_producers.iter().map(|&id| (id, id)).collect();
        for _ in 0..=append_args.len() {
            let mut changed = false;
            for (&dst, args) in &append_args {
                let Some(Operand::Var(left)) = args.first() else {
                    continue;
                };
                if uses.get(left).copied() != Some(1) || !roots.contains_key(left) {
                    continue;
                }
                let root = roots[left];
                if roots.get(&dst).copied() != Some(root) {
                    roots.insert(dst, root);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        let returned_root = if returns_to_caller {
            function
                .body
                .map(|body| &function.seqs[body].term)
                .and_then(|term| match term {
                    Term::Return(Some(Operand::Var(variable))) => roots.get(variable).copied(),
                    _ => None,
                })
        } else {
            None
        };

        let mut root_capacities: HashMap<VarId, usize> = HashMap::new();
        let mut root_intervals: HashMap<VarId, (usize, usize)> = HashMap::new();
        let mut non_reusable_roots = HashSet::new();
        for (&variable, &root) in &roots {
            let bound = bounds
                .get(&variable)
                .copied()
                .unwrap_or(usize::from(capacity))
                .min(usize::from(capacity));
            root_capacities
                .entry(root)
                .and_modify(|current| *current = (*current).max(bound))
                .or_insert(bound);
            let definition = producer_positions
                .get(&variable)
                .and_then(|statement| statement_positions.get(statement))
                .copied()
                .unwrap_or(0);
            let end = last_uses.get(&variable).copied().unwrap_or(definition);
            root_intervals
                .entry(root)
                .and_modify(|(start, last)| {
                    *start = (*start).min(definition);
                    *last = (*last).max(end);
                })
                .or_insert((definition, end));
            if non_reusable_variables.contains(&variable) {
                non_reusable_roots.insert(root);
            }
        }
        let mut groups: Vec<_> = root_capacities
            .iter()
            .filter(|(root, _)| Some(**root) != returned_root)
            .map(|(&root, &size)| {
                let (start, end) = root_intervals.get(&root).copied().unwrap_or((0, 0));
                (root, start, end, size, !non_reusable_roots.contains(&root))
            })
            .collect();
        groups.sort_by_key(|(root, start, _, _, _)| (*start, format!("{root:?}")));

        let mut slot_ends: Vec<Option<usize>> = Vec::new();
        let mut slot_sizes: Vec<usize> = Vec::new();
        let mut root_slots = HashMap::new();
        for (root, start, end, size, reusable) in groups {
            let slot = reusable
                .then(|| {
                    slot_ends
                        .iter()
                        .position(|last| last.is_some_and(|last| last < start))
                })
                .flatten();
            let slot = if let Some(slot) = slot {
                slot_ends[slot] = Some(end);
                slot_sizes[slot] = slot_sizes[slot].max(size.max(1));
                slot
            } else {
                let slot = slot_ends.len();
                slot_ends.push(reusable.then_some(end));
                slot_sizes.push(size.max(1));
                slot
            };
            root_slots.insert(root, slot);
        }
        let declarations = slot_sizes
            .iter()
            .enumerate()
            .map(|(slot, size)| (string_buffer_name(slot), *size))
            .collect();
        let buffers = roots
            .into_iter()
            .map(|(variable, root)| {
                let buffer_capacity = root_capacities[&root] as u16;
                (
                    variable,
                    (
                        (Some(root) != returned_root).then(|| root_slots[&root]),
                        buffer_capacity,
                    ),
                )
            })
            .collect();
        Self {
            discarded_results: layouts
                .discarded_results
                .get(&function_id)
                .cloned()
                .unwrap_or_default(),
            buffers,
            declarations,
            output_calls: output_calls.clone(),
            returns_to_caller,
            capacity,
        }
    }

    fn buffer(&self, variable: VarId) -> Option<(String, String)> {
        self.buffers
            .get(&variable)
            .map(|(slot, capacity)| match slot {
                Some(slot) => (string_buffer_name(*slot), format!("UINT16_C({capacity})")),
                None => (
                    "emela_string_output".to_owned(),
                    "emela_emb_string_output_capacity".to_owned(),
                ),
            })
    }

    /// The length of the stack array for `variable`, or None for the caller's output buffer.
    fn array_capacity(&self, variable: VarId) -> Option<u16> {
        self.buffers
            .get(&variable)
            .and_then(|(slot, capacity)| slot.map(|_| *capacity))
    }

    fn has_string_output_call(&self, function: FnId) -> bool {
        self.output_calls.contains(&function)
    }
}

pub(crate) fn function_positions(
    function: &Function,
) -> (HashMap<StmtId, usize>, HashMap<SeqId, usize>) {
    fn visit(
        function: &Function,
        sequence: SeqId,
        next: &mut usize,
        visited: &mut HashSet<SeqId>,
        statement_positions: &mut HashMap<StmtId, usize>,
        sequence_positions: &mut HashMap<SeqId, usize>,
    ) {
        if !visited.insert(sequence) {
            return;
        }
        for &statement_id in &function.seqs[sequence].stmts {
            *next += 1;
            statement_positions.insert(statement_id, *next);
            match &function.stmts[statement_id] {
                Stmt::If { then, else_, .. } => {
                    visit(
                        function,
                        *then,
                        next,
                        visited,
                        statement_positions,
                        sequence_positions,
                    );
                    visit(
                        function,
                        *else_,
                        next,
                        visited,
                        statement_positions,
                        sequence_positions,
                    );
                }
                Stmt::Switch { cases, default, .. } => {
                    for case in cases {
                        visit(
                            function,
                            case.body,
                            next,
                            visited,
                            statement_positions,
                            sequence_positions,
                        );
                    }
                    if let Some(default) = default {
                        visit(
                            function,
                            *default,
                            next,
                            visited,
                            statement_positions,
                            sequence_positions,
                        );
                    }
                }
                Stmt::Loop { body, .. } | Stmt::Block { body, .. } => visit(
                    function,
                    *body,
                    next,
                    visited,
                    statement_positions,
                    sequence_positions,
                ),
                Stmt::Protect { body, cleanup, .. } => {
                    visit(
                        function,
                        *body,
                        next,
                        visited,
                        statement_positions,
                        sequence_positions,
                    );
                    visit(
                        function,
                        *cleanup,
                        next,
                        visited,
                        statement_positions,
                        sequence_positions,
                    );
                }
                Stmt::Let { .. } | Stmt::Dup(_) | Stmt::Drop(_) => {}
            }
        }
        *next += 1;
        sequence_positions.insert(sequence, *next);
    }

    let mut statement_positions = HashMap::new();
    let mut sequence_positions = HashMap::new();
    let mut visited = HashSet::new();
    let mut next = 0;
    if let Some(body) = function.body {
        visit(
            function,
            body,
            &mut next,
            &mut visited,
            &mut statement_positions,
            &mut sequence_positions,
        );
    }
    (statement_positions, sequence_positions)
}

fn for_each_string_plan_var(
    layouts: &TypeLayouts<'_>,
    value: &Value,
    mut visit: impl FnMut(VarId),
) {
    value.for_each_var(&mut visit);
    if let Value::FnRef(target) = value
        && let Some(&environment) = layouts.closure_functions.get(target)
    {
        for capture in &layouts.closure_environments[environment].captures {
            visit(capture.source);
        }
    }
}

fn count_operand(operand: &Operand, count: &mut impl FnMut(VarId)) {
    if let Operand::Var(variable) = operand {
        count(*variable);
    }
}

fn operand_string_bound(operand: &Operand, bounds: &HashMap<VarId, usize>, fallback: u16) -> usize {
    match operand {
        Operand::Const(Const::String(value)) => value.len(),
        Operand::Var(variable) => bounds
            .get(variable)
            .copied()
            .unwrap_or(usize::from(fallback)),
        _ => usize::from(fallback),
    }
}

fn append_is_static(args: &[Operand]) -> bool {
    matches!(
        args,
        [
            Operand::Const(Const::String(_)),
            Operand::Const(Const::String(_))
        ]
    )
}

fn append_has_empty_constant(args: &[Operand]) -> bool {
    matches!(args.first(), Some(Operand::Const(Const::String(value))) if value.is_empty())
        || matches!(args.get(1), Some(Operand::Const(Const::String(value))) if value.is_empty())
}

fn string_buffer_name(slot: usize) -> String {
    format!("emela_string_buffer_{slot}")
}

fn operand_type(function: &Function, operand: &Operand) -> Option<TypeId> {
    match operand {
        Operand::Var(variable) => Some(function.vars[*variable].ty),
        Operand::Const(Const::Int(_)) => Some(TypeId::INT),
        Operand::Const(Const::Bool(_)) => Some(TypeId::BOOL),
        Operand::Const(Const::Int64(_)) => Some(TypeId::INT64),
        Operand::Const(Const::Float(_)) => Some(TypeId::FLOAT),
        Operand::Const(Const::String(_)) => Some(TypeId::STRING),
        Operand::Const(Const::Unit) => Some(TypeId::UNIT),
    }
}

fn validate_function_value_types(
    layouts: &TypeLayouts<'_>,
    emitted: &HashSet<FnId>,
    flow: &emela_lir::Module,
) -> Result<(), Vec<Error>> {
    for &ty in &layouts.function_types {
        let Type::Fn(function_type) = layouts.types[ty].clone() else {
            unreachable!("function type registry contains a non-function")
        };
        if function_type.sets.is_some()
            || !function_value_abi_type_supported(layouts, function_type.ret, &mut HashSet::new())
            || function_type.params.iter().any(|&parameter| {
                !function_value_abi_type_supported(layouts, parameter, &mut HashSet::new())
            })
        {
            let owner = flow
                .functions
                .iter()
                .find(|(id, function)| {
                    emitted.contains(id)
                        && (function.ret == ty
                            || function.vars.iter().any(|(_, variable)| variable.ty == ty))
                })
                .map_or("function value", |(_, function)| function.name.as_str());
            return Err(unsupported(
                owner,
                "function values with unsupported parameter, result, or suspend types",
            ));
        }
    }
    Ok(())
}

fn function_value_abi_type_supported(
    layouts: &TypeLayouts<'_>,
    ty: TypeId,
    seen: &mut HashSet<TypeId>,
) -> bool {
    match layouts.types[ty].clone() {
        Type::Int
        | Type::Int64
        | Type::Float
        | Type::Bool
        | Type::Unit
        | Type::Never
        | Type::String
        | Type::Any
        | Type::List(_)
        | Type::Cell(_)
        | Type::Handler(_)
        | Type::Evidence
        | Type::FiberScope => true,
        Type::Enum(enum_id, _) => {
            layouts.enum_is_i32(enum_id)
                || layouts.core.enums[enum_id].external
                || layouts.is_cause(ty)
                || aggregate_function_abi_supported(layouts, ty, seen)
        }
        Type::Tuple(_) => aggregate_function_abi_supported(layouts, ty, seen),
        Type::Fn(function_type) => {
            if !seen.insert(ty) {
                // Function signatures may refer back through an aggregate field.
                return true;
            }
            let supported = function_type.sets.is_none()
                && function_type
                    .params
                    .iter()
                    .all(|&parameter| function_value_abi_type_supported(layouts, parameter, seen))
                && function_value_abi_type_supported(layouts, function_type.ret, seen);
            seen.remove(&ty);
            supported
        }
        _ => false,
    }
}

fn aggregate_function_abi_supported(
    layouts: &TypeLayouts<'_>,
    ty: TypeId,
    seen: &mut HashSet<TypeId>,
) -> bool {
    let Some(&index) = layouts.aggregate_ids.get(&ty) else {
        return false;
    };
    if !seen.insert(ty) {
        return true;
    }
    let fields = match &layouts.aggregates[index].kind {
        AggregateKind::Tuple(fields) => vec![fields.as_slice()],
        AggregateKind::Enum(variants) => variants.iter().map(Vec::as_slice).collect(),
    };
    let supported = fields.into_iter().enumerate().all(|(variant, fields)| {
        fields.iter().enumerate().all(|(index, &field)| {
            // Opaque owned edges use the heap header ABI; materialized payload roots are checked separately.
            layouts.is_recursive_box_field(ty, variant, index)
                || layouts.type_repr(field) == Repr::Void
                || function_value_abi_type_supported(layouts, field, seen)
        })
    });
    seen.remove(&ty);
    supported
}

fn validate_function_refs(
    flow: &emela_lir::Module,
    emitted: &HashSet<FnId>,
    layouts: &TypeLayouts<'_>,
) -> Result<(), Vec<Error>> {
    for (function_id, function) in flow.functions.iter() {
        if !emitted.contains(&function_id) {
            continue;
        }
        for (_, statement) in function.stmts.iter() {
            let Stmt::Let {
                dst,
                value: Value::FnRef(target),
                ..
            } = statement
            else {
                continue;
            };
            let Some(dst) = dst else {
                continue;
            };
            let Type::Fn(function_type) = layouts.types[function.vars[*dst].ty].clone() else {
                return Err(invalid_flow_ir(
                    &function.name,
                    "FnRef destination is not a function value",
                ));
            };
            let target_function = &flow.functions[*target];
            let mut expected_params: Vec<TypeId> = function_type
                .params
                .into_iter()
                .filter(|&parameter| layouts.type_repr(parameter) != Repr::Void)
                .collect();
            if function_type.ev {
                expected_params.push(TypeId::EVIDENCE);
            }
            let target_params: Vec<TypeId> = target_function
                .params
                .iter()
                .skip(usize::from(target_function.env.is_some()))
                .map(|&parameter| target_function.vars[parameter].ty)
                .collect();
            if expected_params != target_params
                || function_type.ret != target_function.ret
                || function_type.fails != target_function.fails
                || function_type.suspends != target_function.suspends
            {
                return Err(invalid_flow_ir(
                    &function.name,
                    "FnRef type does not match the referenced function ABI",
                ));
            }
        }
    }
    Ok(())
}

// A literal terminal defect cannot run cleanup under this backend's immediate-stop policy.
fn runtime_statements(function: &Function) -> HashSet<StmtId> {
    fn visit(
        f: &Function,
        seq: SeqId,
        visited: &mut HashSet<SeqId>,
        result: &mut HashSet<StmtId>,
        suspends: bool,
    ) {
        if !visited.insert(seq) {
            return;
        }
        for &id in &f.seqs[seq].stmts {
            result.insert(id);
            match &f.stmts[id] {
                Stmt::Let {
                    value: Value::Closure(c),
                    ..
                } => visit(f, c.body, visited, result, c.suspends),
                Stmt::If { then, else_, .. } => {
                    visit(f, *then, visited, result, suspends);
                    visit(f, *else_, visited, result, suspends);
                }
                Stmt::Switch { cases, default, .. } => {
                    for case in cases {
                        visit(f, case.body, visited, result, suspends);
                    }
                    if let Some(seq) = default {
                        visit(f, *seq, visited, result, suspends);
                    }
                }
                Stmt::Block { body, .. } | Stmt::Loop { body, .. } => {
                    visit(f, *body, visited, result, suspends)
                }
                Stmt::Protect { body, cleanup, .. } => {
                    visit(f, *body, visited, result, suspends);
                    let body = &f.seqs[*body];
                    if suspends
                        || !body.stmts.is_empty()
                        || !matches!(body.term, Term::Defect(_, _))
                    {
                        visit(f, *cleanup, visited, result, suspends);
                    }
                }
                _ => {}
            }
        }
    }
    let mut result = HashSet::new();
    if let Some(body) = function.body {
        visit(
            function,
            body,
            &mut HashSet::new(),
            &mut result,
            function.suspends,
        );
    }
    result
}

pub(crate) fn runtime_reachable(flow: &emela_lir::Module) -> HashSet<FnId> {
    let mut pending: Vec<FnId> = flow
        .functions
        .iter()
        .filter_map(|(id, function)| function.exported.then_some(id))
        .collect();
    if pending.is_empty() {
        pending.extend(
            flow.functions
                .iter()
                .filter_map(|(id, function)| (!function.constant).then_some(id)),
        );
    }

    let mut reachable = HashSet::new();
    loop {
        while let Some(id) = pending.pop() {
            if !reachable.insert(id) {
                continue;
            }
            let function = &flow.functions[id];
            if function.constant {
                continue;
            }
            let active = runtime_statements(function);
            for (_, statement) in function.stmts.iter().filter(|(id, _)| active.contains(id)) {
                if let Stmt::Let { dst, value, .. } = statement {
                    match value {
                        Value::Call {
                            callee: Callee::Direct(callee),
                            ..
                        } => pending.push(*callee),
                        Value::FnRef(callee) if dst.is_some() => pending.push(*callee),
                        _ => {}
                    }
                }
            }
        }
        let shows_causes = reachable.iter().any(|id| {
            flow.functions[*id].stmts.iter().any(|(_, stmt)| {
                matches!(
                    stmt,
                    Stmt::Let {
                        value: Value::Fiber { .. }
                            | Value::Call {
                                callee: Callee::Op { .. },
                                suspends: true,
                                ..
                            },
                        ..
                    }
                )
            })
        });
        if shows_causes {
            for &(error, show) in &flow.error_shows {
                if !reachable.contains(&show)
                    && reachable.iter().any(|id| {
                        flow.functions[*id].vars.iter().any(
                        |(_, v)| matches!(flow.core.types[v.ty], Type::Enum(e, _) if e == error),
                    )
                    })
                {
                    pending.push(show);
                }
            }
        }
        if pending.is_empty() {
            break;
        }
    }
    reachable
}

fn validate_isr_entries(
    flow: &emela_lir::Module,
    emitted: &HashSet<FnId>,
    heap_functions: &HashSet<FnId>,
    layouts: &TypeLayouts<'_>,
    names: &HashMap<FnId, String>,
    options: &Options,
) -> Result<(), Vec<Error>> {
    for entry_name in &options.isr_entries {
        let Some((entry, _)) = flow.functions.iter().find(|(id, function)| {
            emitted.contains(id) && function.exported && function.name == *entry_name
        }) else {
            return Err(vec![Error {
                kind: ErrorKind::InvalidOptions,
                message: format!(
                    "ISR entry `{entry_name}` must name an emitted exported Emela function"
                ),
            }]);
        };
        validate_isr_function(
            flow,
            entry,
            entry_name,
            heap_functions,
            layouts,
            names,
            options,
            &mut HashSet::new(),
            &mut HashSet::new(),
        )?;
    }
    Ok(())
}

// Keep the lowering context explicit across this emission boundary.
#[allow(clippy::too_many_arguments)]
fn validate_isr_function(
    flow: &emela_lir::Module,
    id: FnId,
    entry_name: &str,
    heap_functions: &HashSet<FnId>,
    layouts: &TypeLayouts<'_>,
    names: &HashMap<FnId, String>,
    options: &Options,
    active: &mut HashSet<FnId>,
    complete: &mut HashSet<FnId>,
) -> Result<(), Vec<Error>> {
    if active.contains(&id) {
        return Err(unsupported(
            &flow.functions[id].name,
            &format!("recursive call reachable from ISR entry `{entry_name}`"),
        ));
    }
    if complete.contains(&id) {
        return Ok(());
    }
    let function = &flow.functions[id];
    if layouts.runtime_consts.contains(&id) {
        return Err(unsupported(
            &function.name,
            &format!("runtime const initialization reachable from ISR entry `{entry_name}`"),
        ));
    }
    if function.suspends {
        return Err(unsupported(
            &function.name,
            &format!("suspending function reachable from ISR entry `{entry_name}`"),
        ));
    }
    if heap_functions.contains(&id)
        || std::iter::once(function.ret)
            .chain(function.vars.iter().map(|(_, variable)| variable.ty))
            .any(|ty| layouts.contains_heap_ref(ty, &mut HashSet::new()))
    {
        return Err(unsupported(
            &function.name,
            &format!("heap-backed value reachable from ISR entry `{entry_name}`"),
        ));
    }

    let reachable_statements = runtime_statements(function);
    if function.stmts.iter().any(|(id, statement)| {
        reachable_statements.contains(&id)
            && matches!(
                statement,
                Stmt::Let {
                    value: Value::Runtime {
                        op: RuntimeOp::Hold | RuntimeOp::Unhold,
                        ..
                    },
                    ..
                }
            )
    }) {
        return Err(unsupported(
            &function.name,
            &format!("fiber interrupt control reachable from ISR entry `{entry_name}`"),
        ));
    }

    active.insert(id);
    for (_, statement) in function.stmts.iter() {
        let Stmt::Let {
            value: Value::Call {
                callee, suspends, ..
            },
            ..
        } = statement
        else {
            continue;
        };
        if *suspends {
            active.remove(&id);
            return Err(unsupported(
                &function.name,
                &format!("suspending call reachable from ISR entry `{entry_name}`"),
            ));
        }
        match callee {
            Callee::Direct(callee_id) => {
                let callee = &flow.functions[*callee_id];
                if callee.suspends {
                    active.remove(&id);
                    return Err(unsupported(
                        &function.name,
                        &format!("suspending call reachable from ISR entry `{entry_name}`"),
                    ));
                }
                if callee.external.is_some() {
                    let symbol =
                        external::isr_symbol(flow, callee, &names[callee_id], options, entry_name)?;
                    if !options.isr_safe_externals.iter().any(|safe| safe == symbol) {
                        active.remove(&id);
                        return Err(unsupported(
                            &function.name,
                            &format!(
                                "external C function `{symbol}` reachable from ISR entry `{entry_name}` is not marked ISR-safe"
                            ),
                        ));
                    }
                } else if !callee.constant {
                    validate_isr_function(
                        flow,
                        *callee_id,
                        entry_name,
                        heap_functions,
                        layouts,
                        names,
                        options,
                        active,
                        complete,
                    )?;
                }
            }
            Callee::Indirect(_) | Callee::Op { .. } => {
                active.remove(&id);
                return Err(unsupported(
                    &function.name,
                    &format!("dynamic call reachable from ISR entry `{entry_name}`"),
                ));
            }
        }
    }
    active.remove(&id);
    complete.insert(id);
    Ok(())
}

fn function_needs_heap(
    function_id: FnId,
    function: &Function,
    layouts: &TypeLayouts<'_>,
    closures: &crate::static_closure::ClosureLowering,
    string_output_functions: &HashSet<FnId>,
    empty_list_functions: &HashSet<FnId>,
) -> bool {
    let mut known_empty = HashSet::new();
    for (_, statement) in function.stmts.iter() {
        if let Stmt::Let {
            dst: Some(dst),
            value:
                Value::Call {
                    callee: Callee::Direct(target),
                    ..
                },
            ..
        } = statement
            && empty_list_functions.contains(target)
        {
            known_empty.insert(*dst);
        }
        let Stmt::Let {
            dst: Some(dst),
            value: Value::List { elems, tail },
            ..
        } = statement
        else {
            continue;
        };
        if !elems.is_empty() {
            continue;
        }
        match tail {
            None => {
                known_empty.insert(*dst);
            }
            Some(Operand::Var(tail)) if known_empty.contains(tail) => {
                known_empty.insert(*dst);
            }
            _ => {}
        }
    }

    let statement_needs_heap = function.stmts.iter().any(|(_, statement)| match statement {
        Stmt::Let {
            value: Value::FnRef(target),
            ..
        } if string_output_functions.contains(target) => true,
        Stmt::Let {
            value:
                Value::List {
                    elems,
                    tail: Some(tail),
                },
            ..
        } if !elems.is_empty()
            || !matches!(tail, Operand::Var(variable) if known_empty.contains(variable)) =>
        {
            true
        }
        Stmt::Let {
            value: Value::List { elems, tail: None },
            ..
        } => !elems.is_empty(),
        Stmt::Let {
            value: Value::ListHead(_) | Value::ListTail(_),
            ..
        } => true,
        Stmt::Let {
            value:
                Value::Fiber { .. }
                | Value::Builtin {
                    op: Builtin::CellNew,
                    ..
                },
            ..
        } => true,
        Stmt::Let {
            value: Value::Handler { .. } | Value::EvLookup { .. },
            ..
        } => true,
        Stmt::Let {
            value: Value::EvExtend { bindings, .. },
            ..
        } => !bindings.is_empty(),
        Stmt::Let {
            dst: Some(dst),
            value: Value::Coerce(source),
            ..
        } if matches!(layouts.types[function.vars[*dst].ty], Type::Any)
            && !matches!(layouts.types[function.vars[*source].ty], Type::Any) =>
        {
            layouts.any_box_required
        }
        Stmt::Let {
            value:
                Value::Call {
                    callee: Callee::Op { .. },
                    ..
                }
                | Value::Runtime {
                    op: RuntimeOp::Retire,
                    ..
                },
            ..
        } => true,
        _ => false,
    });
    let returns_managed_alias = matches!(layouts.types[function.ret], Type::List(_))
        && function.seqs.iter().any(|(_, sequence)| {
            matches!(
                sequence.term,
                Term::Return(Some(Operand::Var(variable)))
                    if function.vars[variable].ty == function.ret
                        && !known_empty.contains(&variable)
            )
        });
    let local_managed_value = function.vars.iter().any(|(variable, data)| {
        !function.params.contains(&variable)
            && layouts.contains_heap_ref(data.ty, &mut HashSet::new())
            && !matches!(layouts.types[data.ty], Type::Handler(_) | Type::Evidence)
            && !known_empty.contains(&variable)
    });
    let returns_managed_aggregate = !matches!(layouts.types[function.ret], Type::List(_))
        && layouts.contains_heap_ref(function.ret, &mut HashSet::new());
    (function.fails && layouts.managed_error_payloads)
        || layouts.owned_aggregate_returns.contains(&function.name)
        || (layouts.runtime_consts.contains(&function_id)
            && layouts.owned_string_returns.contains(&function.name))
        || statement_needs_heap
        || (function.suspends
            && (layouts.has_inline_string(function.ret)
                || function
                    .vars
                    .iter()
                    .any(|(_, var)| layouts.has_inline_string(var.ty))))
        || returns_managed_alias
        || local_managed_value
        || returns_managed_aggregate
        || closures
            .environments
            .iter()
            .any(|environment| environment.parent == function_id)
}

fn emit_const_getter(
    layouts: &TypeLayouts<'_>,
    id: FnId,
    function: &Function,
    name: &str,
    initializer: &str,
    writer: &mut Writer,
) {
    let index = u32::from(id.into_raw());
    let state = format!("emela_const_state_{index}");
    let cache = format!("emela_const_value_{index}");
    writer.line(&format!("static EmelaConstState {state};"));
    let is_void = matches!(layouts.types[function.ret], Type::Unit | Type::Never);
    if !is_void {
        writer.line(&format!("static {} {cache};", layouts.c_type(function.ret)));
    }
    writer.begin_block(&signature(
        layouts,
        function,
        name,
        !function.exported,
        false,
        false,
    ));
    writer.begin_block(&format!("if ({state}.status == 3)"));
    writer.line(&format!(
        "emela_embedded_report_defect({state}.kind, {state}.message, {state}.length);"
    ));
    writer.end_block();
    writer.begin_block(&format!("if ({state}.status == 1)"));
    let message = format!(
        "const {} refers to itself while being evaluated",
        function.name
    );
    writer.line(&format!(
        "emela_const_self_reference(EMELA_DEFECT_PANIC, {}, UINT16_C({}));",
        c_string_literal(message.as_bytes()),
        message.len()
    ));
    writer.end_block();
    writer.begin_block(&format!("if ({state}.status == 0)"));
    let prefix = format!("while evaluating const {}: ", function.name);
    writer.line(&format!(
        "EmelaConstContext context = {{ emela_const_current, &{state}, {}, UINT16_C({}) }};",
        c_string_literal(prefix.as_bytes()),
        prefix.len()
    ));
    writer.line(&format!("{state}.status = 1;"));
    writer.line("emela_const_current = &context;");
    if is_void {
        writer.line(&format!("{initializer}();"));
    } else {
        writer.line(&format!("{cache} = {initializer}();"));
        if layouts.has_inline_string(function.ret) {
            layouts.emit_freeze_value_in_place(function.ret, &cache, writer);
        }
    }
    writer.line("emela_const_current = context.previous;");
    writer.line(&format!("{state}.status = 2;"));
    writer.end_block();
    if is_void {
        if matches!(layouts.types[function.ret], Type::Never) {
            writer.line(
                "emela_board_defect(EMELA_DEFECT_PANIC, \"Never const returned\", UINT16_C(20));",
            );
        } else {
            writer.ret(None);
        }
    } else {
        layouts.emit_retain_value_expression(function.ret, &cache, writer);
        writer.ret(Some(&cache));
    }
    writer.end_block();
    writer.blank();
}

fn directly_calls(caller: &Function, target: FnId) -> bool {
    let (positions, _) = function_positions(caller);
    caller.stmts.iter().any(|(id, statement)| {
        positions.contains_key(&id)
            && matches!(
                statement,
                Stmt::Let {
                    value: Value::Call {
                        callee: Callee::Direct(id),
                        ..
                    },
                    ..
                } if *id == target
            )
    })
}

fn operand_is_string(layouts: &TypeLayouts<'_>, function: &Function, operand: &Operand) -> bool {
    match operand {
        Operand::Const(Const::String(_)) => true,
        Operand::Var(variable) => {
            matches!(layouts.types[function.vars[*variable].ty], Type::String)
        }
        _ => false,
    }
}

fn operand_is_string_list(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    operand: &Operand,
) -> bool {
    match operand {
        Operand::Var(variable) => {
            matches!(
                layouts.types[function.vars[*variable].ty],
                Type::List(TypeId::STRING)
            )
        }
        _ => false,
    }
}

fn operand_is_int(layouts: &TypeLayouts<'_>, function: &Function, operand: &Operand) -> bool {
    match operand {
        Operand::Const(Const::Int(_)) => true,
        Operand::Var(variable) => matches!(layouts.types[function.vars[*variable].ty], Type::Int),
        _ => false,
    }
}

fn operand_is_int64(layouts: &TypeLayouts<'_>, function: &Function, operand: &Operand) -> bool {
    match operand {
        Operand::Const(Const::Int64(_)) => true,
        Operand::Var(variable) => {
            matches!(layouts.types[function.vars[*variable].ty], Type::Int64)
        }
        _ => false,
    }
}

fn operand_is_float(layouts: &TypeLayouts<'_>, function: &Function, operand: &Operand) -> bool {
    match operand {
        Operand::Const(Const::Float(_)) => true,
        Operand::Var(variable) => matches!(layouts.types[function.vars[*variable].ty], Type::Float),
        _ => false,
    }
}

fn option_payload_type(layouts: &TypeLayouts<'_>, ty: TypeId) -> Option<TypeId> {
    let Type::Enum(enum_id, _) = layouts.types[ty] else {
        return None;
    };
    let definition = &layouts.core.enums[enum_id];
    if definition.name != "Option" {
        return None;
    }
    let some_index = definition
        .variants
        .iter()
        .position(|variant| variant.name == "Some")?;
    let fields = layouts.enum_fields(ty)?.get(some_index)?;
    (fields.len() == 1).then_some(fields[0])
}

fn option_variant_expression(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    ty: TypeId,
    variant_name: &str,
    value: Option<&str>,
) -> Result<String, Vec<Error>> {
    let Type::Enum(enum_id, _) = layouts.types[ty] else {
        return Err(invalid_flow_ir(
            &function.name,
            "Option builtin result is not an enum",
        ));
    };
    let definition = &layouts.core.enums[enum_id];
    if definition.name != "Option" {
        return Err(invalid_flow_ir(
            &function.name,
            "Option builtin result has the wrong enum type",
        ));
    }
    let Some(variant_index) = definition
        .variants
        .iter()
        .position(|variant| variant.name == variant_name)
    else {
        return Err(invalid_flow_ir(
            &function.name,
            "Option enum is missing a required variant",
        ));
    };
    let Some(fields) = layouts
        .enum_fields(ty)
        .and_then(|variants| variants.get(variant_index))
    else {
        return Err(invalid_flow_ir(
            &function.name,
            "Option result has no inline enum layout",
        ));
    };
    let mut initializers = vec![format!(".tag = {}", int32_literal(variant_index as i32))];
    match value {
        Some(value) if fields.len() == 1 && layouts.type_repr(fields[0]) != Repr::Void => {
            initializers.push(format!(".payload.v{variant_index}.f0 = {value}"));
        }
        Some(_) => {
            return Err(invalid_flow_ir(
                &function.name,
                "Option Some payload does not match its runtime representation",
            ));
        }
        None if fields
            .iter()
            .all(|field| layouts.type_repr(*field) == Repr::Void) =>
        {
            initializers.push(format!(".payload.v{variant_index}._unit = UINT8_C(0)"));
        }
        None => {
            return Err(invalid_flow_ir(
                &function.name,
                "Option None variant unexpectedly carries a runtime payload",
            ));
        }
    }
    Ok(format!(
        "({}){{ {} }}",
        layouts.c_type(ty),
        initializers.join(", ")
    ))
}

fn emit_builtin(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    op: Builtin,
    args: &[Operand],
    string_buffers: &StringBufferPlan,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    if matches!(op, Builtin::CellNew | Builtin::CellGet | Builtin::CellSet) {
        return emit_cell_builtin(layouts, function, dst, op, args, writer);
    }

    let (arity, result_type) = match op {
        Builtin::StringLength => (1, Type::Int),
        Builtin::StringAppend => (2, Type::String),
        Builtin::StringByteSize => (1, Type::Int),
        Builtin::StringChars => (1, Type::List(TypeId::STRING)),
        Builtin::StringCodePoints => (1, Type::List(TypeId::INT)),
        Builtin::StringJoin => (2, Type::String),
        Builtin::StringSplit => (2, Type::List(TypeId::STRING)),
        Builtin::StringContains | Builtin::StringStartsWith | Builtin::StringEndsWith => {
            (2, Type::Bool)
        }
        Builtin::StringTrim => (1, Type::String),
        Builtin::StringFromCodePoint => (1, Type::String),
        Builtin::IntToString => (1, Type::String),
        Builtin::IntParse => (1, Type::Int),
        Builtin::IntBitAnd | Builtin::IntBitOr | Builtin::IntBitXor => (2, Type::Int),
        Builtin::IntBitNot => (1, Type::Int),
        Builtin::IntCheckedAdd => (2, Type::Int),
        Builtin::IntShiftLeft | Builtin::IntShiftRight | Builtin::IntShiftRightUnsigned => {
            (2, Type::Int)
        }
        Builtin::Int64BitAnd | Builtin::Int64BitOr | Builtin::Int64BitXor => (2, Type::Int64),
        Builtin::Int64BitNot => (1, Type::Int64),
        Builtin::Int64ToString => (1, Type::String),
        Builtin::FloatToString => (1, Type::String),
        Builtin::FloatFromInt => (1, Type::Float),
        Builtin::FloatParse => (1, Type::Float),
        Builtin::FloatFloor | Builtin::FloatCeil | Builtin::FloatRound | Builtin::FloatTruncate => {
            (1, Type::Int)
        }
        Builtin::Int64ShiftLeft | Builtin::Int64ShiftRight | Builtin::Int64ShiftRightUnsigned => {
            (2, Type::Int64)
        }
        Builtin::Int64FromInt => (1, Type::Int64),
        Builtin::Int64ToInt => (1, Type::Int),
        _ => return Err(unsupported(&function.name, &format!("builtin {op:?}"))),
    };
    if args.len() != arity {
        return Err(invalid_flow_ir(
            &function.name,
            &format!(
                "builtin {op:?} has {} arguments, expected {arity}",
                args.len()
            ),
        ));
    }
    let arguments_match = match op {
        Builtin::IntBitAnd
        | Builtin::IntBitOr
        | Builtin::IntBitXor
        | Builtin::IntBitNot
        | Builtin::IntShiftLeft
        | Builtin::IntShiftRight
        | Builtin::IntShiftRightUnsigned => args
            .iter()
            .all(|operand| operand_is_int(layouts, function, operand)),
        Builtin::IntCheckedAdd => args
            .iter()
            .all(|operand| operand_is_int(layouts, function, operand)),
        Builtin::IntToString => args
            .iter()
            .all(|operand| operand_is_int(layouts, function, operand)),
        Builtin::FloatFromInt => args
            .iter()
            .all(|operand| operand_is_int(layouts, function, operand)),
        Builtin::FloatToString => operand_is_float(layouts, function, &args[0]),
        Builtin::FloatParse => operand_is_string(layouts, function, &args[0]),
        Builtin::FloatFloor | Builtin::FloatCeil | Builtin::FloatRound | Builtin::FloatTruncate => {
            operand_is_float(layouts, function, &args[0])
        }
        Builtin::StringFromCodePoint => args
            .iter()
            .all(|operand| operand_is_int(layouts, function, operand)),
        Builtin::StringLength | Builtin::StringChars => {
            operand_is_string(layouts, function, &args[0])
        }
        Builtin::StringCodePoints => operand_is_string(layouts, function, &args[0]),
        Builtin::StringSplit => args
            .iter()
            .all(|operand| operand_is_string(layouts, function, operand)),
        Builtin::StringJoin => {
            operand_is_string_list(layouts, function, &args[0])
                && operand_is_string(layouts, function, &args[1])
        }
        Builtin::IntParse => args
            .iter()
            .all(|operand| operand_is_string(layouts, function, operand)),
        Builtin::Int64ShiftLeft | Builtin::Int64ShiftRight | Builtin::Int64ShiftRightUnsigned => {
            operand_is_int64(layouts, function, &args[0])
                && operand_is_int(layouts, function, &args[1])
        }
        Builtin::Int64BitAnd
        | Builtin::Int64BitOr
        | Builtin::Int64BitXor
        | Builtin::Int64BitNot => args
            .iter()
            .all(|operand| operand_is_int64(layouts, function, operand)),
        Builtin::Int64FromInt => operand_is_int(layouts, function, &args[0]),
        Builtin::Int64ToInt => operand_is_int64(layouts, function, &args[0]),
        Builtin::Int64ToString => args
            .iter()
            .all(|operand| operand_is_int64(layouts, function, operand)),
        _ => args
            .iter()
            .all(|operand| operand_is_string(layouts, function, operand)),
    };
    if !arguments_match {
        return Err(invalid_flow_ir(
            &function.name,
            &format!("builtin {op:?} has an argument with the wrong type"),
        ));
    }
    let Some(dst) = dst else {
        return Err(invalid_flow_ir(
            &function.name,
            &format!("builtin {op:?} has no result destination"),
        ));
    };
    let result_matches = if matches!(op, Builtin::IntCheckedAdd | Builtin::IntParse) {
        option_payload_type(layouts, function.vars[dst].ty)
            .is_some_and(|ty| matches!(layouts.types[ty], Type::Int))
    } else if op == Builtin::FloatParse {
        option_payload_type(layouts, function.vars[dst].ty)
            .is_some_and(|ty| matches!(layouts.types[ty], Type::Float))
    } else if op == Builtin::StringFromCodePoint {
        option_payload_type(layouts, function.vars[dst].ty)
            .is_some_and(|ty| matches!(layouts.types[ty], Type::String))
    } else if op == Builtin::StringCodePoints {
        matches!(
            layouts.types[function.vars[dst].ty],
            Type::List(TypeId::INT)
        )
    } else if op == Builtin::StringSplit || op == Builtin::StringChars {
        matches!(
            layouts.types[function.vars[dst].ty],
            Type::List(TypeId::STRING)
        )
    } else {
        layouts.types[function.vars[dst].ty] == result_type
    };
    if !result_matches {
        return Err(invalid_flow_ir(
            &function.name,
            &format!("builtin {op:?} has a result with the wrong type"),
        ));
    }

    let expressions = args
        .iter()
        .map(operand_expression)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|variant| unsupported(&function.name, variant))?;
    if op == Builtin::StringSplit {
        let separator_may_be_empty = !matches!(
            args.get(1),
            Some(Operand::Const(Const::String(separator))) if !separator.is_empty()
        );
        emit_string_split(
            layouts,
            function,
            dst,
            &expressions[0],
            &expressions[1],
            separator_may_be_empty,
            writer,
        )?;
        return Ok(());
    }
    if op == Builtin::StringLength {
        emit_string_length(dst, &expressions[0], writer);
        return Ok(());
    }
    if op == Builtin::StringChars {
        emit_string_chars(layouts, function, dst, &expressions[0], writer)?;
        return Ok(());
    }
    if op == Builtin::StringJoin {
        let Operand::Var(list) = args[0] else {
            return Err(invalid_flow_ir(
                &function.name,
                "StringJoin first argument is not a List variable",
            ));
        };
        emit_string_join(
            layouts,
            function,
            dst,
            function.vars[list].ty,
            &expressions[0],
            &expressions[1],
            string_buffers,
            writer,
        )?;
        return Ok(());
    }
    if op == Builtin::StringCodePoints {
        emit_string_code_points(layouts, function, dst, &expressions[0], writer)?;
        return Ok(());
    }
    if op == Builtin::IntCheckedAdd {
        let ty = function.vars[dst].ty;
        let sum_name = format!("{}_checked_sum", variable_name(dst));
        writer.decl(
            "int64_t",
            &sum_name,
            Some(&format!(
                "((int64_t)({}) + (int64_t)({}))",
                expressions[0], expressions[1]
            )),
        );
        let none = option_variant_expression(layouts, function, ty, "None", None)?;
        let some = option_variant_expression(
            layouts,
            function,
            ty,
            "Some",
            Some(&format!("(int32_t){sum_name}")),
        )?;
        writer.begin_block(&format!(
            "if ({sum_name} < (int64_t)INT32_MIN || {sum_name} > (int64_t)INT32_MAX)"
        ));
        writer.assign(&variable_name(dst), &none);
        writer.end_block();
        writer.begin_block("else");
        writer.assign(&variable_name(dst), &some);
        writer.end_block();
        return Ok(());
    }
    if op == Builtin::IntParse {
        let ty = function.vars[dst].ty;
        let none = option_variant_expression(layouts, function, ty, "None", None)?;
        let parsed_name = format!("{}_parsed", variable_name(dst));
        let some = option_variant_expression(layouts, function, ty, "Some", Some(&parsed_name))?;
        if let Operand::Const(Const::String(value)) = &args[0] {
            let expression = match value.parse::<i32>() {
                Ok(parsed) => option_variant_expression(
                    layouts,
                    function,
                    ty,
                    "Some",
                    Some(&int32_literal(parsed)),
                )?,
                Err(_) => none,
            };
            writer.assign(&variable_name(dst), &expression);
        } else {
            writer.decl("int32_t", &parsed_name, None);
            writer.begin_block(&format!(
                "if (emela_emb_int_parse({}, &{parsed_name}))",
                expressions[0]
            ));
            writer.assign(&variable_name(dst), &some);
            writer.end_block();
            writer.begin_block("else");
            writer.assign(&variable_name(dst), &none);
            writer.end_block();
        }
        return Ok(());
    }
    if op == Builtin::FloatParse {
        let ty = function.vars[dst].ty;
        let none = option_variant_expression(layouts, function, ty, "None", None)?;
        if let Some(Operand::Const(Const::String(value))) = args.first() {
            let expression = match const_eval::parse_float(value) {
                Some(parsed) => option_variant_expression(
                    layouts,
                    function,
                    ty,
                    "Some",
                    Some(&float_literal(parsed)),
                )?,
                None => none,
            };
            writer.assign(&variable_name(dst), &expression);
        } else {
            let parsed_name = format!("{}_parsed", variable_name(dst));
            let some =
                option_variant_expression(layouts, function, ty, "Some", Some(&parsed_name))?;
            writer.decl("double", &parsed_name, None);
            writer.begin_block(&format!(
                "if (emela_emb_float_parse({}, &{parsed_name}))",
                expressions[0]
            ));
            writer.assign(&variable_name(dst), &some);
            writer.end_block();
            writer.begin_block("else");
            writer.assign(&variable_name(dst), &none);
            writer.end_block();
        }
        return Ok(());
    }
    if op == Builtin::StringFromCodePoint {
        let ty = function.vars[dst].ty;
        let none = option_variant_expression(layouts, function, ty, "None", None)?;
        if let Operand::Const(Const::Int(value)) = &args[0] {
            let expression = match u32::try_from(*value).ok().and_then(char::from_u32) {
                Some(character) => {
                    let string = constant_expression(&Const::String(character.to_string()))
                        .map_err(|variant| unsupported(&function.name, variant))?
                        .ok_or_else(|| {
                            invalid_flow_ir(
                                &function.name,
                                "String.from_code_point constant has no C expression",
                            )
                        })?;
                    option_variant_expression(layouts, function, ty, "Some", Some(&string))?
                }
                None => none,
            };
            writer.assign(&variable_name(dst), &expression);
        } else {
            let Some((buffer, capacity)) = string_buffers.buffer(dst) else {
                return Err(invalid_flow_ir(
                    &function.name,
                    "dynamic String.from_code_point has no planned output buffer",
                ));
            };
            let codepoint = &expressions[0];
            let value_name = format!("{}_code_point", variable_name(dst));
            let length_name = format!("{}_byte_length", variable_name(dst));
            let string_value =
                format!("(EmelaString){{ .data = {buffer}, .length = (uint16_t){length_name} }}");
            let some =
                option_variant_expression(layouts, function, ty, "Some", Some(&string_value))?;
            writer.begin_block(&format!(
                "if ({codepoint} < INT32_C(0) || {codepoint} > INT32_C(0x10ffff) || ({codepoint} >= INT32_C(0xd800) && {codepoint} <= INT32_C(0xdfff)))"
            ));
            writer.assign(&variable_name(dst), &none);
            writer.end_block();
            writer.begin_block("else");
            writer.decl(
                "uint32_t",
                &value_name,
                Some(&format!("(uint32_t)({codepoint})")),
            );
            writer.decl("uint16_t", &length_name, None);
            writer.begin_block(&format!("if ({value_name} <= UINT32_C(0x7f))"));
            writer.assign(&length_name, "UINT16_C(1)");
            writer.end_block();
            writer.begin_block(&format!("else if ({value_name} <= UINT32_C(0x7ff))"));
            writer.assign(&length_name, "UINT16_C(2)");
            writer.end_block();
            writer.begin_block(&format!("else if ({value_name} <= UINT32_C(0xffff))"));
            writer.assign(&length_name, "UINT16_C(3)");
            writer.end_block();
            writer.begin_block("else");
            writer.assign(&length_name, "UINT16_C(4)");
            writer.end_block();
            writer.begin_block(&format!("if ({length_name} > {capacity})"));
            writer.line(
                "emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"string capacity exceeded\", UINT16_C(24));",
            );
            writer.end_block();
            // Only the encodings that fit a fixed array are emitted: clang rejects
            // constant indexes past its end even behind the capacity defect above.
            let fits = |bytes: u16| {
                string_buffers
                    .array_capacity(dst)
                    .is_none_or(|capacity| bytes <= capacity)
            };
            writer.begin_block(&format!("if ({length_name} == UINT16_C(1))"));
            writer.assign(&format!("{buffer}[0]"), &format!("(char){value_name}"));
            writer.end_block();
            if fits(2) {
                writer.begin_block(&format!("else if ({length_name} == UINT16_C(2))"));
                writer.assign(
                    &format!("{buffer}[0]"),
                    &format!("(char)(UINT32_C(0xc0) | ({value_name} >> UINT32_C(6)))"),
                );
                writer.assign(
                    &format!("{buffer}[1]"),
                    &format!("(char)(UINT32_C(0x80) | ({value_name} & UINT32_C(0x3f)))"),
                );
                writer.end_block();
            }
            if fits(3) {
                writer.begin_block(&format!("else if ({length_name} == UINT16_C(3))"));
                writer.assign(
                    &format!("{buffer}[0]"),
                    &format!("(char)(UINT32_C(0xe0) | ({value_name} >> UINT32_C(12)))"),
                );
                writer.assign(
                    &format!("{buffer}[1]"),
                    &format!(
                        "(char)(UINT32_C(0x80) | (({value_name} >> UINT32_C(6)) & UINT32_C(0x3f)))"
                    ),
                );
                writer.assign(
                    &format!("{buffer}[2]"),
                    &format!("(char)(UINT32_C(0x80) | ({value_name} & UINT32_C(0x3f)))"),
                );
                writer.end_block();
            }
            if fits(4) {
                writer.begin_block(&format!("else if ({length_name} == UINT16_C(4))"));
                writer.assign(
                    &format!("{buffer}[0]"),
                    &format!("(char)(UINT32_C(0xf0) | ({value_name} >> UINT32_C(18)))"),
                );
                writer.assign(
                    &format!("{buffer}[1]"),
                    &format!(
                        "(char)(UINT32_C(0x80) | (({value_name} >> UINT32_C(12)) & UINT32_C(0x3f)))"
                    ),
                );
                writer.assign(
                    &format!("{buffer}[2]"),
                    &format!(
                        "(char)(UINT32_C(0x80) | (({value_name} >> UINT32_C(6)) & UINT32_C(0x3f)))"
                    ),
                );
                writer.assign(
                    &format!("{buffer}[3]"),
                    &format!("(char)(UINT32_C(0x80) | ({value_name} & UINT32_C(0x3f)))"),
                );
                writer.end_block();
            }
            writer.assign(&variable_name(dst), &some);
            writer.end_block();
        }
        return Ok(());
    }
    if matches!(
        op,
        Builtin::IntShiftLeft
            | Builtin::IntShiftRight
            | Builtin::IntShiftRightUnsigned
            | Builtin::Int64ShiftLeft
            | Builtin::Int64ShiftRight
            | Builtin::Int64ShiftRightUnsigned
    ) {
        writer.begin_block(&format!("if ({} < INT32_C(0))", expressions[1]));
        writer.line(&format!(
            "emela_emb_negative_shift_defect({});",
            expressions[1]
        ));
        writer.end_block();
    }
    let expression = match op {
        Builtin::StringAppend => {
            if let [
                Operand::Const(Const::String(left)),
                Operand::Const(Const::String(right)),
            ] = args
            {
                constant_expression(&Const::String(format!("{left}{right}")))
                    .map_err(|variant| unsupported(&function.name, variant))?
                    .ok_or_else(|| {
                        invalid_flow_ir(&function.name, "String append has no C expression")
                    })?
            } else if let [Operand::Const(Const::String(value)), other] = args
                && value.is_empty()
            {
                operand_expression(other).map_err(|variant| unsupported(&function.name, variant))?
            } else if let [other, Operand::Const(Const::String(value))] = args
                && value.is_empty()
            {
                operand_expression(other).map_err(|variant| unsupported(&function.name, variant))?
            } else {
                let Some((buffer, capacity)) = string_buffers.buffer(dst) else {
                    return Err(invalid_flow_ir(
                        &function.name,
                        "dynamic String append has no planned output buffer",
                    ));
                };
                // A literal cannot alias writable output. Keeping its copy unconditional
                // also avoids GCC diagnosing the impossible output-equals-literal branch.
                let copy_left = if matches!(args[0], Operand::Const(Const::String(_))) {
                    "INT32_C(1)".to_owned()
                } else {
                    format!("(({}).data != {buffer})", expressions[0])
                };
                format!(
                    "emela_emb_string_append({}, {}, {buffer}, {capacity}, {copy_left})",
                    expressions[0], expressions[1]
                )
            }
        }
        Builtin::StringByteSize => format!("(int32_t)(({}).length)", expressions[0]),
        Builtin::IntToString => match &args[0] {
            Operand::Const(Const::Int(value)) => {
                constant_expression(&Const::String(value.to_string()))
                    .map_err(|variant| unsupported(&function.name, variant))?
                    .expect("String constant has an expression")
            }
            _ => {
                let Some((buffer, capacity)) = string_buffers.buffer(dst) else {
                    return Err(invalid_flow_ir(
                        &function.name,
                        "dynamic Int.to_string has no planned output buffer",
                    ));
                };
                format!(
                    "emela_emb_int_to_string((int64_t)({}), {buffer}, {capacity})",
                    expressions[0]
                )
            }
        },
        Builtin::IntBitAnd => format!("({} & {})", expressions[0], expressions[1]),
        Builtin::IntBitOr => format!("({} | {})", expressions[0], expressions[1]),
        Builtin::IntBitXor => format!("({} ^ {})", expressions[0], expressions[1]),
        Builtin::IntBitNot => format!("(~{})", expressions[0]),
        Builtin::IntShiftLeft => format!(
            "emela_i32_shift_left({}, {})",
            expressions[0], expressions[1]
        ),
        Builtin::IntShiftRight => format!(
            "emela_i32_shift_right({}, {})",
            expressions[0], expressions[1]
        ),
        Builtin::IntShiftRightUnsigned => format!(
            "emela_i32_shift_right_unsigned({}, {})",
            expressions[0], expressions[1]
        ),
        Builtin::Int64ShiftLeft => format!(
            "emela_i64_shift_left({}, {})",
            expressions[0], expressions[1]
        ),
        Builtin::Int64ShiftRight => format!(
            "emela_i64_shift_right({}, {})",
            expressions[0], expressions[1]
        ),
        Builtin::Int64ShiftRightUnsigned => format!(
            "emela_i64_shift_right_unsigned({}, {})",
            expressions[0], expressions[1]
        ),
        Builtin::Int64BitAnd => format!("({} & {})", expressions[0], expressions[1]),
        Builtin::Int64BitOr => format!("({} | {})", expressions[0], expressions[1]),
        Builtin::Int64BitXor => format!("({} ^ {})", expressions[0], expressions[1]),
        Builtin::Int64BitNot => format!("(~{})", expressions[0]),
        Builtin::Int64FromInt => format!("((int64_t)({}))", expressions[0]),
        Builtin::Int64ToInt => {
            format!("emela_i32_from_bits((uint32_t)({}))", expressions[0])
        }
        Builtin::Int64ToString => match &args[0] {
            Operand::Const(Const::Int64(value)) => {
                constant_expression(&Const::String(value.to_string()))
                    .map_err(|variant| unsupported(&function.name, variant))?
                    .expect("String constant has an expression")
            }
            _ => {
                let Some((buffer, capacity)) = string_buffers.buffer(dst) else {
                    return Err(invalid_flow_ir(
                        &function.name,
                        "dynamic Int64.to_string has no planned output buffer",
                    ));
                };
                format!(
                    "emela_emb_int_to_string({}, {buffer}, {capacity})",
                    expressions[0]
                )
            }
        },
        Builtin::FloatToString => match &args[0] {
            Operand::Const(Const::Float(value)) => {
                constant_expression(&Const::String(const_eval::format_float(*value)))
                    .map_err(|variant| unsupported(&function.name, variant))?
                    .expect("String constant has an expression")
            }
            _ => {
                let Some((buffer, capacity)) = string_buffers.buffer(dst) else {
                    return Err(invalid_flow_ir(
                        &function.name,
                        "dynamic Float.to_string has no planned output buffer",
                    ));
                };
                format!(
                    "emela_emb_float_to_string({}, {buffer}, {capacity})",
                    expressions[0]
                )
            }
        },
        Builtin::FloatFromInt => format!("((double)({}))", expressions[0]),
        Builtin::FloatFloor => format!("emela_emb_float_to_int({}, UINT8_C(0))", expressions[0]),
        Builtin::FloatCeil => format!("emela_emb_float_to_int({}, UINT8_C(1))", expressions[0]),
        Builtin::FloatRound => format!("emela_emb_float_to_int({}, UINT8_C(2))", expressions[0]),
        Builtin::FloatTruncate => format!("emela_emb_float_to_int({}, UINT8_C(3))", expressions[0]),
        Builtin::StringContains => format!(
            "emela_emb_string_contains({}, {})",
            expressions[0], expressions[1]
        ),
        Builtin::StringStartsWith => format!(
            "emela_emb_string_starts_with({}, {})",
            expressions[0], expressions[1]
        ),
        Builtin::StringEndsWith => format!(
            "emela_emb_string_ends_with({}, {})",
            expressions[0], expressions[1]
        ),
        Builtin::StringTrim => format!("emela_emb_string_trim({})", expressions[0]),
        _ => unreachable!("builtin was validated above"),
    };
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

fn emit_cell_builtin(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    op: Builtin,
    args: &[Operand],
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let arity = if op == Builtin::CellSet { 2 } else { 1 };
    if args.len() != arity {
        return Err(invalid_flow_ir(
            &function.name,
            "Cell builtin has an invalid argument count",
        ));
    }
    if op == Builtin::CellNew {
        let content = operand_type(function, &args[0]).expect("Cell argument has a type");
        let cell_ty = if let Some(cell) = dst {
            let ty = function.vars[cell].ty;
            if !matches!(layouts.types[ty], Type::Cell(field) if field == content) {
                return Err(invalid_flow_ir(
                    &function.name,
                    "CellNew destination or initial value has the wrong type",
                ));
            }
            ty
        } else {
            *layouts
                .cell_types
                .iter()
                .find(|&&ty| matches!(layouts.types[ty], Type::Cell(field) if field == content))
                .expect("CellNew layout was collected")
        };
        let target = dst
            .map(variable_name)
            .unwrap_or_else(|| "emela_discarded_cell".into());
        if dst.is_none() {
            writer.begin_block("");
            writer.decl("void *", &target, None);
        }
        let storage_ty = layouts.cell_content_c_type(content);
        writer.assign(
            &target,
            &format!(
                "emela_heap_allocate(sizeof({storage_ty}), UINT32_C({}))",
                layouts.cell_kind(cell_ty)
            ),
        );
        if layouts.type_repr(content) != Repr::Void {
            let source =
                operand_expression(&args[0]).map_err(|v| unsupported(&function.name, v))?;
            let storage = format!("(*(({storage_ty} *){target}))");
            layouts.emit_owned_value_store(content, &source, &storage, writer);
        }
        if dst.is_none() {
            writer.line(&format!("emela_heap_release({target});"));
            writer.end_block();
        }
        return Ok(());
    }
    let Some(cell_ty) = operand_type(function, &args[0]) else {
        unreachable!()
    };
    let Type::Cell(content) = layouts.types[cell_ty] else {
        return Err(invalid_flow_ir(
            &function.name,
            "Cell builtin first argument is not a Cell",
        ));
    };
    let cell = operand_expression(&args[0]).map_err(|v| unsupported(&function.name, v))?;
    let storage_ty = layouts.cell_content_c_type(content);
    let storage = format!("(*(({storage_ty} *){cell}))");
    match op {
        Builtin::CellGet => match dst {
            Some(result) if function.vars[result].ty == content => {
                layouts.emit_owned_value_store(content, &storage, &variable_name(result), writer);
            }
            None => {}
            _ => {
                return Err(invalid_flow_ir(
                    &function.name,
                    "CellGet destination does not match the cell content type",
                ));
            }
        },
        Builtin::CellSet => {
            if dst.is_some() || operand_type(function, &args[1]) != Some(content) {
                return Err(invalid_flow_ir(
                    &function.name,
                    "CellSet destination or value type is invalid",
                ));
            }
            if layouts.type_repr(content) != Repr::Void {
                let source =
                    operand_expression(&args[1]).map_err(|v| unsupported(&function.name, v))?;
                writer.begin_block("");
                writer.decl(&storage_ty, "emela_cell_new_value", Some(&source));
                layouts.emit_own_value_in_place(content, "emela_cell_new_value", writer);
                if layouts.is_managed_value(content) {
                    writer.decl(&storage_ty, "emela_cell_old_value", Some(&storage));
                }
                // Publish before release: collectors must see the new edge, including self-assignment.
                writer.assign(&storage, "emela_cell_new_value");
                layouts.emit_release_value_expression(content, "emela_cell_old_value", writer);
                writer.end_block();
            }
        }
        _ => unreachable!("only Cell builtins use this emitter"),
    }
    Ok(())
}

// Keep the lowering context explicit across this emission boundary.
#[allow(clippy::too_many_arguments)]
fn emit_indirect_call(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    callee: VarId,
    args: &[Operand],
    on_fail: Option<OnFail>,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    emit_function_value_call(
        layouts,
        function,
        dst,
        function.vars[callee].ty,
        &variable_name(callee),
        args,
        on_fail,
        writer,
        names,
        string_buffers,
        loops,
        blocks,
        protects,
        next_temp,
        next_label,
    )
}

#[allow(clippy::too_many_arguments)]
fn emit_function_value_call(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    function_type_id: TypeId,
    callee_expression: &str,
    args: &[Operand],
    on_fail: Option<OnFail>,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    let Type::Fn(function_type) = layouts.types[function_type_id].clone() else {
        return Err(invalid_flow_ir(
            &function.name,
            "indirect call target is not a function value",
        ));
    };
    if function_type.suspends {
        return Err(unsupported(&function.name, "indirect call with suspend"));
    }
    if function_type.fails != on_fail.is_some() {
        return Err(invalid_flow_ir(
            &function.name,
            "indirect call failure edge does not match its function type",
        ));
    }
    let mut parameter_types: Vec<TypeId> = function_type
        .params
        .into_iter()
        .filter(|&parameter| layouts.type_repr(parameter) != Repr::Void)
        .collect();
    if function_type.ev {
        parameter_types.push(TypeId::EVIDENCE);
    }
    if args.len() != parameter_types.len() {
        return Err(invalid_flow_ir(
            &function.name,
            &format!(
                "indirect call has {} arguments, expected {}",
                args.len(),
                parameter_types.len()
            ),
        ));
    }

    let mut call_args = Vec::new();
    for (&parameter_type, argument) in parameter_types.iter().zip(args) {
        if operand_type(function, argument) != Some(parameter_type) {
            return Err(invalid_flow_ir(
                &function.name,
                "indirect call argument has the wrong type",
            ));
        }
        call_args.push(
            operand_expression(argument).map_err(|variant| unsupported(&function.name, variant))?,
        );
    }

    match dst {
        None => {}
        Some(_) if layouts.type_repr(function_type.ret) == Repr::Void => {
            return Err(invalid_flow_ir(
                &function.name,
                "void indirect call has a result destination",
            ));
        }
        Some(result) if function.vars[result].ty != function_type.ret => {
            return Err(invalid_flow_ir(
                &function.name,
                "indirect call result has the wrong type",
            ));
        }
        Some(_) => {}
    }

    let callee_name = callee_expression;
    writer.begin_block(&format!("if ({callee_name}.call == NULL)"));
    writer.line("emela_board_defect(EMELA_DEFECT_PANIC, \"null function value\", UINT16_C(19));");
    writer.end_block();
    if let Some(edge) = on_fail {
        let error_name = format!("emela_indirect_error_{}", *next_temp);
        *next_temp += 1;
        writer.begin_block("");
        writer.decl("EmelaError", &error_name, Some("{0}"));
        call_args.push(format!("&{error_name}"));
        call_args.insert(0, format!("{callee_name}.env"));
        let expression = format!("{callee_name}.call({})", call_args.join(", "));
        let discarded = emit_call_result(
            layouts,
            function_type.ret,
            dst,
            &expression,
            writer,
            next_temp,
        );
        writer.begin_block(&format!("if ({error_name}.tag != UINT32_C(0))"));
        emit_call_failure_edge(
            layouts,
            function,
            edge,
            &error_name,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        )?;
        writer.end_block();
        if let Some(value) = discarded {
            layouts.emit_release_value_expression(function_type.ret, &value, writer);
        }
        writer.end_block();
    } else {
        call_args.insert(0, format!("{callee_name}.env"));
        let expression = format!("{callee_name}.call({})", call_args.join(", "));
        if let Some(value) = emit_call_result(
            layouts,
            function_type.ret,
            dst,
            &expression,
            writer,
            next_temp,
        ) {
            layouts.emit_release_value_expression(function_type.ret, &value, writer);
        }
    }
    Ok(())
}

fn emit_call_result(
    layouts: &TypeLayouts<'_>,
    result: TypeId,
    dst: Option<VarId>,
    expression: &str,
    writer: &mut Writer,
    next_temp: &mut usize,
) -> Option<String> {
    if let Some(dst) = dst {
        writer.assign(&variable_name(dst), expression);
    } else if layouts.is_managed_value(result) {
        let discarded = format!("emela_discarded_call_{}", *next_temp);
        *next_temp += 1;
        writer.decl(&layouts.c_type(result), &discarded, Some(expression));
        return Some(discarded);
    } else {
        writer.line(&format!("{expression};"));
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn emit_handler_operation_call(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    handler: VarId,
    effect: EffectId,
    operation: usize,
    args: &[Operand],
    on_fail: Option<OnFail>,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    if layouts.types[function.vars[handler].ty] != Type::Handler(effect) {
        return Err(invalid_flow_ir(
            &function.name,
            "operation call handler type does not match its effect",
        ));
    }
    let Some(layout_index) = layouts.handler_layout_ids.get(&effect).copied() else {
        return Err(invalid_flow_ir(
            &function.name,
            "dynamic operation call has no handler layout",
        ));
    };
    let Some(&function_type) = layouts.handler_layouts[layout_index]
        .operations
        .get(operation)
    else {
        return Err(invalid_flow_ir(
            &function.name,
            "dynamic operation index is outside its handler layout",
        ));
    };
    let handler_expression = variable_name(handler);
    let handler_type = TypeLayouts::handler_type_name(effect);
    let handler_pointer = format!("(({handler_type} *){handler_expression})");
    writer.begin_block(&format!("if ({handler_expression} == NULL)"));
    writer
        .line("emela_board_defect(EMELA_DEFECT_PANIC, \"invalid effect handler\", UINT16_C(22));");
    writer.end_block();
    writer.begin_block(&format!("if (!{handler_pointer}->active)"));
    writer.line(&format!(
        "emela_board_defect(EMELA_DEFECT_PANIC, {handler_pointer}->released_message, {handler_pointer}->released_message_length);"
    ));
    writer.end_block();

    emit_function_value_call(
        layouts,
        function,
        dst,
        function_type,
        &format!("({handler_pointer}->op_{operation})"),
        args,
        on_fail,
        writer,
        names,
        string_buffers,
        loops,
        blocks,
        protects,
        next_temp,
        next_label,
    )
}

fn emit_handler_value(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    effect: EffectId,
    name: &str,
    operations: &[VarId],
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let Some(dst) = dst else {
        return Err(invalid_flow_ir(
            &function.name,
            "Handler has no destination",
        ));
    };
    if layouts.types[function.vars[dst].ty] != Type::Handler(effect) {
        return Err(invalid_flow_ir(
            &function.name,
            "Handler destination does not match its effect",
        ));
    }
    let Some(&layout_index) = layouts.handler_layout_ids.get(&effect) else {
        return Err(invalid_flow_ir(
            &function.name,
            "Handler has no runtime layout",
        ));
    };
    let layout = &layouts.handler_layouts[layout_index];
    if operations.len() != layout.operations.len()
        || operations
            .iter()
            .zip(&layout.operations)
            .any(|(&operation, &ty)| function.vars[operation].ty != ty)
    {
        return Err(invalid_flow_ir(
            &function.name,
            "Handler operation types do not match its runtime layout",
        ));
    }
    let handler_type = TypeLayouts::handler_type_name(effect);
    let destination = variable_name(dst);
    let kind = layouts.handler_kind(effect);
    writer.assign(
        &destination,
        &format!("emela_heap_allocate(sizeof({handler_type}), UINT32_C({kind}))"),
    );
    let target = format!("(({handler_type} *){destination})");
    writer.assign(&format!("{target}->active"), "INT32_C(1)");
    let message = format!("handler {name} was used after release");
    writer.assign(
        &format!("{target}->released_message"),
        &c_string_literal(message.as_bytes()),
    );
    writer.assign(
        &format!("{target}->released_message_length"),
        &format!("UINT16_C({})", message.len()),
    );
    for (index, &operation) in operations.iter().enumerate() {
        if layouts.core.effects[effect].ops[index].suspend {
            let message = format!(
                "resume called twice in operation `{}` of handler `{name}`",
                layouts.core.effects[effect].ops[index].name
            );
            writer.assign(
                &format!("{target}->resume_error_{index}"),
                &c_string_literal(message.as_bytes()),
            );
            writer.assign(
                &format!("{target}->resume_error_length_{index}"),
                &format!("UINT16_C({})", message.len()),
            );
        }
        if layouts.is_managed_value(function.vars[operation].ty) {
            emit_retain_operand(layouts, function, &Operand::Var(operation), writer);
        }
        writer.assign(&format!("{target}->op_{index}"), &variable_name(operation));
    }
    Ok(())
}

fn emit_evidence_extend(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    base: Option<VarId>,
    bindings: &[(EffectId, VarId)],
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let Some(dst) = dst else {
        return Err(invalid_flow_ir(
            &function.name,
            "EvExtend has no destination",
        ));
    };
    if !matches!(layouts.types[function.vars[dst].ty], Type::Evidence) {
        return Err(invalid_flow_ir(
            &function.name,
            "EvExtend destination is not Evidence",
        ));
    }
    if let Some(base) = base
        && !matches!(layouts.types[function.vars[base].ty], Type::Evidence)
    {
        return Err(invalid_flow_ir(
            &function.name,
            "EvExtend base is not Evidence",
        ));
    }
    if bindings.is_empty() {
        writer.assign(
            &variable_name(dst),
            &base.map_or_else(|| "NULL".to_owned(), variable_name),
        );
        return Ok(());
    }
    let suffix = u32::from(dst.into_raw());
    let chain = format!("emela_evidence_chain_{suffix}");
    let base_expression = base.map_or_else(|| "NULL".to_owned(), variable_name);
    writer.decl("void *", &chain, Some(&base_expression));
    if base.is_some() {
        writer.line(&format!("emela_heap_retain({chain});"));
    }
    let node_kind = layouts.evidence_node_kind();
    for (index, (effect, handler)) in bindings.iter().enumerate() {
        if layouts.types[function.vars[*handler].ty] != Type::Handler(*effect) {
            return Err(invalid_flow_ir(
                &function.name,
                "EvExtend binding does not match its handler effect",
            ));
        }
        let node = format!("emela_evidence_node_{suffix}_{index}");
        writer.decl(
            "emela_evidence_node *",
            &node,
            Some(&format!(
                "(emela_evidence_node *)emela_heap_allocate(sizeof(emela_evidence_node), UINT32_C({node_kind}))"
            )),
        );
        writer.assign(
            &format!("{node}->effect"),
            &format!("UINT32_C({})", u32::from(effect.into_raw())),
        );
        writer.assign(&format!("{node}->handler"), &variable_name(*handler));
        writer.line(&format!("emela_heap_retain({node}->handler);"));
        writer.assign(&format!("{node}->parent"), &chain);
        writer.assign(&chain, &node);
    }
    writer.assign(&variable_name(dst), &chain);
    Ok(())
}

fn emit_evidence_lookup(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    evidence: VarId,
    effect: EffectId,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let Some(dst) = dst else {
        return Err(invalid_flow_ir(
            &function.name,
            "EvLookup has no destination",
        ));
    };
    if layouts.types[function.vars[dst].ty] != Type::Handler(effect)
        || !matches!(layouts.types[function.vars[evidence].ty], Type::Evidence)
    {
        return Err(invalid_flow_ir(
            &function.name,
            "EvLookup has inconsistent evidence and handler types",
        ));
    }
    let cursor = format!("emela_evidence_cursor_{}", u32::from(dst.into_raw()));
    writer.decl(
        "emela_evidence_node *",
        &cursor,
        Some(&variable_name(evidence)),
    );
    writer.begin_block(&format!("while ({cursor} != NULL)"));
    writer.begin_block(&format!(
        "if ({cursor}->effect == UINT32_C({}))",
        u32::from(effect.into_raw())
    ));
    writer.assign(&variable_name(dst), &format!("{cursor}->handler"));
    writer.line(&format!("goto emela_evidence_lookup_done_{cursor};"));
    writer.end_block();
    writer.assign(&cursor, &format!("({cursor}->parent)"));
    writer.end_block();
    writer.line(
        "emela_board_defect(EMELA_DEFECT_PANIC, \"effect handler not found\", UINT16_C(23));",
    );
    writer.line(&format!("emela_evidence_lookup_done_{cursor}: ;"));
    Ok(())
}

fn emit_coerce(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    source: VarId,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let Some(dst) = dst else {
        return Ok(());
    };
    let source_ty = function.vars[source].ty;
    let destination_ty = function.vars[dst].ty;
    let source_name = variable_name(source);
    let destination_name = variable_name(dst);
    let source_is_any = matches!(layouts.types[source_ty], Type::Any);
    let destination_is_any = matches!(layouts.types[destination_ty], Type::Any);

    if (destination_is_any && layouts.is_runtime_handle(source_ty))
        || (source_is_any && layouts.is_runtime_handle(destination_ty))
    {
        if destination_is_any {
            writer.assign(
                &destination_name,
                &format!("(EmelaAny){{.payload.any_box = {source_name}}}"),
            );
        } else {
            let kind = if layouts.is_named_external(destination_ty, "Std.Fiber.Fiber") {
                layouts.child_kind()
            } else if layouts.is_cause(destination_ty) {
                layouts.cause_kind()
            } else {
                layouts.deferred_kind()
            };
            writer.line(&format!("if ({source_name}.tag != 0 || {source_name}.payload.any_box == NULL || emela_heap_header({source_name}.payload.any_box)->kind != UINT32_C({kind})) emela_scheduler_invalid();"));
            writer.assign(&destination_name, &format!("{source_name}.payload.any_box"));
        }
        layouts.emit_retain_value_expression(destination_ty, &destination_name, writer);
        return Ok(());
    }

    if matches!(layouts.types[source_ty], Type::Fn(_))
        && matches!(layouts.types[destination_ty], Type::Fn(_))
    {
        if let Some(&adapter) = layouts
            .function_adapter_ids
            .get(&(source_ty, destination_ty))
        {
            let environment = format!("emela_fn_coerce_env_for_var_{}", u32::from(dst.into_raw()));
            let kind = layouts.function_adapter_kind(adapter);
            writer.decl(
                &format!("emela_fn_coerce_env_{adapter} *"),
                &environment,
                Some(&format!(
                    "(emela_fn_coerce_env_{adapter} *)emela_heap_allocate(sizeof(emela_fn_coerce_env_{adapter}), UINT32_C({kind}))"
                )),
            );
            if layouts.is_managed_value(source_ty) {
                layouts.emit_retain_value_expression(source_ty, &source_name, writer);
            }
            writer.assign(&format!("{environment}->source"), &source_name);
            writer.assign(
                &format!("{destination_name}.call"),
                &function_coerce_adapter_name(adapter),
            );
            writer.assign(&format!("{destination_name}.env"), &environment);
        } else {
            writer.assign(
                &format!("{destination_name}.call"),
                &format!("{source_name}.call"),
            );
            writer.assign(
                &format!("{destination_name}.env"),
                &format!("{source_name}.env"),
            );
            if layouts.is_managed_value(source_ty) {
                layouts.emit_retain_value_expression(source_ty, &source_name, writer);
            }
        }
        return Ok(());
    }

    if destination_is_any && source_is_any {
        writer.assign(&destination_name, &source_name);
        layouts.emit_retain_value_expression(destination_ty, &destination_name, writer);
        return Ok(());
    }
    if destination_is_any {
        let Some(&box_type) = layouts.any_box_ids.get(&source_ty) else {
            return Err(unsupported(
                &function.name,
                "boxing an Any value without a concrete runtime layout",
            ));
        };
        let box_name = format!("emela_any_box_for_var_{}", u32::from(dst.into_raw()));
        let kind = layouts.any_box_kind();
        writer.decl(
            "emela_any_box *",
            &box_name,
            Some(&format!(
                "(emela_any_box *)emela_heap_allocate(sizeof(emela_any_box), UINT32_C({kind}))"
            )),
        );
        writer.assign(
            &format!("{box_name}->type"),
            &format!("UINT32_C({box_type})"),
        );
        layouts.emit_owned_value_store(
            source_ty,
            &source_name,
            &format!("{box_name}->payload.v{box_type}"),
            writer,
        );
        writer.assign(&format!("{destination_name}.tag"), "UINT32_C(0)");
        writer.assign(&format!("{destination_name}.payload.any_box"), &box_name);
        return Ok(());
    }
    if source_is_any {
        let Some(&box_type) = layouts.any_box_ids.get(&destination_ty) else {
            return Err(unsupported(
                &function.name,
                "unboxing an Any value without a concrete runtime layout",
            ));
        };
        let box_name = format!("((emela_any_box *){source_name}.payload.any_box)");
        writer.begin_block(&format!(
            "if ({source_name}.tag != UINT32_C(0) || {source_name}.payload.any_box == NULL)"
        ));
        writer.line("emela_board_defect(EMELA_DEFECT_PANIC, \"invalid Any unbox\", UINT16_C(16));");
        writer.end_block();
        writer.begin_block(&format!("if ({box_name}->type != UINT32_C({box_type}))"));
        writer.line("emela_board_defect(EMELA_DEFECT_PANIC, \"Any type mismatch\", UINT16_C(17));");
        writer.end_block();
        writer.assign(
            &destination_name,
            &format!("{box_name}->payload.v{box_type}"),
        );
        if layouts.is_managed_value(destination_ty) {
            layouts.emit_retain_value_expression(destination_ty, &destination_name, writer);
        }
        return Ok(());
    }

    if layouts.c_type(source_ty) == layouts.c_type(destination_ty) {
        writer.assign(&destination_name, &source_name);
        if layouts.is_managed_value(destination_ty) {
            layouts.emit_retain_value_expression(destination_ty, &destination_name, writer);
        }
        return Ok(());
    }
    Err(unsupported(
        &function.name,
        "function-value coercion requiring a runtime adapter",
    ))
}

fn emit_any_ownership_declarations(layouts: &TypeLayouts<'_>, writer: &mut Writer) {
    if !layouts.any_abi {
        return;
    }
    writer.line("static inline EMELA_UNUSED void emela_any_retain(EmelaAny value);");
    writer.line("static inline EMELA_UNUSED void emela_any_own(EmelaAny *value);");
    writer.line("static inline EMELA_UNUSED void emela_any_enqueue(EmelaAny value, emela_heap_block **pending, emela_heap_block **tail);");
    writer.line("static inline EMELA_UNUSED void emela_any_release(EmelaAny value);");
}

fn emit_any_ownership(layouts: &TypeLayouts<'_>, heap: bool, writer: &mut Writer) {
    if !layouts.any_abi {
        return;
    }
    writer.begin_block("static inline EMELA_UNUSED void emela_any_retain(EmelaAny value)");
    writer.line("(void)value;");
    if heap && layouts.any_box_required {
        writer.line("if (value.tag == 0) emela_heap_retain(value.payload.any_box);");
    }
    if layouts.has_error_ownership() {
        writer.line("if (value.tag != 0) emela_error_retain(value);");
    }
    writer.end_block();
    writer.begin_block("static inline EMELA_UNUSED void emela_any_own(EmelaAny *value)");
    writer.line("(void)value;");
    if heap && layouts.any_box_required {
        writer.line("if (value->tag == 0) emela_heap_retain(value->payload.any_box);");
    }
    if layouts.has_error_ownership() {
        writer.line("if (value->tag != 0) emela_error_own(value);");
    }
    writer.end_block();
    if heap {
        writer.begin_block("static inline EMELA_UNUSED void emela_any_enqueue(EmelaAny value, emela_heap_block **pending, emela_heap_block **tail)");
        writer.line("(void)value; (void)pending; (void)tail;");
        if layouts.any_box_required {
            writer.line(
                "if (value.tag == 0) emela_heap_enqueue(value.payload.any_box, pending, tail);",
            );
        }
        if layouts.has_error_ownership() {
            writer.line("if (value.tag != 0) emela_error_enqueue(value, pending, tail);");
        }
        writer.end_block();
    }
    writer.begin_block("static inline EMELA_UNUSED void emela_any_release(EmelaAny value)");
    if heap {
        writer.line("emela_heap_block *pending = NULL, *tail = NULL;");
        writer.line("emela_any_enqueue(value, &pending, &tail); emela_heap_drain(pending, tail);");
    } else {
        writer.line("(void)value;");
    }
    writer.end_block();
}

fn emit_public_string_ownership(writer: &mut Writer) {
    writer.begin_block("void emela_embedded_retain_string(const EmelaString *value)");
    writer.begin_block("if (value != NULL)");
    writer.line("emela_heap_retain(value->owner);");
    writer.end_block();
    writer.end_block();
    writer.blank();

    writer.begin_block("void emela_embedded_release_string(EmelaString *value)");
    writer.begin_block("if (value != NULL)");
    writer.line("emela_heap_release(value->owner);");
    writer.line("*value = (EmelaString){0};");
    writer.end_block();
    writer.end_block();
    writer.blank();
}

fn emit_runtime(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    op: RuntimeOp,
    args: &[Operand],
    string_buffers: &StringBufferPlan,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    if matches!(op, RuntimeOp::Hold | RuntimeOp::Unhold) {
        if dst.is_some() || !args.is_empty() {
            return Err(invalid_flow_ir(
                &function.name,
                "Hold/Unhold arity or result",
            ));
        }
        if layouts.has_unwind {
            let operation = if op == RuntimeOp::Hold {
                "hold"
            } else {
                "unhold"
            };
            writer.line(&format!(
                "if (emela_current_fiber != NULL) emela_interrupt_{operation}(emela_current_fiber);"
            ));
        }
        return Ok(());
    }
    if op == RuntimeOp::Retire {
        if dst.is_some() || args.len() != 2 {
            return Err(invalid_flow_ir(
                &function.name,
                "RuntimeOp::Retire must have two arguments and no destination",
            ));
        }
        let Operand::Var(handler) = args[0] else {
            return Err(invalid_flow_ir(
                &function.name,
                "RuntimeOp::Retire handler is not a variable",
            ));
        };
        let Type::Handler(effect) = layouts.types[function.vars[handler].ty] else {
            return Err(invalid_flow_ir(
                &function.name,
                "RuntimeOp::Retire argument is not a Handler value",
            ));
        };
        if operand_type(function, &args[1]) != Some(TypeId::STRING) {
            return Err(invalid_flow_ir(
                &function.name,
                "RuntimeOp::Retire name is not a String",
            ));
        }
        let name =
            operand_expression(&args[1]).map_err(|variant| unsupported(&function.name, variant))?;
        if !layouts.handler_layout_ids.contains_key(&effect) {
            return Err(unsupported(
                &function.name,
                "RuntimeOp::Retire without a runtime handler layout",
            ));
        }
        let expression = variable_name(handler);
        let pointer = format!(
            "(({} *){expression})",
            TypeLayouts::handler_type_name(effect)
        );
        writer.begin_block(&format!("if ({expression} == NULL)"));
        writer.line(
            "emela_board_defect(EMELA_DEFECT_PANIC, \"invalid handler release\", UINT16_C(24));",
        );
        writer.end_block();
        writer.assign(&format!("{pointer}->active"), "INT32_C(0)");
        writer.line(&format!("(void)({name});"));
        return Ok(());
    }
    if op == RuntimeOp::DebugPrint {
        if args.len() != 1 || !operand_is_string(layouts, function, &args[0]) {
            return Err(invalid_flow_ir(
                &function.name,
                "RuntimeOp::DebugPrint must have one String argument",
            ));
        }
        if dst.is_some() {
            return Err(invalid_flow_ir(
                &function.name,
                "RuntimeOp::DebugPrint has a result destination",
            ));
        }
        let value =
            operand_expression(&args[0]).map_err(|variant| unsupported(&function.name, variant))?;
        writer.line(&format!(
            "emela_board_debug(({value}).data, ({value}).length);"
        ));
        return Ok(());
    }
    if op != RuntimeOp::ShowString {
        return Err(unsupported(&function.name, &format!("RuntimeOp::{op:?}")));
    }
    if args.len() != 1 || !operand_is_string(layouts, function, &args[0]) {
        return Err(invalid_flow_ir(
            &function.name,
            "RuntimeOp::ShowString must have one String argument",
        ));
    }
    let Some(dst) = dst else {
        return Err(invalid_flow_ir(
            &function.name,
            "RuntimeOp::ShowString has no String destination",
        ));
    };
    if !matches!(layouts.types[function.vars[dst].ty], Type::String) {
        return Err(invalid_flow_ir(
            &function.name,
            "RuntimeOp::ShowString destination is not String",
        ));
    }

    let expression = match &args[0] {
        Operand::Const(Const::String(value)) => {
            constant_expression(&Const::String(show_string(value)))
                .map_err(|variant| unsupported(&function.name, variant))?
                .ok_or_else(|| {
                    invalid_flow_ir(&function.name, "RuntimeOp::ShowString has no C expression")
                })?
        }
        operand => {
            let input = operand_expression(operand)
                .map_err(|variant| unsupported(&function.name, variant))?;
            let Some((buffer, capacity)) = string_buffers.buffer(dst) else {
                return Err(invalid_flow_ir(
                    &function.name,
                    "dynamic RuntimeOp::ShowString has no planned output buffer",
                ));
            };
            format!("emela_emb_string_show({input}, {buffer}, {capacity})")
        }
    };
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

fn show_string(value: &str) -> String {
    let mut output = String::with_capacity(value.len().saturating_add(2));
    output.push('"');
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\t' => output.push_str("\\t"),
            '\r' => output.push_str("\\r"),
            '#' if characters.peek() == Some(&'{') => output.push_str("\\#"),
            control if control <= '\u{1f}' || control == '\u{7f}' => {
                output.push_str(&format!("\\u{{{:x}}}", control as u32));
            }
            character => output.push(character),
        }
    }
    output.push('"');
    output
}

fn emit_string_matches_at_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED int32_t emela_emb_string_matches_at(EmelaString value, EmelaString needle, uint32_t offset)",
    );
    writer.begin_block(
        "if (offset > (uint32_t)value.length || (uint32_t)needle.length > (uint32_t)value.length - offset)",
    );
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.begin_block("for (uint32_t i = 0; i < (uint32_t)needle.length; ++i)");
    writer.begin_block("if (value.data[offset + i] != needle.data[i])");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.end_block();
    writer.line("return INT32_C(1);");
    writer.end_block();
    writer.blank();
}

fn emit_string_contains_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED int32_t emela_emb_string_contains(EmelaString value, EmelaString needle)",
    );
    writer.begin_block("if (needle.length > value.length)");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.line("uint32_t last = (uint32_t)value.length - needle.length;");
    writer.begin_block("for (uint32_t offset = 0; offset <= last; ++offset)");
    writer.begin_block("if (emela_emb_string_matches_at(value, needle, offset))");
    writer.line("return INT32_C(1);");
    writer.end_block();
    writer.end_block();
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.blank();
}

fn emit_string_starts_with_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED int32_t emela_emb_string_starts_with(EmelaString value, EmelaString prefix)",
    );
    writer.line("return emela_emb_string_matches_at(value, prefix, UINT32_C(0));");
    writer.end_block();
    writer.blank();
}

fn emit_string_ends_with_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED int32_t emela_emb_string_ends_with(EmelaString value, EmelaString suffix)",
    );
    writer.begin_block("if (suffix.length > value.length)");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.line("uint32_t offset = (uint32_t)value.length - suffix.length;");
    writer.line("return emela_emb_string_matches_at(value, suffix, offset);");
    writer.end_block();
    writer.blank();
}

fn emit_string_append_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED EmelaString emela_emb_string_append(EmelaString left, EmelaString right, char *buffer, uint16_t capacity, int copy_left)",
    );
    writer.begin_block(
        "if (left.length > capacity || right.length > (uint16_t)(capacity - left.length))",
    );
    writer.line(
        "emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"string capacity exceeded\", UINT16_C(24));",
    );
    writer.end_block();
    writer.begin_block("if (copy_left)");
    writer.begin_block("for (uint32_t i = UINT32_C(0); i < (uint32_t)left.length; ++i)");
    writer.line("buffer[i] = left.data[i];");
    writer.end_block();
    writer.end_block();
    writer.begin_block("for (uint32_t i = UINT32_C(0); i < (uint32_t)right.length; ++i)");
    writer.line("buffer[(uint32_t)left.length + i] = right.data[i];");
    writer.end_block();
    writer.line(
        "return (EmelaString){ .data = buffer, .length = (uint16_t)(left.length + right.length) };",
    );
    writer.end_block();
    writer.blank();
}

fn emit_int_to_string_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED EmelaString emela_emb_int_to_string(int64_t value, char *buffer, uint16_t capacity)",
    );
    writer.line("uint64_t magnitude = value < INT64_C(0) ? (uint64_t)(-(value + INT64_C(1))) + UINT64_C(1) : (uint64_t)value;");
    writer.line("uint32_t digit_count = UINT32_C(1);");
    writer.begin_block("for (uint64_t remaining = magnitude; remaining >= UINT64_C(10); remaining /= UINT64_C(10))");
    writer.line("++digit_count;");
    writer.end_block();
    writer
        .line("uint32_t length = digit_count + (value < INT64_C(0) ? UINT32_C(1) : UINT32_C(0));");
    writer.begin_block("if (length > (uint32_t)capacity)");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"string capacity exceeded\", UINT16_C(24));",
    );
    writer.end_block();
    writer.line("uint32_t cursor = length;");
    writer.begin_block("do");
    writer.line("buffer[--cursor] = (char)('0' + magnitude % UINT64_C(10));");
    writer.line("magnitude /= UINT64_C(10);");
    writer.end_block();
    writer.line("while (magnitude != UINT64_C(0));");
    writer.begin_block("if (value < INT64_C(0))");
    writer.line("buffer[0] = '-';");
    writer.end_block();
    writer.line("return (EmelaString){ .data = buffer, .length = (uint16_t)length };");
    writer.end_block();
    writer.blank();
}

fn emit_string_output_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED EmelaString emela_emb_string_copy_to_output(EmelaString value, char *output, uint16_t capacity, uint16_t maximum_length)",
    );
    writer.begin_block(
        "if (value.length > capacity || value.length > maximum_length || (value.length != UINT16_C(0) && (output == NULL || value.data == NULL)))",
    );
    writer.line(
        "emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"string capacity exceeded\", UINT16_C(24));",
    );
    writer.end_block();
    writer.begin_block("if (value.data != output && value.length != UINT16_C(0))");
    writer.line("const uintptr_t source_address = (uintptr_t)value.data;");
    writer.line("const uintptr_t output_address = (uintptr_t)output;");
    writer.begin_block(
        "if (output_address > source_address && output_address - source_address < (uintptr_t)value.length)",
    );
    writer.line(
        "for (uint32_t index = (uint32_t)value.length; index > UINT32_C(0); --index) output[index - UINT32_C(1)] = value.data[index - UINT32_C(1)];",
    );
    writer.end_block();
    writer.begin_block("else");
    writer.line(
        "for (uint32_t index = UINT32_C(0); index < (uint32_t)value.length; ++index) output[index] = value.data[index];",
    );
    writer.end_block();
    writer.end_block();
    writer.line("return (EmelaString){ .data = output, .length = value.length, .owner = NULL };");
    writer.end_block();
    writer.blank();
}

fn emit_int_parse_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED int emela_emb_int_parse(EmelaString value, int32_t *result)",
    );
    writer.begin_block("if (value.length == UINT16_C(0))");
    writer.line("return 0;");
    writer.end_block();
    writer.line("uint32_t index = UINT32_C(0);");
    writer.line("int negative = 0;");
    writer.line("const uint8_t first = (uint8_t)value.data[index];");
    writer.begin_block("if (first == UINT8_C(0x2b) || first == UINT8_C(0x2d))");
    writer.line("negative = first == UINT8_C(0x2d);");
    writer.line("++index;");
    writer.end_block();
    writer.begin_block("if (index >= (uint32_t)value.length)");
    writer.line("return 0;");
    writer.end_block();
    writer.line("uint64_t limit = negative ? UINT64_C(2147483648) : UINT64_C(2147483647);");
    writer.line("uint64_t magnitude = UINT64_C(0);");
    writer.begin_block("for (; index < (uint32_t)value.length; ++index)");
    writer.line("const uint8_t byte = (uint8_t)value.data[index];");
    writer.begin_block("if (byte < UINT8_C(0x30) || byte > UINT8_C(0x39))");
    writer.line("return 0;");
    writer.end_block();
    writer.line("const uint64_t digit = (uint64_t)(byte - UINT8_C(0x30));");
    writer.begin_block("if (magnitude > (limit - digit) / UINT64_C(10))");
    writer.line("return 0;");
    writer.end_block();
    writer.line("magnitude = magnitude * UINT64_C(10) + digit;");
    writer.end_block();
    writer.begin_block("if (negative && magnitude == UINT64_C(2147483648))");
    writer.line("*result = INT32_MIN;");
    writer.end_block();
    writer.line("else if (negative)");
    writer.begin_block("");
    writer.line("*result = -(int32_t)magnitude;");
    writer.end_block();
    writer.line("else");
    writer.begin_block("");
    writer.line("*result = (int32_t)magnitude;");
    writer.end_block();
    writer.line("return 1;");
    writer.end_block();
    writer.blank();
}

fn emit_string_show_helpers(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED void emela_emb_string_show_put(char *buffer, uint16_t capacity, uint32_t *length, uint8_t byte)",
    );
    writer.begin_block("if (*length >= (uint32_t)capacity)");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"string capacity exceeded\", UINT16_C(24));",
    );
    writer.end_block();
    writer.line("buffer[*length] = (char)byte;");
    writer.line("++*length;");
    writer.end_block();
    writer.blank();

    writer.begin_block(
        "static inline EMELA_UNUSED EmelaString emela_emb_string_show(EmelaString value, char *buffer, uint16_t capacity)",
    );
    writer.line("uint32_t length = UINT32_C(0);");
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, UINT8_C(0x22));");
    writer.begin_block("for (uint32_t i = UINT32_C(0); i < (uint32_t)value.length; ++i)");
    writer.line("const uint8_t byte = (uint8_t)value.data[i];");
    writer.begin_block("if (byte == UINT8_C(0x22) || byte == UINT8_C(0x5c))");
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, UINT8_C(0x5c));");
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, byte);");
    writer.end_block();
    writer.begin_block(
        "else if (byte == UINT8_C(0x23) && i + UINT32_C(1) < (uint32_t)value.length && value.data[i + UINT32_C(1)] == '{')",
    );
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, UINT8_C(0x5c));");
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, byte);");
    writer.end_block();
    writer.begin_block(
        "else if (byte == UINT8_C(0x0a) || byte == UINT8_C(0x09) || byte == UINT8_C(0x0d))",
    );
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, UINT8_C(0x5c));");
    writer.line("const uint8_t escaped = byte == UINT8_C(0x0a) ? UINT8_C(0x6e) : (byte == UINT8_C(0x09) ? UINT8_C(0x74) : UINT8_C(0x72));");
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, escaped);");
    writer.end_block();
    writer.begin_block("else if (byte < UINT8_C(0x20) || byte == UINT8_C(0x7f))");
    writer.line("static const char digits[] = \"0123456789abcdef\";");
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, UINT8_C(0x5c));");
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, UINT8_C(0x75));");
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, UINT8_C(0x7b));");
    writer.begin_block("if (byte >= UINT8_C(0x10))");
    writer
        .line("emela_emb_string_show_put(buffer, capacity, &length, (uint8_t)digits[byte >> 4]);");
    writer.end_block();
    writer.line(
        "emela_emb_string_show_put(buffer, capacity, &length, (uint8_t)digits[byte & UINT8_C(0x0f)]);",
    );
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, UINT8_C(0x7d));");
    writer.end_block();
    writer.begin_block("else");
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, byte);");
    writer.end_block();
    writer.end_block();
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, UINT8_C(0x22));");
    writer.line("return (EmelaString){ .data = buffer, .length = (uint16_t)length };");
    writer.end_block();
    writer.blank();
}

fn emit_utf8_decode_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED int32_t emela_emb_utf8_decode(EmelaString value, uint32_t offset, uint32_t *code_point, uint32_t *width)",
    );
    writer.begin_block("if (offset >= (uint32_t)value.length)");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.line("const uint8_t first = (uint8_t)value.data[offset];");
    writer.begin_block("if (first <= UINT8_C(0x7f))");
    writer.line("*code_point = first;");
    writer.line("*width = UINT32_C(1);");
    writer.line("return INT32_C(1);");
    writer.end_block();
    writer.line("uint32_t count;");
    writer.line("uint32_t decoded;");
    writer.begin_block("if (first >= UINT8_C(0xc2) && first <= UINT8_C(0xdf))");
    writer.line("count = UINT32_C(2);");
    writer.line("decoded = (uint32_t)(first & UINT8_C(0x1f));");
    writer.end_block();
    writer.line("else if (first >= UINT8_C(0xe0) && first <= UINT8_C(0xef))");
    writer.begin_block("");
    writer.line("count = UINT32_C(3);");
    writer.line("decoded = (uint32_t)(first & UINT8_C(0x0f));");
    writer.end_block();
    writer.line("else if (first >= UINT8_C(0xf0) && first <= UINT8_C(0xf4))");
    writer.begin_block("");
    writer.line("count = UINT32_C(4);");
    writer.line("decoded = (uint32_t)(first & UINT8_C(0x07));");
    writer.end_block();
    writer.line("else");
    writer.begin_block("");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.begin_block("if (offset + count > (uint32_t)value.length)");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.begin_block("for (uint32_t i = UINT32_C(1); i < count; ++i)");
    writer.line("const uint8_t next = (uint8_t)value.data[offset + i];");
    writer.begin_block("if ((next & UINT8_C(0xc0)) != UINT8_C(0x80))");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.line("decoded = (decoded << 6) | (uint32_t)(next & UINT8_C(0x3f));");
    writer.end_block();
    writer.begin_block(
        "if (count == UINT32_C(3) && (decoded < UINT32_C(0x800) || (decoded >= UINT32_C(0xd800) && decoded <= UINT32_C(0xdfff))))",
    );
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.begin_block(
        "if (count == UINT32_C(4) && (decoded < UINT32_C(0x10000) || decoded > UINT32_C(0x10ffff)))",
    );
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.line("*code_point = decoded;");
    writer.line("*width = count;");
    writer.line("return INT32_C(1);");
    writer.end_block();
    writer.blank();
}

fn emit_string_trim_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED int32_t emela_emb_is_white_space(uint32_t code_point)",
    );
    writer.line(
        "return (code_point >= UINT32_C(0x0009) && code_point <= UINT32_C(0x000d)) || code_point == UINT32_C(0x0020) || code_point == UINT32_C(0x0085) || code_point == UINT32_C(0x00a0) || code_point == UINT32_C(0x1680) || (code_point >= UINT32_C(0x2000) && code_point <= UINT32_C(0x200a)) || code_point == UINT32_C(0x2028) || code_point == UINT32_C(0x2029) || code_point == UINT32_C(0x202f) || code_point == UINT32_C(0x205f) || code_point == UINT32_C(0x3000);",
    );
    writer.end_block();
    writer.blank();

    writer.begin_block(
        "static inline EMELA_UNUSED EmelaString emela_emb_string_trim(EmelaString value)",
    );
    writer.begin_block("if (value.length == UINT16_C(0))");
    writer.line("return value;");
    writer.end_block();
    writer.line("uint32_t start = UINT32_C(0);");
    writer.line("uint32_t end = (uint32_t)value.length;");
    writer.line("uint32_t code_point;");
    writer.line("uint32_t width;");
    writer.begin_block("while (start < end)");
    writer.begin_block("if (!emela_emb_utf8_decode(value, start, &code_point, &width))");
    writer.line("break;");
    writer.end_block();
    writer.begin_block("if (!emela_emb_is_white_space(code_point))");
    writer.line("break;");
    writer.end_block();
    writer.line("start += width;");
    writer.end_block();
    writer.begin_block("while (end > start)");
    writer.line("uint32_t begin = end - UINT32_C(1);");
    writer.begin_block(
        "while (begin > start && ((uint8_t)value.data[begin] & UINT8_C(0xc0)) == UINT8_C(0x80))",
    );
    writer.line("--begin;");
    writer.end_block();
    writer.begin_block(
        "if (!emela_emb_utf8_decode(value, begin, &code_point, &width) || begin + width != end)",
    );
    writer.line("break;");
    writer.end_block();
    writer.begin_block("if (!emela_emb_is_white_space(code_point))");
    writer.line("break;");
    writer.end_block();
    writer.line("end = begin;");
    writer.end_block();
    writer.line(
        "return (EmelaString){ .data = value.data + start, .length = (uint16_t)(end - start), .owner = value.owner };",
    );
    writer.end_block();
    writer.blank();
}

fn emit_string_join_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED void emela_emb_string_join_put(char *buffer, uint16_t capacity, uint32_t *length, EmelaString value)",
    );
    writer.begin_block("if ((uint32_t)value.length > (uint32_t)capacity - *length)");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"string capacity exceeded\", UINT16_C(24));",
    );
    writer.end_block();
    writer.line("for (uint32_t index = UINT32_C(0); index < (uint32_t)value.length; ++index)");
    writer.begin_block("");
    writer.line("buffer[*length + index] = value.data[index];");
    writer.end_block();
    writer.line("*length += (uint32_t)value.length;");
    writer.end_block();
    writer.blank();
}

// Keep the lowering context explicit across this emission boundary.
#[allow(clippy::too_many_arguments)]
fn emit_statement(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    statement: &Stmt,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    let managed_dst = statement
        .dst()
        .filter(|&dst| layouts.is_managed_value(function.vars[dst].ty));
    if let Some(dst) = managed_dst {
        emit_release_if_owned(layouts, function, dst, writer);
    }
    let retain_result = match statement {
        Stmt::Let {
            value:
                Value::ListHead(_)
                | Value::ListTail(_)
                | Value::Field { .. }
                | Value::TupleGet { .. }
                | Value::AsError { .. },
            ..
        } => true,
        Stmt::Let {
            value:
                Value::Builtin {
                    op: Builtin::StringTrim,
                    ..
                },
            ..
        } => true,
        Stmt::Let {
            value:
                Value::Builtin {
                    op: Builtin::StringAppend,
                    args,
                },
            ..
        } if append_has_empty_constant(args) => true,
        Stmt::Let {
            value: Value::EvLookup { .. },
            ..
        } => true,
        Stmt::Let {
            value: Value::EvExtend { bindings, .. },
            ..
        } if bindings.is_empty() => true,
        Stmt::Let {
            value: Value::List {
                elems,
                tail: Some(_),
            },
            ..
        } => elems.is_empty(),
        _ => false,
    };
    emit_statement_inner(
        layouts,
        function,
        statement,
        writer,
        names,
        string_buffers,
        loops,
        blocks,
        protects,
        next_temp,
        next_label,
    )?;
    if let Some(dst) = managed_dst {
        if retain_result {
            layouts.emit_retain_value_expression(
                function.vars[dst].ty,
                &variable_name(dst),
                writer,
            );
        }
        writer.assign(&owned_variable_name(dst), "INT32_C(1)");
        if string_buffers.discarded_results.contains(&dst) {
            emit_release_if_owned(layouts, function, dst, writer);
        }
    }
    if layouts.has_cause_raise
        && layouts.has_unwind
        && !function.suspends
        && matches!(
            statement,
            Stmt::Let {
                value: Value::Call { .. }
                    | Value::Fiber {
                        op: emela_core::FiberOp::Attempt { .. },
                        ..
                    },
                ..
            }
        )
    {
        emit_sync_interrupt_exit(
            layouts,
            function,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        )?;
    }
    Ok(())
}

// Keep the lowering context explicit across this emission boundary.
#[allow(clippy::too_many_arguments)]
fn emit_statement_inner(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    statement: &Stmt,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    match statement {
        Stmt::Let {
            dst,
            value,
            on_defect,
            on_suspend,
        } => {
            if on_defect.is_some() || on_suspend.is_some() {
                return Err(vec![Error {
                    kind: ErrorKind::InvalidFlowIr,
                    message: format!(
                        "invalid Flow IR in function `{}`: Flow-stage Let has a landing edge",
                        function.name
                    ),
                }]);
            }
            match value {
                Value::Const(value) => {
                    let expression = constant_expression(value).map_err(|kind| {
                        unsupported(&function.name, &format!("{kind} in Value::Const"))
                    })?;
                    match (dst, expression) {
                        (Some(dst), Some(expression)) => {
                            writer.assign(&variable_name(*dst), &expression);
                        }
                        (None, None) => {}
                        (Some(_), None) => {
                            return Err(vec![Error {
                                kind: ErrorKind::InvalidFlowIr,
                                message: format!(
                                    "invalid Flow IR in function `{}`: Unit constant has a destination",
                                    function.name
                                ),
                            }]);
                        }
                        (None, Some(_)) => {}
                    }
                }
                Value::Prim { op, ty, args } => {
                    let expression = primitive_expression(*op, *ty, args)
                        .map_err(|variant| unsupported(&function.name, variant))?;
                    if let Some(dst) = dst {
                        writer.assign(&variable_name(*dst), &expression);
                    } else {
                        writer.line(&format!("(void)({expression});"));
                    }
                }
                Value::Builtin { op, args } => {
                    emit_builtin(layouts, function, *dst, *op, args, string_buffers, writer)?;
                }
                Value::Runtime { op, args } => {
                    emit_runtime(layouts, function, *dst, *op, args, string_buffers, writer)?;
                }
                Value::Call {
                    callee:
                        Callee::Op {
                            handler,
                            effect,
                            op,
                        },
                    args,
                    on_fail,
                    ..
                } if layouts.core.effects[*effect].ops[*op].suspend => {
                    writer.begin_block("");
                    if on_fail.is_some() {
                        writer.line("EmelaError operation_error = {0};");
                    }
                    suspend::emit_sync_operation(
                        layouts,
                        function,
                        *dst,
                        *handler,
                        *effect,
                        *op,
                        args,
                        on_fail.map(|_| "&operation_error"),
                        writer,
                    )?;
                    if let Some(edge) = on_fail {
                        writer.begin_block("if (operation_error.tag != 0)");
                        emit_call_failure_edge(
                            layouts,
                            function,
                            *edge,
                            "operation_error",
                            writer,
                            names,
                            string_buffers,
                            loops,
                            blocks,
                            protects,
                            next_temp,
                            next_label,
                        )?;
                        writer.end_block();
                    }
                    writer.end_block();
                }
                Value::Call {
                    callee: Callee::Direct(id),
                    args,
                    suspends: false,
                    on_fail,
                } => {
                    let result_ty = layouts.function_results[id];
                    let name = names
                        .get(id)
                        .ok_or_else(|| unsupported(&function.name, "call to missing function"))?;
                    let mut args = args
                        .iter()
                        .map(operand_expression)
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|variant| unsupported(&function.name, variant))?;
                    let mut output_scope = false;
                    let mut string_output_args = None;
                    if string_buffers.has_string_output_call(*id) {
                        if let Some(dst) = dst {
                            let Some((buffer, capacity)) = string_buffers.buffer(*dst) else {
                                return Err(invalid_flow_ir(
                                    &function.name,
                                    "String-returning call has no planned output buffer",
                                ));
                            };
                            string_output_args = Some((buffer, capacity));
                        } else {
                            let suffix = *next_temp;
                            *next_temp += 1;
                            let buffer = format!("emela_call_string_discard_{suffix}");
                            writer.begin_block("");
                            writer.line(&format!("char {buffer}[{}];", string_buffers.capacity));
                            string_output_args =
                                Some((buffer, format!("UINT16_C({})", string_buffers.capacity)));
                            output_scope = true;
                        }
                    }
                    if let Some(edge) = on_fail {
                        let error_name = format!("emela_call_error_{}", *next_temp);
                        *next_temp += 1;
                        writer.begin_block("");
                        writer.decl("EmelaError", &error_name, Some("{0}"));
                        args.push(format!("&{error_name}"));
                        if let Some((buffer, capacity)) = string_output_args {
                            args.push(buffer);
                            args.push(capacity);
                        }
                        let expression = format!("{name}({})", args.join(", "));
                        let discarded = emit_call_result(
                            layouts,
                            result_ty,
                            *dst,
                            &expression,
                            writer,
                            next_temp,
                        );
                        writer.begin_block(&format!("if ({error_name}.tag != UINT32_C(0))"));
                        emit_call_failure_edge(
                            layouts,
                            function,
                            *edge,
                            &error_name,
                            writer,
                            names,
                            string_buffers,
                            loops,
                            blocks,
                            protects,
                            next_temp,
                            next_label,
                        )?;
                        writer.end_block();
                        if let Some(value) = discarded {
                            layouts.emit_release_value_expression(result_ty, &value, writer);
                        }
                        writer.end_block();
                    } else {
                        if let Some((buffer, capacity)) = string_output_args {
                            args.push(buffer);
                            args.push(capacity);
                        }
                        let expression = format!("{name}({})", args.join(", "));
                        if let Some(value) = emit_call_result(
                            layouts,
                            result_ty,
                            *dst,
                            &expression,
                            writer,
                            next_temp,
                        ) {
                            layouts.emit_release_value_expression(result_ty, &value, writer);
                        }
                    }
                    if output_scope {
                        writer.end_block();
                    }
                }
                Value::Call {
                    callee: Callee::Indirect(callee),
                    args,
                    suspends: false,
                    on_fail,
                } => {
                    emit_indirect_call(
                        layouts,
                        function,
                        *dst,
                        *callee,
                        args,
                        *on_fail,
                        writer,
                        names,
                        string_buffers,
                        loops,
                        blocks,
                        protects,
                        next_temp,
                        next_label,
                    )?;
                }
                Value::Call {
                    callee:
                        Callee::Op {
                            handler,
                            effect,
                            op,
                        },
                    args,
                    suspends: false,
                    on_fail,
                } => {
                    emit_handler_operation_call(
                        layouts,
                        function,
                        *dst,
                        *handler,
                        *effect,
                        *op,
                        args,
                        *on_fail,
                        writer,
                        names,
                        string_buffers,
                        loops,
                        blocks,
                        protects,
                        next_temp,
                        next_label,
                    )?;
                }
                Value::Call { .. } => {
                    return Err(unsupported(&function.name, "indirect or suspending calls"));
                }
                Value::Make { ctor, args } => {
                    emit_make(layouts, function, *dst, *ctor, args, writer)?;
                }
                Value::Field { base, ctor, index } => {
                    emit_field(layouts, function, *dst, *base, *ctor, *index, writer)?;
                }
                Value::Tag(base) => {
                    emit_tag(layouts, function, *dst, *base, writer)?;
                }
                Value::Tuple(args) => {
                    emit_tuple(layouts, function, *dst, args, writer)?;
                }
                Value::TupleGet { base, index } => {
                    emit_tuple_get(layouts, function, *dst, *base, *index, writer)?;
                }
                Value::List { elems, tail } => emit_list_value(
                    layouts,
                    function,
                    *dst,
                    elems,
                    tail.as_ref(),
                    writer,
                    next_temp,
                )?,
                Value::ListIsEmpty(list) => {
                    let Some(dst) = dst else {
                        return Ok(());
                    };
                    writer.assign(
                        &variable_name(*dst),
                        &format!(
                            "({} == NULL) ? INT32_C(1) : INT32_C(0)",
                            variable_name(*list)
                        ),
                    );
                }
                Value::ListHead(list) => {
                    emit_list_head(layouts, function, *dst, *list, writer)?;
                }
                Value::ListTail(list) => {
                    emit_list_tail(layouts, function, *dst, *list, writer)?;
                }
                Value::FnRef(target) => {
                    let Some(dst) = dst else {
                        return Ok(());
                    };
                    let Type::Fn(_) = layouts.types[function.vars[*dst].ty] else {
                        return Err(invalid_flow_ir(
                            &function.name,
                            "FnRef destination is not a function value",
                        ));
                    };
                    let name = names.get(target).ok_or_else(|| {
                        invalid_flow_ir(
                            &function.name,
                            "FnRef target is not reachable for C emission",
                        )
                    })?;
                    let destination = variable_name(*dst);
                    if let Some(&environment) = layouts.closure_functions.get(target) {
                        let closure_environment = &layouts.closure_environments[environment];
                        if closure_environment.function_type != function.vars[*dst].ty {
                            return Err(invalid_flow_ir(
                                &function.name,
                                "closure function type differs from its closure value type",
                            ));
                        }
                        let Type::Fn(function_type) = &layouts.types[function.vars[*dst].ty] else {
                            unreachable!()
                        };
                        let target_name = if function_type.suspends {
                            format!("{name}_push")
                        } else {
                            name.clone()
                        };
                        writer.assign(&format!("{destination}.call"), &target_name);
                        let kind = layouts.closure_environment_kind(environment);
                        writer.assign(
                            &format!("{destination}.env"),
                            &format!(
                                "emela_heap_allocate(sizeof(emela_closure_env_{environment}), UINT32_C({kind}))"
                            ),
                        );
                        for (capture_index, capture) in
                            closure_environment.captures.iter().enumerate()
                        {
                            if layouts.type_repr(capture.ty) == Repr::Void {
                                continue;
                            }
                            layouts.emit_owned_value_store(
                                capture.ty,
                                &variable_name(capture.source),
                                &format!(
                                    "((emela_closure_env_{environment} *){destination}.env)->c{capture_index}"
                                ),
                                writer,
                            );
                        }
                    } else {
                        let function_type = function.vars[*dst].ty;
                        writer.assign(
                            &format!("{destination}.call"),
                            &function_ref_adapter_name(*target, function_type),
                        );
                        writer.assign(&format!("{destination}.env"), "NULL");
                    }
                }
                Value::Fiber {
                    op: emela_core::FiberOp::CauseRaise { .. },
                    args,
                    on_fail: Some(edge),
                } => {
                    let [cause] = args.as_slice() else {
                        return Err(invalid_flow_ir(&function.name, "Cause.raise arity"));
                    };
                    let cause = operand_expression(cause)
                        .map_err(|why| unsupported(&function.name, why))?;
                    writer.begin_block("");
                    writer.line(&format!(
                        "emela_cause *raised = emela_cause_checked({cause});"
                    ));
                    writer.begin_block("if (raised->kind == 0)");
                    writer.line("EmelaError raised_error = raised->error; emela_error_retain(raised_error);");
                    emit_call_failure_edge(
                        layouts,
                        function,
                        *edge,
                        "raised_error",
                        writer,
                        names,
                        string_buffers,
                        loops,
                        blocks,
                        protects,
                        next_temp,
                        next_label,
                    )?;
                    writer.end_block();
                    writer.begin_block("else if (raised->kind == 2)");
                    if layouts.has_unwind {
                        writer.line("if (emela_current_fiber == NULL) emela_board_defect(EMELA_DEFECT_PANIC, \"fiber was interrupted\", UINT16_C(21));");
                        writer.line("emela_force_interrupt(emela_current_fiber);");
                        if !function.suspends {
                            emit_sync_interrupt_exit(
                                layouts,
                                function,
                                writer,
                                names,
                                string_buffers,
                                loops,
                                blocks,
                                protects,
                                next_temp,
                                next_label,
                            )?;
                        }
                    } else {
                        writer.line("emela_board_defect(EMELA_DEFECT_PANIC, \"fiber was interrupted\", UINT16_C(21));");
                    }
                    writer.end_block();
                    writer.begin_block("else");
                    writer.line(
                        "emela_board_defect(EMELA_DEFECT_ASSERT, \"invalid Cause\", UINT16_C(13));",
                    );
                    writer.end_block();
                    writer.end_block();
                }
                Value::Fiber {
                    op,
                    args,
                    on_fail: None,
                } => {
                    suspend::emit_fiber_value(
                        layouts,
                        function,
                        *dst,
                        *op,
                        args,
                        names,
                        string_buffers,
                        writer,
                    )?;
                }
                Value::Handler { name, effect, ops } => {
                    emit_handler_value(layouts, function, *dst, *effect, name, ops, writer)?;
                }
                Value::EvExtend { base, bindings } => {
                    emit_evidence_extend(layouts, function, *dst, *base, bindings, writer)?;
                }
                Value::EvLookup { ev, effect } => {
                    emit_evidence_lookup(layouts, function, *dst, *ev, *effect, writer)?;
                }
                Value::Coerce(source) => {
                    emit_coerce(layouts, function, *dst, *source, writer)?;
                }
                Value::IsError { error, enum_id } => {
                    if !matches!(layouts.types[function.vars[*error].ty], Type::Any) {
                        return Err(invalid_flow_ir(
                            &function.name,
                            "IsError operand is not an EmelaError value",
                        ));
                    }
                    let Some(dst) = dst else {
                        return Ok(());
                    };
                    if !layouts.core.enums[*enum_id].error {
                        return Err(invalid_flow_ir(
                            &function.name,
                            "IsError target is not an error enum",
                        ));
                    }
                    writer.assign(
                        &variable_name(*dst),
                        &format!(
                            "({}).tag == UINT32_C({}) ? INT32_C(1) : INT32_C(0)",
                            variable_name(*error),
                            enum_index(*enum_id) + 1
                        ),
                    );
                }
                Value::AsError { error, enum_id } => {
                    if !matches!(layouts.types[function.vars[*error].ty], Type::Any) {
                        return Err(invalid_flow_ir(
                            &function.name,
                            "AsError operand is not an EmelaError value",
                        ));
                    }
                    let Some(dst) = dst else {
                        return Ok(());
                    };
                    if layouts.error_enum(function.vars[*dst].ty) != Some(*enum_id) {
                        return Err(invalid_flow_ir(
                            &function.name,
                            "AsError destination does not match its error enum",
                        ));
                    }
                    writer.assign(
                        &variable_name(*dst),
                        &format!(
                            "{}.payload.e{}",
                            variable_name(*error),
                            enum_index(*enum_id)
                        ),
                    );
                }
                other => return Err(unsupported(&function.name, value_name(other))),
            }
            Ok(())
        }
        Stmt::If {
            dst,
            cond,
            then,
            else_,
        } => {
            let condition =
                operand_expression(cond).map_err(|variant| unsupported(&function.name, variant))?;
            writer.begin_block(&format!("if (({condition}) != INT32_C(0))"));
            emit_compound_sequence(
                layouts,
                function,
                *then,
                *dst,
                writer,
                names,
                string_buffers,
                loops,
                blocks,
                protects,
                next_temp,
                next_label,
            )?;
            writer.end_block();
            writer.begin_block("else");
            emit_compound_sequence(
                layouts,
                function,
                *else_,
                *dst,
                writer,
                names,
                string_buffers,
                loops,
                blocks,
                protects,
                next_temp,
                next_label,
            )?;
            writer.end_block();
            Ok(())
        }
        Stmt::Switch {
            dst,
            on,
            cases,
            default,
        } => emit_switch(
            layouts,
            function,
            *dst,
            *on,
            cases,
            *default,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        ),
        Stmt::Loop {
            dst,
            vars,
            inits,
            body,
        } => emit_loop(
            layouts,
            function,
            *dst,
            vars,
            inits,
            *body,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        ),
        Stmt::Block { dst, label, body } => {
            let exit = format!("emela_block_exit_{}", *next_label);
            *next_label += 1;
            writer.begin_block("");
            blocks.push(BlockTarget {
                label: *label,
                dst: *dst,
                exit: exit.clone(),
                used: false,
                protect_depth: protects.len(),
            });
            let result = emit_compound_sequence(
                layouts,
                function,
                *body,
                *dst,
                writer,
                names,
                string_buffers,
                loops,
                blocks,
                protects,
                next_temp,
                next_label,
            );
            let block = blocks.pop().expect("block target stack underflow");
            writer.end_block();
            if block.used {
                writer.line(&format!("{exit}: ;"));
            }
            result
        }
        Stmt::Protect { dst, body, cleanup } => emit_protect(
            layouts,
            function,
            *dst,
            *body,
            *cleanup,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        ),
        other => Err(unsupported(&function.name, statement_name(other))),
    }
}

// Keep the lowering context explicit across this emission boundary.
#[allow(clippy::too_many_arguments)]
fn emit_switch(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    on: SwitchOn,
    cases: &[emela_lir::Case],
    default: Option<emela_lir::SeqId>,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    let (scrutinee, tag_switch) = match on {
        SwitchOn::Tag(value) => {
            let ty = function.vars[value].ty;
            let Type::Enum(enum_id, _) = layouts.types[ty] else {
                return Err(invalid_flow_ir(
                    &function.name,
                    "tag switch scrutinee is not an enum",
                ));
            };
            if layouts.enum_is_i32(enum_id) {
                (variable_name(value), true)
            } else if layouts.enum_fields(ty).is_some() {
                (format!("{}.tag", variable_name(value)), true)
            } else {
                return Err(invalid_flow_ir(
                    &function.name,
                    "tag switch enum has no supported C representation",
                ));
            }
        }
        SwitchOn::Lit(value) => {
            let ty = function.vars[value].ty;
            if !matches!(layouts.types[ty], Type::Int | Type::Int64) {
                return Err(invalid_flow_ir(
                    &function.name,
                    "literal switch scrutinee is not Int or Int64",
                ));
            }
            (variable_name(value), false)
        }
    };

    if !tag_switch && default.is_none() {
        return Err(invalid_flow_ir(
            &function.name,
            "literal switch has no default sequence",
        ));
    }

    writer.begin_block(&format!("switch ({scrutinee})"));
    for case in cases {
        let label = match (&case.key, on) {
            (CaseKey::Variant(variant), SwitchOn::Tag(value)) => {
                let ty = function.vars[value].ty;
                let Type::Enum(enum_id, _) = layouts.types[ty] else {
                    unreachable!("tag switch was validated above")
                };
                if *variant >= layouts.core.enums[enum_id].variants.len() {
                    return Err(invalid_flow_ir(
                        &function.name,
                        "tag switch variant is out of range",
                    ));
                }
                let tag = i32::try_from(*variant).map_err(|_| {
                    invalid_flow_ir(&function.name, "tag switch variant does not fit Int32")
                })?;
                int32_literal(tag)
            }
            (CaseKey::Lit(Const::Int(value)), SwitchOn::Lit(scrutinee))
                if matches!(layouts.types[function.vars[scrutinee].ty], Type::Int) =>
            {
                int32_literal(*value)
            }
            (CaseKey::Lit(Const::Int64(value)), SwitchOn::Lit(scrutinee))
                if matches!(layouts.types[function.vars[scrutinee].ty], Type::Int64) =>
            {
                int64_literal(*value)
            }
            _ => {
                return Err(invalid_flow_ir(
                    &function.name,
                    "switch case key does not match its scrutinee",
                ));
            }
        };
        writer.begin_block(&format!("case {label}:"));
        emit_compound_sequence(
            layouts,
            function,
            case.body,
            dst,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        )?;
        writer.line("break;");
        writer.end_block();
    }

    if let Some(default) = default {
        writer.begin_block("default:");
        emit_compound_sequence(
            layouts,
            function,
            default,
            dst,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        )?;
        writer.line("break;");
        writer.end_block();
    } else {
        writer.begin_block("default:");
        writer.line("emela_board_defect(EMELA_DEFECT_NO_MATCH, \"no match\", UINT16_C(8));");
        writer.end_block();
    }
    writer.end_block();
    Ok(())
}

fn invalid_flow_ir(function: &str, reason: &str) -> Vec<Error> {
    vec![Error {
        kind: ErrorKind::InvalidFlowIr,
        message: format!("invalid Flow IR in function `{function}`: {reason}"),
    }]
}

fn emit_list_value(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    elems: &[Operand],
    tail: Option<&Operand>,
    writer: &mut Writer,
    next_temp: &mut usize,
) -> Result<(), Vec<Error>> {
    let Some(dst) = dst else {
        return Err(invalid_flow_ir(
            &function.name,
            "Value::List has no destination",
        ));
    };
    let list_ty = function.vars[dst].ty;
    let Type::List(element_ty) = layouts.types[list_ty].clone() else {
        return Err(invalid_flow_ir(
            &function.name,
            "Value::List destination is not a List",
        ));
    };
    if elems.is_empty() {
        let expression = match tail {
            Some(tail) => {
                operand_expression(tail).map_err(|variant| unsupported(&function.name, variant))?
            }
            None => "NULL".to_owned(),
        };
        writer.assign(&variable_name(dst), &expression);
        return Ok(());
    }

    let list_name = layouts.list_node_name(list_ty);
    let list_kind = layouts.list_index(list_ty);
    let temp = format!("emela_list_build_{}", *next_temp);
    *next_temp += 1;
    let node_temp = format!("emela_list_node_tmp_{}", *next_temp);
    *next_temp += 1;
    let initial_tail = match tail {
        Some(tail) => {
            operand_expression(tail).map_err(|variant| unsupported(&function.name, variant))?
        }
        None => "NULL".to_owned(),
    };
    writer.decl("void *", &temp, Some(&initial_tail));
    writer.decl("void *", &node_temp, None);
    if tail.is_some() {
        writer.line(&format!("emela_heap_retain({temp});"));
    }
    for elem in elems.iter().rev() {
        writer.assign(
            &node_temp,
            &format!(
                "({list_name} *)emela_heap_allocate(sizeof({list_name}), UINT32_C({list_kind}))"
            ),
        );
        if layouts.type_repr(element_ty) == Repr::Void {
            writer.assign(
                &format!("(({list_name} *){node_temp})->_unit"),
                "UINT8_C(0)",
            );
        } else {
            let expression =
                operand_expression(elem).map_err(|variant| unsupported(&function.name, variant))?;
            layouts.emit_owned_value_store(
                element_ty,
                &expression,
                &format!("(({list_name} *){node_temp})->head"),
                writer,
            );
        }
        writer.assign(&format!("(({list_name} *){node_temp})->tail"), &temp);
        writer.assign(&temp, &node_temp);
    }
    writer.assign(&variable_name(dst), &temp);
    Ok(())
}

fn emit_string_code_points(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: VarId,
    input: &str,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let list_ty = function.vars[dst].ty;
    if !matches!(layouts.types[list_ty], Type::List(TypeId::INT)) {
        return Err(invalid_flow_ir(
            &function.name,
            "StringCodePoints destination is not List[Int]",
        ));
    }

    let suffix = u32::from(dst.into_raw());
    let input_name = format!("emela_code_points_input_{suffix}");
    let offset_name = format!("emela_code_points_offset_{suffix}");
    let code_point_name = format!("emela_code_points_value_{suffix}");
    let width_name = format!("emela_code_points_width_{suffix}");
    let head_name = format!("emela_code_points_head_{suffix}");
    let last_name = format!("emela_code_points_last_{suffix}");
    let node_name = format!("emela_code_points_node_{suffix}");
    let list_node = layouts.list_node_name(list_ty);
    let list_kind = layouts.list_index(list_ty);

    writer.decl("EmelaString", &input_name, Some(input));
    writer.decl("uint32_t", &offset_name, Some("UINT32_C(0)"));
    writer.decl("uint32_t", &code_point_name, None);
    writer.decl("uint32_t", &width_name, None);
    writer.decl("void *", &head_name, Some("NULL"));
    writer.decl(&format!("{list_node} *"), &last_name, Some("NULL"));
    writer.decl(&format!("{list_node} *"), &node_name, None);

    writer.begin_block(&format!(
        "while ({offset_name} < (uint32_t){input_name}.length)"
    ));
    writer.begin_block(&format!(
        "if (!emela_emb_utf8_decode({input_name}, {offset_name}, &{code_point_name}, &{width_name}))"
    ));
    writer.line("emela_board_defect(EMELA_DEFECT_PANIC, \"invalid UTF-8 string\", UINT16_C(20));");
    writer.end_block();
    writer.assign(
        &node_name,
        &format!("({list_node} *)emela_heap_allocate(sizeof({list_node}), UINT32_C({list_kind}))"),
    );
    writer.assign(
        &format!("{node_name}->head"),
        &format!("(int32_t){code_point_name}"),
    );
    writer.assign(&format!("{node_name}->tail"), "NULL");
    writer.begin_block(&format!("if ({last_name} == NULL)"));
    writer.assign(&head_name, &node_name);
    writer.end_block();
    writer.begin_block("else");
    writer.assign(&format!("{last_name}->tail"), &node_name);
    writer.end_block();
    writer.assign(&last_name, &node_name);
    writer.line(&format!("{offset_name} += {width_name};"));
    writer.end_block();
    writer.assign(&variable_name(dst), &head_name);
    Ok(())
}

fn emit_string_length(dst: VarId, input: &str, writer: &mut Writer) {
    let suffix = u32::from(dst.into_raw());
    let input_name = format!("emela_string_length_input_{suffix}");
    let offset_name = format!("emela_string_length_offset_{suffix}");
    let next_name = format!("emela_string_length_next_{suffix}");
    let count_name = format!("emela_string_length_count_{suffix}");

    writer.decl("EmelaString", &input_name, Some(input));
    writer.decl("uint32_t", &offset_name, Some("UINT32_C(0)"));
    writer.decl("uint32_t", &next_name, None);
    writer.decl("int32_t", &count_name, Some("INT32_C(0)"));
    writer.begin_block(&format!(
        "while ({offset_name} < (uint32_t){input_name}.length)"
    ));
    writer.begin_block(&format!(
        "if (!emela_emb_grapheme_next({input_name}, {offset_name}, &{next_name}))"
    ));
    writer.line("emela_board_defect(EMELA_DEFECT_PANIC, \"invalid UTF-8 string\", UINT16_C(20));");
    writer.end_block();
    writer.assign(&offset_name, &next_name);
    writer.line(&format!("++{count_name};"));
    writer.end_block();
    writer.assign(&variable_name(dst), &count_name);
}

fn emit_string_split(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: VarId,
    input: &str,
    separator: &str,
    separator_may_be_empty: bool,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let list_ty = function.vars[dst].ty;
    if !matches!(layouts.types[list_ty], Type::List(TypeId::STRING)) {
        return Err(invalid_flow_ir(
            &function.name,
            "StringSplit destination is not List[String]",
        ));
    }

    let suffix = u32::from(dst.into_raw());
    let input_name = format!("emela_string_split_input_{suffix}");
    let separator_name = format!("emela_string_split_separator_{suffix}");
    let bytes_name = format!("emela_string_split_bytes_{suffix}");
    let owner_name = format!("emela_string_split_owner_{suffix}");
    let owner_created_name = format!("emela_string_split_owner_created_{suffix}");
    let head_name = format!("emela_string_split_head_{suffix}");
    let last_name = format!("emela_string_split_last_{suffix}");
    let node_name = format!("emela_string_split_node_{suffix}");
    let start_name = format!("emela_string_split_start_{suffix}");
    let scan_name = format!("emela_string_split_scan_{suffix}");
    let matched_name = format!("emela_string_split_matched_{suffix}");
    let separator_index_name = format!("emela_string_split_separator_index_{suffix}");
    let list_node = layouts.list_node_name(list_ty);
    let list_kind = layouts.list_index(list_ty);
    let string_owner_kind = layouts.string_owner_kind();
    writer.decl("EmelaString", &input_name, Some(input));
    writer.decl("EmelaString", &separator_name, Some(separator));

    writer.decl(
        "const char *",
        &bytes_name,
        Some(&format!("{input_name}.data")),
    );
    writer.decl("void *", &owner_name, Some(&format!("{input_name}.owner")));
    writer.decl("int", &owner_created_name, Some("INT32_C(0)"));
    writer.begin_block(&format!(
        "if ({owner_name} == NULL && {input_name}.length != UINT16_C(0))"
    ));
    writer.assign(
        &owner_name,
        &format!("emela_heap_allocate((size_t){input_name}.length, UINT32_C({string_owner_kind}))"),
    );
    writer.assign(&bytes_name, &format!("(const char *){owner_name}"));
    let copy_index_name = format!("emela_string_split_copy_index_{suffix}");
    writer.decl("uint32_t", &copy_index_name, Some("UINT32_C(0)"));
    writer.begin_block(&format!(
        "while ({copy_index_name} < (uint32_t){input_name}.length)"
    ));
    writer.line(&format!(
        "((char *){owner_name})[{copy_index_name}] = {input_name}.data[{copy_index_name}];"
    ));
    writer.line(&format!("++{copy_index_name};"));
    writer.end_block();
    writer.assign(&owner_created_name, "INT32_C(1)");
    writer.end_block();
    writer.begin_block(&format!("if ({bytes_name} == NULL)"));
    writer.assign(&bytes_name, "\"\"");
    writer.end_block();

    writer.decl("void *", &head_name, Some("NULL"));
    writer.decl(&format!("{list_node} *"), &last_name, Some("NULL"));
    writer.decl(&format!("{list_node} *"), &node_name, None);
    if separator_may_be_empty {
        writer.begin_block(&format!("if ({separator_name}.length == UINT16_C(0))"));
        let cluster_offset_name = format!("emela_string_split_cluster_offset_{suffix}");
        let cluster_next_name = format!("emela_string_split_cluster_next_{suffix}");
        let cluster_length_name = format!("emela_string_split_cluster_length_{suffix}");
        writer.decl("uint32_t", &cluster_offset_name, Some("UINT32_C(0)"));
        writer.decl("uint32_t", &cluster_next_name, None);
        writer.decl("uint32_t", &cluster_length_name, None);
        writer.begin_block(&format!(
            "while ({cluster_offset_name} < (uint32_t){input_name}.length)"
        ));
        writer.begin_block(&format!(
            "if (!emela_emb_grapheme_next({input_name}, {cluster_offset_name}, &{cluster_next_name}))"
        ));
        writer.line(
            "emela_board_defect(EMELA_DEFECT_PANIC, \"invalid UTF-8 string\", UINT16_C(20));",
        );
        writer.end_block();
        writer.assign(
            &cluster_length_name,
            &format!("{cluster_next_name} - {cluster_offset_name}"),
        );
        emit_string_split_push(
            writer,
            &node_name,
            &list_node,
            list_kind,
            &bytes_name,
            &cluster_offset_name,
            &cluster_length_name,
            &owner_name,
            &last_name,
            &head_name,
        );
        writer.assign(&cluster_offset_name, &cluster_next_name);
        writer.end_block();
        writer.end_block();
        writer.begin_block("else");
    }
    writer.decl("uint32_t", &start_name, Some("UINT32_C(0)"));
    writer.decl("uint32_t", &scan_name, Some("UINT32_C(0)"));
    writer.decl("int", &matched_name, None);
    writer.decl("uint32_t", &separator_index_name, None);
    writer.begin_block(&format!(
        "while ({scan_name} <= (uint32_t){input_name}.length && (uint32_t){separator_name}.length <= (uint32_t){input_name}.length - {scan_name})"
    ));
    writer.assign(&matched_name, "INT32_C(1)");
    writer.assign(&separator_index_name, "UINT32_C(0)");
    writer.begin_block(&format!(
        "while ({separator_index_name} < (uint32_t){separator_name}.length)"
    ));
    writer.begin_block(&format!(
        "if ({bytes_name}[{scan_name} + {separator_index_name}] != {separator_name}.data[{separator_index_name}])"
    ));
    writer.assign(&matched_name, "INT32_C(0)");
    writer.line("break;");
    writer.end_block();
    writer.line(&format!("++{separator_index_name};"));
    writer.end_block();
    writer.begin_block(&format!("if ({matched_name} != INT32_C(0))"));
    emit_string_split_push(
        writer,
        &node_name,
        &list_node,
        list_kind,
        &bytes_name,
        &start_name,
        &format!("{scan_name} - {start_name}"),
        &owner_name,
        &last_name,
        &head_name,
    );
    writer.line(&format!(
        "{scan_name} += (uint32_t){separator_name}.length;"
    ));
    writer.assign(&start_name, &scan_name);
    writer.end_block();
    writer.begin_block("else");
    writer.line(&format!("++{scan_name};"));
    writer.end_block();
    writer.end_block();
    emit_string_split_push(
        writer,
        &node_name,
        &list_node,
        list_kind,
        &bytes_name,
        &start_name,
        &format!("(uint32_t){input_name}.length - {start_name}"),
        &owner_name,
        &last_name,
        &head_name,
    );
    if separator_may_be_empty {
        writer.end_block();
    }
    writer.begin_block(&format!("if ({owner_created_name} != INT32_C(0))"));
    writer.line(&format!("emela_heap_release({owner_name});"));
    writer.end_block();
    writer.assign(&variable_name(dst), &head_name);
    Ok(())
}

// Keep the lowering context explicit across this emission boundary.
#[allow(clippy::too_many_arguments)]
fn emit_string_split_push(
    writer: &mut Writer,
    node_name: &str,
    list_node: &str,
    list_kind: u32,
    bytes: &str,
    start: &str,
    length: &str,
    owner: &str,
    last: &str,
    head: &str,
) {
    writer.assign(
        node_name,
        &format!("({list_node} *)emela_heap_allocate(sizeof({list_node}), UINT32_C({list_kind}))"),
    );
    writer.assign(
        &format!("{node_name}->head"),
        &format!(
            "(EmelaString){{ .data = {bytes} + ({start}), .length = (uint16_t)({length}), .owner = {owner} }}"
        ),
    );
    writer.line(&format!("emela_heap_retain({owner});"));
    writer.assign(&format!("{node_name}->tail"), "NULL");
    writer.begin_block(&format!("if ({last} == NULL)"));
    writer.assign(head, node_name);
    writer.end_block();
    writer.begin_block("else");
    writer.assign(&format!("{last}->tail"), node_name);
    writer.end_block();
    writer.assign(last, node_name);
}

fn emit_string_chars(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: VarId,
    input: &str,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let list_ty = function.vars[dst].ty;
    if !matches!(layouts.types[list_ty], Type::List(TypeId::STRING)) {
        return Err(invalid_flow_ir(
            &function.name,
            "StringChars destination is not List[String]",
        ));
    }

    let suffix = u32::from(dst.into_raw());
    let input_name = format!("emela_string_chars_input_{suffix}");
    let copy_index_name = format!("emela_string_chars_copy_index_{suffix}");
    let bytes_name = format!("emela_string_chars_bytes_{suffix}");
    let owner_name = format!("emela_string_chars_owner_{suffix}");
    let owner_created_name = format!("emela_string_chars_owner_created_{suffix}");
    let head_name = format!("emela_string_chars_head_{suffix}");
    let last_name = format!("emela_string_chars_last_{suffix}");
    let node_name = format!("emela_string_chars_node_{suffix}");
    let offset_name = format!("emela_string_chars_offset_{suffix}");
    let next_name = format!("emela_string_chars_next_{suffix}");
    let length_name = format!("emela_string_chars_length_{suffix}");
    let list_node = layouts.list_node_name(list_ty);
    let list_kind = layouts.list_index(list_ty);
    let string_owner_kind = layouts.string_owner_kind();

    writer.decl("EmelaString", &input_name, Some(input));
    writer.decl(
        "const char *",
        &bytes_name,
        Some(&format!("{input_name}.data")),
    );
    writer.decl("void *", &owner_name, Some(&format!("{input_name}.owner")));
    writer.decl("int", &owner_created_name, Some("INT32_C(0)"));
    writer.begin_block(&format!(
        "if ({owner_name} == NULL && {input_name}.length != UINT16_C(0))"
    ));
    writer.assign(
        &owner_name,
        &format!("emela_heap_allocate((size_t){input_name}.length, UINT32_C({string_owner_kind}))"),
    );
    writer.assign(&bytes_name, &format!("(const char *){owner_name}"));
    writer.decl("uint32_t", &copy_index_name, Some("UINT32_C(0)"));
    writer.begin_block(&format!(
        "while ({copy_index_name} < (uint32_t){input_name}.length)"
    ));
    writer.line(&format!(
        "((char *){owner_name})[{copy_index_name}] = {input_name}.data[{copy_index_name}];"
    ));
    writer.line(&format!("++{copy_index_name};"));
    writer.end_block();
    writer.assign(&owner_created_name, "INT32_C(1)");
    writer.end_block();
    writer.begin_block(&format!("if ({bytes_name} == NULL)"));
    writer.assign(&bytes_name, "\"\"");
    writer.end_block();

    writer.decl("void *", &head_name, Some("NULL"));
    writer.decl(&format!("{list_node} *"), &last_name, Some("NULL"));
    writer.decl(&format!("{list_node} *"), &node_name, None);
    writer.decl("uint32_t", &offset_name, Some("UINT32_C(0)"));
    writer.decl("uint32_t", &next_name, None);
    writer.decl("uint32_t", &length_name, None);
    writer.begin_block(&format!(
        "while ({offset_name} < (uint32_t){input_name}.length)"
    ));
    writer.begin_block(&format!(
        "if (!emela_emb_grapheme_next({input_name}, {offset_name}, &{next_name}))"
    ));
    writer.line("emela_board_defect(EMELA_DEFECT_PANIC, \"invalid UTF-8 string\", UINT16_C(20));");
    writer.end_block();
    writer.assign(&length_name, &format!("{next_name} - {offset_name}"));
    emit_string_split_push(
        writer,
        &node_name,
        &list_node,
        list_kind,
        &bytes_name,
        &offset_name,
        &length_name,
        &owner_name,
        &last_name,
        &head_name,
    );
    writer.assign(&offset_name, &next_name);
    writer.end_block();
    writer.begin_block(&format!("if ({owner_created_name} != INT32_C(0))"));
    writer.line(&format!("emela_heap_release({owner_name});"));
    writer.end_block();
    writer.assign(&variable_name(dst), &head_name);
    Ok(())
}

// Keep the lowering context explicit across this emission boundary.
#[allow(clippy::too_many_arguments)]
fn emit_string_join(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: VarId,
    list_ty: TypeId,
    parts: &str,
    separator: &str,
    string_buffers: &StringBufferPlan,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    if !matches!(layouts.types[list_ty], Type::List(TypeId::STRING)) {
        return Err(invalid_flow_ir(
            &function.name,
            "StringJoin first argument is not List[String]",
        ));
    }
    let Some((buffer, capacity)) = string_buffers.buffer(dst) else {
        return Err(invalid_flow_ir(
            &function.name,
            "StringJoin has no planned output buffer",
        ));
    };
    let suffix = u32::from(dst.into_raw());
    let list_node = layouts.list_node_name(list_ty);
    let cursor_name = format!("emela_string_join_cursor_{suffix}");
    let length_name = format!("emela_string_join_length_{suffix}");
    let first_name = format!("emela_string_join_first_{suffix}");

    writer.decl(
        &format!("const {list_node} *"),
        &cursor_name,
        Some(&format!("(const {list_node} *)({parts})")),
    );
    writer.decl("uint32_t", &length_name, Some("UINT32_C(0)"));
    writer.decl("int", &first_name, Some("INT32_C(1)"));
    writer.begin_block(&format!("while ({cursor_name} != NULL)"));
    writer.begin_block(&format!("if ({first_name} == INT32_C(0))"));
    writer.line(&format!(
        "emela_emb_string_join_put({buffer}, {capacity}, &{length_name}, {separator});"
    ));
    writer.end_block();
    writer.line(&format!(
        "emela_emb_string_join_put({buffer}, {capacity}, &{length_name}, {cursor_name}->head);"
    ));
    writer.assign(&first_name, "INT32_C(0)");
    writer.assign(
        &cursor_name,
        &format!("(const {list_node} *){cursor_name}->tail"),
    );
    writer.end_block();
    writer.assign(
        &variable_name(dst),
        &format!("(EmelaString){{ .data = {buffer}, .length = (uint16_t){length_name} }}"),
    );
    Ok(())
}

fn emit_list_head(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    list: VarId,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let list_ty = function.vars[list].ty;
    let Type::List(element_ty) = layouts.types[list_ty].clone() else {
        return Err(invalid_flow_ir(
            &function.name,
            "ListHead operand is not a List",
        ));
    };
    let has_repr = layouts.type_repr(element_ty) != Repr::Void;
    if dst.is_some() && !has_repr {
        return Err(invalid_flow_ir(
            &function.name,
            "ListHead destination does not match its element representation",
        ));
    }
    if let Some(dst) = dst {
        let node_type = layouts.list_node_name(list_ty);
        writer.assign(
            &variable_name(dst),
            &format!("(({node_type} *){})->head", variable_name(list)),
        );
    }
    Ok(())
}

fn emit_list_tail(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    list: VarId,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let Some(dst) = dst else {
        return Ok(());
    };
    let list_ty = function.vars[list].ty;
    if !matches!(layouts.types[list_ty], Type::List(_)) || function.vars[dst].ty != list_ty {
        return Err(invalid_flow_ir(
            &function.name,
            "ListTail operand and destination types differ",
        ));
    }
    let node_type = layouts.list_node_name(list_ty);
    writer.assign(
        &variable_name(dst),
        &format!("(({node_type} *){})->tail", variable_name(list)),
    );
    Ok(())
}

// Keep the lowering context explicit across this emission boundary.
#[allow(clippy::too_many_arguments)]
fn emit_loop(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    vars: &[VarId],
    inits: &[Operand],
    body: emela_lir::SeqId,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    if vars.len() != inits.len() {
        return Err(invalid_flow_ir(
            &function.name,
            "loop variable and initializer counts differ",
        ));
    }
    for (&variable, initial) in vars.iter().zip(inits) {
        let expression =
            operand_expression(initial).map_err(|variant| unsupported(&function.name, variant))?;
        emit_retain_operand(layouts, function, initial, writer);
        emit_release_if_owned(layouts, function, variable, writer);
        writer.assign(&variable_name(variable), &expression);
        if layouts.is_managed_value(function.vars[variable].ty) {
            writer.assign(&owned_variable_name(variable), "INT32_C(1)");
        }
    }
    writer.begin_block("for (;;)");
    loops.push(vars.to_vec());
    let result = emit_compound_sequence(
        layouts,
        function,
        body,
        dst,
        writer,
        names,
        string_buffers,
        loops,
        blocks,
        protects,
        next_temp,
        next_label,
    );
    loops.pop();
    result?;
    writer.line("break;");
    writer.end_block();
    Ok(())
}

// Keep the lowering context explicit across this emission boundary.
#[allow(clippy::too_many_arguments)]
fn emit_compound_sequence(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    sequence: emela_lir::SeqId,
    dst: Option<VarId>,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    let sequence = &function.seqs[sequence];
    for &statement in &sequence.stmts {
        emit_statement(
            layouts,
            function,
            &function.stmts[statement],
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        )?;
    }
    match &sequence.term {
        Term::Yield(value) => match (dst, value) {
            (Some(dst), Some(value)) => {
                emit_retain_operand(layouts, function, value, writer);
                let expression = operand_expression(value)
                    .map_err(|variant| unsupported(&function.name, variant))?;
                writer.assign(&variable_name(dst), &expression);
                if layouts.is_managed_value(function.vars[dst].ty) {
                    writer.assign(&owned_variable_name(dst), "INT32_C(1)");
                }
            }
            (Some(_), None) => {
                return Err(invalid_flow_ir(
                    &function.name,
                    "compound statement destination has a valueless arm",
                ));
            }
            (None, _) => {}
        },
        Term::Recur(args) => emit_recur(layouts, function, args, loops, writer, next_temp)?,
        Term::Break { label, value } => {
            let Some(target_index) = blocks.iter().rposition(|target| target.label == *label)
            else {
                return Err(invalid_flow_ir(
                    &function.name,
                    "Break target is not an enclosing emitted block",
                ));
            };
            let target_dst = blocks[target_index].dst;
            let exit = blocks[target_index].exit.clone();
            let protect_depth = blocks[target_index].protect_depth;
            match (target_dst, value) {
                (Some(dst), Some(value)) => {
                    if function.vars[dst].ty == TypeId::ANY
                        && operand_type(function, value)
                            .is_some_and(|ty| layouts.error_enum(ty).is_some())
                    {
                        emit_release_if_owned(layouts, function, dst, writer);
                        emit_error_assignment(
                            layouts,
                            function,
                            value,
                            &variable_name(dst),
                            writer,
                        )?;
                    } else {
                        emit_retain_operand(layouts, function, value, writer);
                        emit_release_if_owned(layouts, function, dst, writer);
                        let expression = operand_expression(value)
                            .map_err(|variant| unsupported(&function.name, variant))?;
                        writer.assign(&variable_name(dst), &expression);
                    }
                    if layouts.is_managed_value(function.vars[dst].ty) {
                        writer.assign(&owned_variable_name(dst), "INT32_C(1)");
                    }
                }
                (Some(_), None) => {
                    return Err(invalid_flow_ir(
                        &function.name,
                        "Break omits the enclosing block value",
                    ));
                }
                (None, _) => {}
            }
            emit_pending_cleanups(
                layouts,
                function,
                protect_depth,
                writer,
                names,
                string_buffers,
                loops,
                blocks,
                protects,
                next_temp,
                next_label,
            )?;
            blocks[target_index].used = true;
            writer.line(&format!("goto {exit};"));
        }
        Term::Return(_) => {
            emit_pending_cleanups(
                layouts,
                function,
                0,
                writer,
                names,
                string_buffers,
                loops,
                blocks,
                protects,
                next_temp,
                next_label,
            )?;
            emit_return(layouts, function, &sequence.term, string_buffers, writer)?;
        }
        Term::Fail(error) => {
            if !function.fails {
                return Err(invalid_flow_ir(
                    &function.name,
                    "Fail appears in a function without a failure channel",
                ));
            }
            emit_error_assignment(layouts, function, error, "*emela_error_out", writer)?;
            emit_pending_cleanups(
                layouts,
                function,
                0,
                writer,
                names,
                string_buffers,
                loops,
                blocks,
                protects,
                next_temp,
                next_label,
            )?;
            emit_failure_return(layouts, function, writer);
        }
        Term::Defect(message, _) => emit_defect(layouts, function, message, writer)?,
        Term::Unreachable => writer
            .line("emela_board_defect(EMELA_DEFECT_ASSERT, \"unreachable code\", UINT16_C(16));"),
        other => return Err(unsupported(&function.name, term_name(other))),
    }
    Ok(())
}

struct BlockTarget {
    label: LabelId,
    dst: Option<VarId>,
    exit: String,
    used: bool,
    protect_depth: usize,
}

#[derive(Clone, Copy)]
struct ProtectTarget {
    cleanup: emela_lir::SeqId,
}

fn emit_recur(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    args: &[Operand],
    loops: &[Vec<VarId>],
    writer: &mut Writer,
    next_temp: &mut usize,
) -> Result<(), Vec<Error>> {
    let Some(vars) = loops.last() else {
        return Err(invalid_flow_ir(
            &function.name,
            "Recur appears outside an emitted loop",
        ));
    };
    if vars.len() != args.len() {
        return Err(invalid_flow_ir(
            &function.name,
            "Recur argument and loop variable counts differ",
        ));
    }

    let mut temporaries = Vec::with_capacity(args.len());
    for (&variable, argument) in vars.iter().zip(args) {
        let expression =
            operand_expression(argument).map_err(|variant| unsupported(&function.name, variant))?;
        let temporary = format!("emela_recur_tmp_{}", *next_temp);
        *next_temp += 1;
        writer.decl(
            &layouts.c_type(function.vars[variable].ty),
            &temporary,
            Some(&expression),
        );
        temporaries.push(temporary);
    }
    for (&variable, temporary) in vars.iter().zip(&temporaries) {
        if layouts.is_managed_value(function.vars[variable].ty) {
            layouts.emit_retain_value_expression(function.vars[variable].ty, temporary, writer);
        }
    }
    for &variable in vars {
        emit_release_if_owned(layouts, function, variable, writer);
    }
    for (&variable, temporary) in vars.iter().zip(temporaries) {
        writer.assign(&variable_name(variable), &temporary);
        if layouts.is_managed_value(function.vars[variable].ty) {
            writer.assign(&owned_variable_name(variable), "INT32_C(1)");
        }
    }
    writer.line("continue;");
    Ok(())
}

// Keep the lowering context explicit across this emission boundary.
#[allow(clippy::too_many_arguments)]
fn emit_call_failure_edge(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    edge: OnFail,
    error_name: &str,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    match edge {
        OnFail::Break(label) => {
            let Some(target_index) = blocks.iter().rposition(|target| target.label == label) else {
                return Err(invalid_flow_ir(
                    &function.name,
                    "failing call target is not an enclosing emitted block",
                ));
            };
            let target_dst = blocks[target_index].dst;
            let protect_depth = blocks[target_index].protect_depth;
            let Some(target_dst) = target_dst else {
                return Err(invalid_flow_ir(
                    &function.name,
                    "failing call target block has no error destination",
                ));
            };
            if !matches!(layouts.types[function.vars[target_dst].ty], Type::Any) {
                return Err(invalid_flow_ir(
                    &function.name,
                    "failing call target does not use the EmelaError representation",
                ));
            }
            emit_release_if_owned(layouts, function, target_dst, writer);
            writer.assign(&variable_name(target_dst), error_name);
            writer.assign(error_name, "(EmelaError){0}");
            if layouts.is_managed_value(function.vars[target_dst].ty) {
                writer.assign(&owned_variable_name(target_dst), "INT32_C(1)");
            }
            emit_pending_cleanups(
                layouts,
                function,
                protect_depth,
                writer,
                names,
                string_buffers,
                loops,
                blocks,
                protects,
                next_temp,
                next_label,
            )?;
            blocks[target_index].used = true;
            writer.line(&format!("goto {};", blocks[target_index].exit));
            Ok(())
        }
        OnFail::Propagate => {
            if !function.fails {
                return Err(invalid_flow_ir(
                    &function.name,
                    "failure propagates through a function without a failure channel",
                ));
            }
            writer.assign("*emela_error_out", error_name);
            writer.assign(error_name, "(EmelaError){0}");
            emit_pending_cleanups(
                layouts,
                function,
                0,
                writer,
                names,
                string_buffers,
                loops,
                blocks,
                protects,
                next_temp,
                next_label,
            )?;
            emit_failure_return(layouts, function, writer);
            Ok(())
        }
        OnFail::Handle { .. } => Err(invalid_flow_ir(
            &function.name,
            "Heap IR failure landing appears in Flow IR",
        )),
    }
}

fn emit_error_assignment(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    error: &Operand,
    target: &str,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let Operand::Var(variable) = error else {
        return Err(invalid_flow_ir(
            &function.name,
            "Fail operand is not an error variable",
        ));
    };
    let ty = function.vars[*variable].ty;
    if matches!(layouts.types[ty], Type::Any) {
        writer.assign(target, &variable_name(*variable));
        writer.line(&format!("emela_error_retain({target});"));
        return Ok(());
    }
    let Some(enum_id) = layouts.error_enum(ty) else {
        return Err(invalid_flow_ir(
            &function.name,
            "Fail operand is not an error enum or propagated EmelaError",
        ));
    };
    if layouts.error_type(enum_id).is_none() {
        return Err(invalid_flow_ir(
            &function.name,
            "Fail operand error enum is missing from EmelaError layout",
        ));
    }
    let object = format!("({target})");
    writer.assign(target, "(EmelaError){0}");
    writer.assign(
        &format!("{object}.tag"),
        &format!("UINT32_C({})", enum_index(enum_id) + 1),
    );
    layouts.emit_owned_value_store(
        ty,
        &variable_name(*variable),
        &format!("{object}.payload.e{}", enum_index(enum_id)),
        writer,
    );
    Ok(())
}

fn emit_failure_return(layouts: &TypeLayouts<'_>, function: &Function, writer: &mut Writer) {
    emit_owned_cleanup(layouts, function, None, writer);
    if layouts.type_repr(function.ret) == Repr::Void {
        writer.ret(None);
    } else {
        writer.ret(Some(&format!("({}){{0}}", layouts.c_type(function.ret))));
    }
}

fn emit_defect(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    message: &Operand,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    match message {
        Operand::Const(Const::String(message)) => {
            let length = u16::try_from(message.len()).map_err(|_| {
                unsupported(
                    &function.name,
                    "a defect message longer than the embedded C ABI supports",
                )
            })?;
            writer.line(&format!(
                "emela_board_defect(EMELA_DEFECT_{}, {}, UINT16_C({length}));",
                defect_kind_for_message(message),
                c_string_literal(message.as_bytes())
            ));
            Ok(())
        }
        Operand::Var(variable)
            if matches!(layouts.types[function.vars[*variable].ty], Type::String) =>
        {
            let value = variable_name(*variable);
            writer.line(&format!(
                "emela_board_defect(emela_emb_defect_kind({value}), {value}.data, {value}.length);"
            ));
            Ok(())
        }
        _ => Err(invalid_flow_ir(
            &function.name,
            "Defect message is not a String value",
        )),
    }
}

fn defect_kind_for_message(message: &str) -> &'static str {
    if message == "not yet implemented" {
        "TODO"
    } else if message.starts_with("unreachable: no match arm matched") {
        "NO_MATCH"
    } else if message.starts_with("assertion failed") {
        "ASSERT"
    } else {
        "PANIC"
    }
}

fn has_dynamic_defect(flow: &emela_lir::Module, emitted: &HashSet<FnId>) -> bool {
    flow.functions.iter().any(|(id, function)| {
        emitted.contains(&id)
            && !function.constant
            && function.seqs.iter().any(|(_, sequence)| {
                matches!(
                    &sequence.term,
                    Term::Defect(Operand::Var(variable), _)
                        if matches!(flow.core.ty(function.vars[*variable].ty), Type::String)
                )
            })
    })
}

fn emit_dynamic_defect_kind_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED int emela_emb_string_matches(EmelaString value, const char *expected, uint16_t length, int exact)",
    );
    writer.begin_block("if (value.length < length || (exact && value.length != length))");
    writer.line("return 0;");
    writer.end_block();
    writer.begin_block("for (uint16_t i = 0; i < length; ++i)");
    writer.begin_block("if (value.data[i] != expected[i])");
    writer.line("return 0;");
    writer.end_block();
    writer.end_block();
    writer.line("return 1;");
    writer.end_block();
    writer.blank();
    writer.begin_block(
        "static inline EMELA_UNUSED EmelaDefectKind emela_emb_defect_kind(EmelaString message)",
    );
    for (message, kind, exact) in [
        ("not yet implemented", "TODO", "1"),
        ("unreachable: no match arm matched", "NO_MATCH", "1"),
        ("assertion failed", "ASSERT", "0"),
    ] {
        writer.begin_block(&format!(
            "if (emela_emb_string_matches(message, {}, UINT16_C({}), {exact}))",
            c_string_literal(message.as_bytes()),
            message.len(),
        ));
        writer.line(&format!("return EMELA_DEFECT_{kind};"));
        writer.end_block();
    }
    writer.line("return EMELA_DEFECT_PANIC;");
    writer.end_block();
    writer.blank();
}

// Keep the lowering context explicit across this emission boundary.
#[allow(clippy::too_many_arguments)]
fn emit_protect(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    body: emela_lir::SeqId,
    cleanup: emela_lir::SeqId,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    let body_yields = matches!(function.seqs[body].term, Term::Yield(_));
    writer.begin_block("");
    protects.push(ProtectTarget { cleanup });
    let body_result = emit_compound_sequence(
        layouts,
        function,
        body,
        dst,
        writer,
        names,
        string_buffers,
        loops,
        blocks,
        protects,
        next_temp,
        next_label,
    );
    protects.pop();
    let cleanup_result = if body_result.is_ok() && body_yields {
        emit_compound_sequence(
            layouts,
            function,
            cleanup,
            None,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        )
    } else {
        Ok(())
    };
    writer.end_block();
    body_result?;
    cleanup_result
}

// Keep the lowering context explicit across this emission boundary.
#[allow(clippy::too_many_arguments)]
fn emit_pending_cleanups(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    keep_depth: usize,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    if keep_depth > protects.len() {
        return Err(invalid_flow_ir(
            &function.name,
            "control transfer escapes a block outside the active protect stack",
        ));
    }
    let saved = protects.clone();
    for index in (keep_depth..saved.len()).rev() {
        protects.truncate(index);
        let result = emit_compound_sequence(
            layouts,
            function,
            saved[index].cleanup,
            None,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        );
        *protects = saved.clone();
        result?;
    }
    Ok(())
}

fn emit_make(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    ctor: CtorRef,
    args: &[Operand],
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let Some(dst) = dst else {
        return Ok(());
    };
    let ty = function.vars[dst].ty;
    let Type::Enum(enum_id, _) = layouts.types[ty] else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: enum constructor defines a non-enum value",
                function.name
            ),
        }]);
    };
    if enum_id != ctor.enum_id {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: enum constructor and destination types differ",
                function.name
            ),
        }]);
    }
    let Some(variant) = layouts.core.enums[enum_id].variants.get(ctor.variant) else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: enum constructor index is out of range",
                function.name
            ),
        }]);
    };
    if args.len() != variant.tys.len() {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: enum constructor has {} arguments, expected {}",
                function.name,
                args.len(),
                variant.tys.len()
            ),
        }]);
    }

    let target = variable_name(dst);
    if layouts.enum_is_i32(enum_id) {
        let tag = i32::try_from(ctor.variant)
            .map_err(|_| unsupported(&function.name, "enum with too many variants"))?;
        writer.assign(&target, &int32_literal(tag));
        return Ok(());
    }

    let Some(fields) = layouts
        .enum_fields(ty)
        .and_then(|variants| variants.get(ctor.variant))
    else {
        return Err(unsupported(
            &function.name,
            "enum value without an inline C layout",
        ));
    };
    let tag = i32::try_from(ctor.variant)
        .map_err(|_| unsupported(&function.name, "enum with too many variants"))?;
    writer.assign(&format!("{target}.tag"), &int32_literal(tag));

    let mut represented = false;
    for (index, (field_ty, arg)) in fields.iter().zip(args).enumerate() {
        if layouts.type_repr(*field_ty) == Repr::Void {
            continue;
        }
        represented = true;
        emit_aggregate_field_store(
            layouts,
            function,
            ty,
            ctor.variant,
            index,
            *field_ty,
            arg,
            &format!("{target}.payload.v{}.f{index}", ctor.variant),
            writer,
        )?;
    }
    if !represented {
        writer.assign(
            &format!("{target}.payload.v{}._unit", ctor.variant),
            "UINT8_C(0)",
        );
    }
    Ok(())
}

fn emit_field(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    base: VarId,
    ctor: CtorRef,
    index: usize,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let base_ty = function.vars[base].ty;
    let Type::Enum(enum_id, _) = layouts.types[base_ty] else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: field base is not an enum",
                function.name
            ),
        }]);
    };
    if enum_id != ctor.enum_id {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: field constructor and base types differ",
                function.name
            ),
        }]);
    }
    let Some(fields) = layouts
        .enum_fields(base_ty)
        .and_then(|variants| variants.get(ctor.variant))
    else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: field base has no inline enum layout",
                function.name
            ),
        }]);
    };
    let Some(&field_ty) = fields.get(index) else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: enum field index is out of range",
                function.name
            ),
        }]);
    };
    let has_repr = layouts.type_repr(field_ty) != Repr::Void;
    if dst.is_some() && !has_repr {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: enum field destination does not match its representation",
                function.name
            ),
        }]);
    }
    if let Some(dst) = dst {
        let access = format!("{}.payload.v{}.f{index}", variable_name(base), ctor.variant);
        let expression = if layouts.is_recursive_box_field(base_ty, ctor.variant, index) {
            format!("*(({} *){access})", layouts.c_type(field_ty))
        } else {
            access
        };
        writer.assign(&variable_name(dst), &expression);
    }
    Ok(())
}

fn emit_tag(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    base: VarId,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let Some(dst) = dst else {
        return Ok(());
    };
    let base_ty = function.vars[base].ty;
    let Type::Enum(enum_id, _) = layouts.types[base_ty] else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: tag base is not an enum",
                function.name
            ),
        }]);
    };
    let expression = if layouts.enum_is_i32(enum_id) {
        variable_name(base)
    } else if layouts.enum_fields(base_ty).is_some() {
        format!("{}.tag", variable_name(base))
    } else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: tag base has no supported enum representation",
                function.name
            ),
        }]);
    };
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

fn emit_tuple(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    args: &[Operand],
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let Some(dst) = dst else {
        return Ok(());
    };
    let ty = function.vars[dst].ty;
    let Type::Tuple(fields) = &layouts.types[ty] else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: tuple value defines a non-tuple variable",
                function.name
            ),
        }]);
    };
    if args.len() != fields.len() {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: tuple has {} elements, expected {}",
                function.name,
                args.len(),
                fields.len()
            ),
        }]);
    }

    let target = variable_name(dst);
    let mut represented = false;
    for (index, (&field_ty, arg)) in fields.iter().zip(args).enumerate() {
        if layouts.type_repr(field_ty) == Repr::Void {
            continue;
        }
        represented = true;
        emit_aggregate_field_store(
            layouts,
            function,
            ty,
            0,
            index,
            field_ty,
            arg,
            &format!("{target}.f{index}"),
            writer,
        )?;
    }
    if !represented {
        writer.assign(&format!("{target}._unit"), "UINT8_C(0)");
    }
    Ok(())
}

fn emit_tuple_get(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    base: VarId,
    index: usize,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let base_ty = function.vars[base].ty;
    let Type::Tuple(fields) = &layouts.types[base_ty] else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: tuple projection base is not a tuple",
                function.name
            ),
        }]);
    };
    let Some(&field_ty) = fields.get(index) else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: tuple field index is out of range",
                function.name
            ),
        }]);
    };
    let has_repr = layouts.type_repr(field_ty) != Repr::Void;
    if dst.is_some() && !has_repr {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: tuple projection destination does not match its representation",
                function.name
            ),
        }]);
    }
    if let Some(dst) = dst {
        let access = format!("{}.f{index}", variable_name(base));
        let expression = if layouts.is_recursive_box_field(base_ty, 0, index) {
            format!("*(({} *){access})", layouts.c_type(field_ty))
        } else {
            access
        };
        writer.assign(&variable_name(dst), &expression);
    }
    Ok(())
}

// Keep the lowering context explicit across this emission boundary.
#[allow(clippy::too_many_arguments)]
fn emit_aggregate_field_store(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    owner_ty: TypeId,
    variant: usize,
    field: usize,
    field_ty: TypeId,
    value: &Operand,
    target: &str,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let expression = operand_expression(value).map_err(|kind| unsupported(&function.name, kind))?;
    if layouts.is_recursive_box_field(owner_ty, variant, field) {
        let box_kind = layouts.recursive_box_kind(field_ty).ok_or_else(|| {
            invalid_flow_ir(
                &function.name,
                "recursive aggregate field has no heap box layout",
            )
        })?;
        let field_type = layouts.c_type(field_ty);
        writer.assign(
            target,
            &format!("emela_heap_allocate(sizeof({field_type}), UINT32_C({box_kind}))"),
        );
        layouts.emit_owned_value_store(
            field_ty,
            &expression,
            &format!("*(({field_type} *){target})"),
            writer,
        );
    } else {
        if layouts.is_managed_value(field_ty) {
            emit_retain_operand(layouts, function, value, writer);
        }
        writer.assign(target, &expression);
    }
    Ok(())
}

fn validate_external(flow: &emela_lir::Module, function: &Function) -> Result<String, Vec<Error>> {
    if crate::fiber::is_infallible_cause_fail(function) {
        if function.params.len() != 1
            || function.vars[function.params[0]].ty != TypeId::ANY
            || function.ret != TypeId::NEVER
            || function.fails
            || function.suspends
        {
            return Err(invalid_flow_ir(
                &function.name,
                "invalid backend Cause landing binding",
            ));
        }
        return Ok(crate::fiber::INFALLIBLE_CAUSE_FAIL.into());
    }
    let Some(external) = &function.external else {
        return Err(unsupported(&function.name, "missing external binding"));
    };
    let Some(binding) = external.binding("c") else {
        return Err(unsupported(
            &function.name,
            "external function without a C binding",
        ));
    };
    let symbol = match &binding.host {
        emela_core::HostRef::Global(name) => name.clone(),
        emela_core::HostRef::Import { name, .. } => name.clone(),
    };
    if !is_c_identifier(&symbol) {
        return Err(unsupported(
            &function.name,
            "external function with an invalid C symbol",
        ));
    }

    let supported_type = |ty| {
        matches!(
            flow.core.ty(ty),
            emela_core::Type::Int
                | emela_core::Type::Int64
                | emela_core::Type::Float
                | emela_core::Type::Bool
                | emela_core::Type::Unit
                | emela_core::Type::Never
        )
    };
    if function
        .params
        .iter()
        .any(|&param| !supported_type(function.vars[param].ty))
        || !supported_type(function.ret)
    {
        return Err(unsupported(
            &function.name,
            "external function with a non-scalar signature",
        ));
    }
    Ok(symbol)
}

fn is_c_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic())
        || !chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
    {
        return false;
    }
    !matches!(
        value,
        "auto"
            | "break"
            | "case"
            | "char"
            | "const"
            | "continue"
            | "default"
            | "do"
            | "double"
            | "else"
            | "enum"
            | "extern"
            | "float"
            | "for"
            | "goto"
            | "if"
            | "inline"
            | "int"
            | "long"
            | "register"
            | "restrict"
            | "return"
            | "short"
            | "signed"
            | "sizeof"
            | "static"
            | "struct"
            | "switch"
            | "typedef"
            | "union"
            | "unsigned"
            | "void"
            | "volatile"
            | "while"
            | "_Alignas"
            | "_Alignof"
            | "_Atomic"
            | "_Bool"
            | "_Complex"
            | "_Generic"
            | "_Imaginary"
            | "_Noreturn"
            | "_Static_assert"
            | "_Thread_local"
    )
}

fn primitive_expression(op: PrimOp, ty: PrimTy, args: &[Operand]) -> Result<String, &'static str> {
    let args: Vec<String> = args
        .iter()
        .map(operand_expression)
        .collect::<Result<_, _>>()?;
    match ty {
        PrimTy::I32 => primitive_i32_expression(op, args.as_slice()),
        PrimTy::I64 => primitive_i64_expression(op, args.as_slice()),
        PrimTy::F64 => primitive_f64_expression(op, args.as_slice()),
        PrimTy::String => primitive_string_expression(op, args.as_slice()),
    }
}

fn primitive_string_expression(op: PrimOp, args: &[String]) -> Result<String, &'static str> {
    let [left, right] = args else {
        return Err("String comparison with an invalid arity");
    };
    let comparison = match op {
        PrimOp::Eq => "==",
        PrimOp::Ne => "!=",
        PrimOp::Lt => "<",
        PrimOp::Le => "<=",
        PrimOp::Gt => ">",
        PrimOp::Ge => ">=",
        _ => return Err("unsupported String primitive"),
    };
    comparison_expression(
        &format!("emela_emb_string_compare(&({left}), &({right}))"),
        comparison,
        "INT32_C(0)",
    )
}

fn emit_string_comparison_helper(writer: &mut Writer) {
    writer.begin_block("static inline EMELA_UNUSED int32_t emela_emb_string_compare(const EmelaString *left, const EmelaString *right)");
    // Valid UTF-8 has the same lexicographic byte order as Unicode scalar values.
    writer.line("uint32_t length = left->length < right->length ? left->length : right->length;");
    writer.begin_block("for (uint32_t i = 0; i < length; i++)");
    writer.line("uint8_t a = (uint8_t)left->data[i], b = (uint8_t)right->data[i];");
    writer.line("if (a < b) return -INT32_C(1);");
    writer.line("if (a > b) return INT32_C(1);");
    writer.end_block();
    writer.line("return left->length < right->length ? -INT32_C(1) : left->length > right->length ? INT32_C(1) : INT32_C(0);");
    writer.end_block();
    writer.blank();
}

fn has_runtime_string_comparison(flow: &emela_lir::Module, emitted: &HashSet<FnId>) -> bool {
    flow.functions.iter().any(|(id, function)| {
        if !emitted.contains(&id) || function.constant { return false; }
        let statements = runtime_statements(function);
        function.stmts.iter().any(|(id, statement)| statements.contains(&id) && matches!(statement,
            Stmt::Let { value: Value::Prim { ty: PrimTy::String, op, .. }, .. } if op.is_comparison()))
    })
}

fn primitive_i32_expression(op: PrimOp, args: &[String]) -> Result<String, &'static str> {
    match (op, args) {
        (PrimOp::Add, [lhs, rhs]) => Ok(format!(
            "emela_i32_from_bits((uint32_t)((uint64_t)(uint32_t)({lhs}) + (uint64_t)(uint32_t)({rhs})))"
        )),
        (PrimOp::Sub, [lhs, rhs]) => Ok(format!(
            "emela_i32_from_bits((uint32_t)((uint64_t)(uint32_t)({lhs}) - (uint64_t)(uint32_t)({rhs})))"
        )),
        (PrimOp::Mul, [lhs, rhs]) => Ok(format!(
            "emela_i32_from_bits((uint32_t)((uint64_t)(uint32_t)({lhs}) * (uint64_t)(uint32_t)({rhs})))"
        )),
        (PrimOp::Neg, [value]) => Ok(format!(
            "emela_i32_from_bits((uint32_t)(UINT64_C(0) - (uint64_t)(uint32_t)({value})))"
        )),
        (PrimOp::Not, [value]) => Ok(format!(
            "(({value}) == INT32_C(0) ? INT32_C(1) : INT32_C(0))"
        )),
        (PrimOp::Eq, [lhs, rhs]) => comparison_expression(lhs, "==", rhs),
        (PrimOp::Ne, [lhs, rhs]) => comparison_expression(lhs, "!=", rhs),
        (PrimOp::Lt, [lhs, rhs]) => comparison_expression(lhs, "<", rhs),
        (PrimOp::Le, [lhs, rhs]) => comparison_expression(lhs, "<=", rhs),
        (PrimOp::Gt, [lhs, rhs]) => comparison_expression(lhs, ">", rhs),
        (PrimOp::Ge, [lhs, rhs]) => comparison_expression(lhs, ">=", rhs),
        (PrimOp::Div, [lhs, rhs]) => Ok(format!("emela_i32_div({lhs}, {rhs})")),
        (PrimOp::Rem, [lhs, rhs]) => Ok(format!("emela_i32_rem({lhs}, {rhs})")),
        _ => Err("Value::Prim with an invalid arity"),
    }
}

fn primitive_i64_expression(op: PrimOp, args: &[String]) -> Result<String, &'static str> {
    match (op, args) {
        (PrimOp::Add, [lhs, rhs]) => Ok(format!(
            "emela_i64_from_bits((uint64_t)({lhs}) + (uint64_t)({rhs}))"
        )),
        (PrimOp::Sub, [lhs, rhs]) => Ok(format!(
            "emela_i64_from_bits((uint64_t)({lhs}) - (uint64_t)({rhs}))"
        )),
        (PrimOp::Mul, [lhs, rhs]) => Ok(format!(
            "emela_i64_from_bits((uint64_t)({lhs}) * (uint64_t)({rhs}))"
        )),
        (PrimOp::Neg, [value]) => Ok(format!(
            "emela_i64_from_bits(UINT64_C(0) - (uint64_t)({value}))"
        )),
        (PrimOp::Eq, [lhs, rhs]) => comparison_expression(lhs, "==", rhs),
        (PrimOp::Ne, [lhs, rhs]) => comparison_expression(lhs, "!=", rhs),
        (PrimOp::Lt, [lhs, rhs]) => comparison_expression(lhs, "<", rhs),
        (PrimOp::Le, [lhs, rhs]) => comparison_expression(lhs, "<=", rhs),
        (PrimOp::Gt, [lhs, rhs]) => comparison_expression(lhs, ">", rhs),
        (PrimOp::Ge, [lhs, rhs]) => comparison_expression(lhs, ">=", rhs),
        (PrimOp::Div, [lhs, rhs]) => Ok(format!("emela_i64_div({lhs}, {rhs})")),
        (PrimOp::Rem, [lhs, rhs]) => Ok(format!("emela_i64_rem({lhs}, {rhs})")),
        _ => Err("Value::Prim with an invalid arity"),
    }
}

fn primitive_f64_expression(op: PrimOp, args: &[String]) -> Result<String, &'static str> {
    match (op, args) {
        (PrimOp::Add, [lhs, rhs]) => Ok(format!("(({lhs}) + ({rhs}))")),
        (PrimOp::Sub, [lhs, rhs]) => Ok(format!("(({lhs}) - ({rhs}))")),
        (PrimOp::Mul, [lhs, rhs]) => Ok(format!("(({lhs}) * ({rhs}))")),
        (PrimOp::Div, [lhs, rhs]) => Ok(format!("(({lhs}) / ({rhs}))")),
        (PrimOp::Rem, [lhs, rhs]) => Ok(format!("fmod(({lhs}), ({rhs}))")),
        (PrimOp::Neg, [value]) => Ok(format!("(-({value}))")),
        (PrimOp::Eq, [lhs, rhs]) => comparison_expression(lhs, "==", rhs),
        (PrimOp::Ne, [lhs, rhs]) => comparison_expression(lhs, "!=", rhs),
        (PrimOp::Lt, [lhs, rhs]) => comparison_expression(lhs, "<", rhs),
        (PrimOp::Le, [lhs, rhs]) => comparison_expression(lhs, "<=", rhs),
        (PrimOp::Gt, [lhs, rhs]) => comparison_expression(lhs, ">", rhs),
        (PrimOp::Ge, [lhs, rhs]) => comparison_expression(lhs, ">=", rhs),
        _ => Err("Value::Prim with an invalid arity"),
    }
}

fn emit_i32_shift_helpers(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED int32_t emela_i32_shift_left(int32_t value, int32_t by)",
    );
    writer.begin_block("if (by >= INT32_C(32))");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.line("return emela_i32_from_bits((uint32_t)value << (uint32_t)by);");
    writer.end_block();
    writer.blank();

    writer.begin_block(
        "static inline EMELA_UNUSED int32_t emela_i32_shift_right(int32_t value, int32_t by)",
    );
    writer.begin_block("if (by >= INT32_C(32))");
    writer.line("return value < INT32_C(0) ? -INT32_C(1) : INT32_C(0);");
    writer.end_block();
    writer.line("uint32_t shifted = (uint32_t)value >> (uint32_t)by;");
    writer.begin_block("if (by > INT32_C(0) && value < INT32_C(0))");
    writer.line("shifted |= UINT32_MAX << (UINT32_C(32) - (uint32_t)by);");
    writer.end_block();
    writer.line("return emela_i32_from_bits(shifted);");
    writer.end_block();
    writer.blank();

    writer.begin_block(
        "static inline EMELA_UNUSED int32_t emela_i32_shift_right_unsigned(int32_t value, int32_t by)",
    );
    writer.begin_block("if (by >= INT32_C(32))");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.line("return emela_i32_from_bits((uint32_t)value >> (uint32_t)by);");
    writer.end_block();
    writer.blank();
}

fn emit_i64_shift_helpers(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED int64_t emela_i64_shift_left(int64_t value, int32_t by)",
    );
    writer.begin_block("if (by >= INT32_C(64))");
    writer.line("return INT64_C(0);");
    writer.end_block();
    writer.line("return emela_i64_from_bits((uint64_t)value << (uint32_t)by);");
    writer.end_block();
    writer.blank();

    writer.begin_block(
        "static inline EMELA_UNUSED int64_t emela_i64_shift_right(int64_t value, int32_t by)",
    );
    writer.begin_block("if (by >= INT32_C(64))");
    writer.line("return value < INT64_C(0) ? -INT64_C(1) : INT64_C(0);");
    writer.end_block();
    writer.line("uint64_t shifted = (uint64_t)value >> (uint32_t)by;");
    writer.begin_block("if (by > INT32_C(0) && value < INT64_C(0))");
    writer.line("shifted |= UINT64_MAX << (UINT32_C(64) - (uint32_t)by);");
    writer.end_block();
    writer.line("return emela_i64_from_bits(shifted);");
    writer.end_block();
    writer.blank();

    writer.begin_block(
        "static inline EMELA_UNUSED int64_t emela_i64_shift_right_unsigned(int64_t value, int32_t by)",
    );
    writer.begin_block("if (by >= INT32_C(64))");
    writer.line("return INT64_C(0);");
    writer.end_block();
    writer.line("return emela_i64_from_bits((uint64_t)value >> (uint32_t)by);");
    writer.end_block();
    writer.blank();
}

fn emit_negative_shift_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED void emela_emb_negative_shift_defect(int32_t amount)",
    );
    writer.line("char message[48];");
    writer.line("static const char prefix[] = \"negative shift amount: \";");
    writer.line("uint16_t prefix_length = (uint16_t)(sizeof prefix - 1);");
    writer.line("uint32_t magnitude = (uint32_t)(-(amount + INT32_C(1))) + UINT32_C(1);");
    writer.line("char *digits = message + sizeof message;");
    writer.begin_block("do");
    writer.line("*--digits = (char)('0' + magnitude % UINT32_C(10));");
    writer.line("magnitude /= UINT32_C(10);");
    writer.end_block();
    writer.line("while (magnitude != UINT32_C(0));");
    writer.line("*--digits = '-';");
    writer.line("uint16_t number_length = (uint16_t)((message + sizeof message) - digits);");
    writer.begin_block("for (uint16_t i = 0; i < prefix_length; ++i)");
    writer.line("message[i] = prefix[i];");
    writer.end_block();
    writer.begin_block("for (uint16_t i = 0; i < number_length; ++i)");
    writer.line("message[prefix_length + i] = digits[i];");
    writer.end_block();
    writer.line(
        "emela_board_defect(EMELA_DEFECT_NEGATIVE_SHIFT, message, (uint16_t)(prefix_length + number_length));",
    );
    writer.end_block();
    writer.blank();
}

fn emit_i32_division_helpers(writer: &mut Writer) {
    writer
        .begin_block("static inline EMELA_UNUSED int32_t emela_i32_div(int32_t lhs, int32_t rhs)");
    writer.begin_block("if (rhs == INT32_C(0))");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_DIVISION_BY_ZERO, \"division by zero\", UINT16_C(16));",
    );
    writer.end_block();
    writer.begin_block("if (lhs == INT32_MIN && rhs == -INT32_C(1))");
    writer.line("return INT32_MIN;");
    writer.end_block();
    writer.line("return lhs / rhs;");
    writer.end_block();
    writer.blank();

    writer
        .begin_block("static inline EMELA_UNUSED int32_t emela_i32_rem(int32_t lhs, int32_t rhs)");
    writer.begin_block("if (rhs == INT32_C(0))");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_DIVISION_BY_ZERO, \"division by zero\", UINT16_C(16));",
    );
    writer.end_block();
    writer.begin_block("if (lhs == INT32_MIN && rhs == -INT32_C(1))");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.line("return lhs % rhs;");
    writer.end_block();
    writer.blank();
}

fn emit_i64_wrap_helper(writer: &mut Writer) {
    writer.begin_block("static inline EMELA_UNUSED int64_t emela_i64_from_bits(uint64_t bits)");
    writer.begin_block("if (bits <= (uint64_t)INT64_MAX)");
    writer.line("return (int64_t)bits;");
    writer.end_block();
    writer.line("return -INT64_C(1) - (int64_t)(UINT64_MAX - bits);");
    writer.end_block();
    writer.blank();
}

fn emit_i64_division_helpers(writer: &mut Writer) {
    writer
        .begin_block("static inline EMELA_UNUSED int64_t emela_i64_div(int64_t lhs, int64_t rhs)");
    writer.begin_block("if (rhs == INT64_C(0))");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_DIVISION_BY_ZERO, \"division by zero\", UINT16_C(16));",
    );
    writer.end_block();
    writer.begin_block("if (lhs == INT64_MIN && rhs == -INT64_C(1))");
    writer.line("return INT64_MIN;");
    writer.end_block();
    writer.line("return lhs / rhs;");
    writer.end_block();
    writer.blank();

    writer
        .begin_block("static inline EMELA_UNUSED int64_t emela_i64_rem(int64_t lhs, int64_t rhs)");
    writer.begin_block("if (rhs == INT64_C(0))");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_DIVISION_BY_ZERO, \"division by zero\", UINT16_C(16));",
    );
    writer.end_block();
    writer.begin_block("if (lhs == INT64_MIN && rhs == -INT64_C(1))");
    writer.line("return INT64_C(0);");
    writer.end_block();
    writer.line("return lhs % rhs;");
    writer.end_block();
    writer.blank();
}

fn emit_float_to_int_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EMELA_UNUSED int32_t emela_emb_float_to_int(double value, uint8_t operation)",
    );
    writer.begin_block("if (!(value > -2147483649.0 && value < 2147483648.0))");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_FLOAT_TO_INT, \"cannot convert Float to Int\", UINT16_C(27));",
    );
    writer.end_block();
    writer.line("int64_t rounded = (int64_t)value;");
    writer.begin_block("if (operation == UINT8_C(0) && (double)rounded > value)");
    writer.line("--rounded;");
    writer.end_block();
    writer.begin_block("else if (operation == UINT8_C(1) && (double)rounded < value)");
    writer.line("++rounded;");
    writer.end_block();
    writer.begin_block("else if (operation == UINT8_C(2))");
    writer.begin_block("if (value >= 0.0 && value - (double)rounded >= 0.5)");
    writer.line("++rounded;");
    writer.end_block();
    writer.begin_block("else if (value < 0.0 && (double)rounded - value >= 0.5)");
    writer.line("--rounded;");
    writer.end_block();
    writer.end_block();
    writer.begin_block("if (rounded < INT32_MIN || rounded > INT32_MAX)");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_FLOAT_TO_INT, \"cannot convert Float to Int\", UINT16_C(27));",
    );
    writer.end_block();
    writer.line("return (int32_t)rounded;");
    writer.end_block();
    writer.blank();
}

fn has_prim(flow: &emela_lir::Module, emitted: &HashSet<FnId>, ty: PrimTy, ops: &[PrimOp]) -> bool {
    flow.functions.iter().any(|(id, function)| {
        emitted.contains(&id)
            && !function.constant
            && function.stmts.iter().any(|(_, statement)| {
                matches!(
                    statement,
                    Stmt::Let {
                        value: Value::Prim {
                            op, ty: found_ty, ..
                        },
                        ..
                    } if *found_ty == ty && ops.contains(op)
                )
            })
    })
}

fn has_builtin(flow: &emela_lir::Module, emitted: &HashSet<FnId>, ops: &[Builtin]) -> bool {
    flow.functions.iter().any(|(id, function)| {
        emitted.contains(&id)
            && !function.constant
            && function.stmts.iter().any(|(_, statement)| {
                matches!(
                    statement,
                    Stmt::Let {
                        value: Value::Builtin { op, .. },
                        ..
                    } if ops.contains(op)
                )
            })
    })
}

fn has_indirect_call(flow: &emela_lir::Module, emitted: &HashSet<FnId>) -> bool {
    flow.functions.iter().any(|(id, function)| {
        emitted.contains(&id)
            && !function.constant
            && function.stmts.iter().any(|(_, statement)| {
                matches!(
                    statement,
                    Stmt::Let {
                        value: Value::Call {
                            callee: Callee::Indirect(_),
                            suspends: false,
                            on_fail: None,
                            ..
                        },
                        ..
                    }
                )
            })
    })
}

fn has_grapheme_runtime(flow: &emela_lir::Module, emitted: &HashSet<FnId>) -> bool {
    flow.functions
        .iter()
        .filter(|(id, function)| emitted.contains(id) && !function.constant)
        .any(|(_, function)| {
            function.stmts.iter().any(|(_, statement)| {
                let Stmt::Let {
                    value: Value::Builtin { op, args },
                    ..
                } = statement
                else {
                    return false;
                };
                match op {
                    Builtin::StringLength | Builtin::StringChars => true,
                    Builtin::StringSplit => !matches!(
                        args.get(1),
                        Some(Operand::Const(Const::String(separator))) if !separator.is_empty()
                    ),
                    _ => false,
                }
            })
        })
}

fn has_runtime_string_append(flow: &emela_lir::Module, emitted: &HashSet<FnId>) -> bool {
    flow.functions
        .iter()
        .filter(|(id, function)| emitted.contains(id) && !function.constant)
        .any(|(_, function)| {
            function.stmts.iter().any(|(_, statement)| {
                matches!(
                    statement,
                    Stmt::Let {
                        value: Value::Builtin {
                            op: Builtin::StringAppend,
                            args,
                        },
                        ..
                    } if args.len() == 2 && !append_is_static(args) && !append_has_empty_constant(args)
                )
            })
        })
}

fn has_runtime_int_string(flow: &emela_lir::Module, emitted: &HashSet<FnId>) -> bool {
    flow.functions
        .iter()
        .filter(|(id, function)| emitted.contains(id) && !function.constant)
        .any(|(_, function)| {
            function.stmts.iter().any(|(_, statement)| {
                matches!(
                    statement,
                    Stmt::Let {
                        value: Value::Builtin {
                            op: Builtin::IntToString | Builtin::Int64ToString,
                            args,
                        },
                        ..
                    } if args.len() == 1 && matches!(args[0], Operand::Var(_))
                )
            })
        })
}

fn has_runtime_float_to_string(flow: &emela_lir::Module, emitted: &HashSet<FnId>) -> bool {
    flow.functions
        .iter()
        .filter(|(id, function)| emitted.contains(id) && !function.constant)
        .any(|(_, function)| {
            function.stmts.iter().any(|(_, statement)| {
                matches!(
                    statement,
                    Stmt::Let {
                        value: Value::Builtin {
                            op: Builtin::FloatToString,
                            args,
                        },
                        ..
                    } if args.len() == 1 && matches!(args[0], Operand::Var(_))
                )
            })
        })
}

fn functions_with_string_output(
    flow: &emela_lir::Module,
    emitted: &HashSet<FnId>,
    layouts: &TypeLayouts<'_>,
) -> HashSet<FnId> {
    let mut result = HashSet::new();
    loop {
        let mut changed = false;
        for (id, function) in flow.functions.iter() {
            if !emitted.contains(&id)
                || function.constant
                || function.suspends
                || function.external.is_some()
                || layouts.owned_string_returns.contains(&function.name)
                || !matches!(layouts.types[function.ret], Type::String)
                || result.contains(&id)
            {
                continue;
            }
            let has_output_producer = function.stmts.iter().any(|(_, statement)| {
                let Stmt::Let { value, .. } = statement else {
                    return false;
                };
                match value {
                    Value::Builtin {
                        op: Builtin::StringAppend,
                        args,
                    } => {
                        args.len() == 2
                            && !append_is_static(args)
                            && !append_has_empty_constant(args)
                    }
                    Value::Builtin {
                        op:
                            Builtin::IntToString
                            | Builtin::Int64ToString
                            | Builtin::FloatToString
                            | Builtin::StringFromCodePoint,
                        args,
                    } => args.len() == 1 && matches!(args[0], Operand::Var(_)),
                    Value::Builtin {
                        op: Builtin::StringJoin,
                        args,
                    } => args.len() == 2,
                    Value::Runtime {
                        op: RuntimeOp::ShowString,
                        args,
                    } => args.len() == 1 && !matches!(args[0], Operand::Const(Const::String(_))),
                    Value::Fiber {
                        op: emela_core::FiberOp::CauseToString,
                        ..
                    } => true,
                    Value::Call {
                        callee: Callee::Direct(callee),
                        ..
                    } => result.contains(callee),
                    _ => false,
                }
            });
            if has_output_producer {
                result.insert(id);
                changed = true;
            }
        }
        if !changed {
            return result;
        }
    }
}

fn has_runtime_int_parse(flow: &emela_lir::Module, emitted: &HashSet<FnId>) -> bool {
    flow.functions
        .iter()
        .filter(|(id, function)| emitted.contains(id) && !function.constant)
        .any(|(_, function)| {
            function.stmts.iter().any(|(_, statement)| {
                matches!(
                    statement,
                    Stmt::Let {
                        value: Value::Builtin {
                            op: Builtin::IntParse,
                            args,
                        },
                        ..
                    } if args.len() == 1 && matches!(args[0], Operand::Var(_))
                )
            })
        })
}

fn has_runtime_float_parse(flow: &emela_lir::Module, emitted: &HashSet<FnId>) -> bool {
    flow.functions
        .iter()
        .filter(|(id, function)| emitted.contains(id) && !function.constant)
        .any(|(_, function)| {
            function.stmts.iter().any(|(_, statement)| {
                matches!(
                    statement,
                    Stmt::Let {
                        value: Value::Builtin {
                            op: Builtin::FloatParse,
                            args,
                        },
                        ..
                    } if args.len() == 1 && matches!(args[0], Operand::Var(_))
                )
            })
        })
}

fn has_runtime_string_from_code_point(flow: &emela_lir::Module, emitted: &HashSet<FnId>) -> bool {
    flow.functions
        .iter()
        .filter(|(id, function)| emitted.contains(id) && !function.constant)
        .any(|(_, function)| {
            function.stmts.iter().any(|(_, statement)| {
                matches!(
                    statement,
                    Stmt::Let {
                        value: Value::Builtin {
                            op: Builtin::StringFromCodePoint,
                            args,
                        },
                        ..
                    } if args.len() == 1 && matches!(args[0], Operand::Var(_))
                )
            })
        })
}

fn has_runtime_string_show(flow: &emela_lir::Module, emitted: &HashSet<FnId>) -> bool {
    flow.functions
        .iter()
        .filter(|(id, function)| emitted.contains(id) && !function.constant)
        .any(|(_, function)| {
            function.stmts.iter().any(|(_, statement)| {
                matches!(
                    statement,
                    Stmt::Let {
                        value: Value::Runtime {
                            op: RuntimeOp::ShowString,
                            args,
                        },
                        ..
                    } if !matches!(args.as_slice(), [Operand::Const(Const::String(_))])
                )
            })
        })
}

fn has_runtime_op(flow: &emela_lir::Module, emitted: &HashSet<FnId>, op: RuntimeOp) -> bool {
    flow.functions
        .iter()
        .filter(|(id, function)| emitted.contains(id) && !function.constant)
        .any(|(_, function)| {
            function.stmts.iter().any(|(_, statement)| {
                matches!(
                    statement,
                    Stmt::Let {
                        value: Value::Runtime { op: found, .. },
                        ..
                    } if *found == op
                )
            })
        })
}

fn comparison_expression(lhs: &str, op: &str, rhs: &str) -> Result<String, &'static str> {
    Ok(format!("(({lhs}) {op} ({rhs}) ? INT32_C(1) : INT32_C(0))"))
}

fn emit_return(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    term: &Term,
    string_buffers: &StringBufferPlan,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    match term {
        Term::Return(None) => {
            emit_owned_cleanup(layouts, function, None, writer);
            writer.ret(None);
        }
        Term::Return(Some(operand)) => {
            let mut expression = operand_expression(operand)
                .map_err(|variant| unsupported(&function.name, variant))?;
            // Flow uses an empty reference as the unobserved boxed Unit result of fork.
            if function.ret == TypeId::ANY
                && matches!(operand, Operand::Const(Const::String(s)) if s.is_empty())
            {
                expression = "(EmelaAny){0}".into();
            }
            if layouts.owned_string_returns.contains(&function.name) {
                writer.line(&format!(
                    "EmelaString emela_owned_return = emela_heap_own_string({expression});"
                ));
                emit_owned_cleanup(layouts, function, None, writer);
                writer.ret(Some("emela_owned_return"));
                return Ok(());
            }
            if layouts.owned_aggregate_returns.contains(&function.name) {
                writer.decl(
                    &layouts.c_type(function.ret),
                    "emela_owned_return",
                    Some(&expression),
                );
                layouts.emit_own_value_in_place(function.ret, "emela_owned_return", writer);
                emit_owned_cleanup(layouts, function, None, writer);
                writer.ret(Some("emela_owned_return"));
                return Ok(());
            }
            let mut moved = None;
            let copies_string_to_caller = string_buffers.returns_to_caller
                && matches!(layouts.types[function.ret], Type::String);
            if copies_string_to_caller {
                writer.decl(
                    "EmelaString",
                    "emela_string_return_source",
                    Some(&expression),
                );
                writer.decl(
                    "EmelaString",
                    "emela_string_return_value",
                    Some(&format!(
                        "emela_emb_string_copy_to_output(emela_string_return_source, emela_string_output, emela_emb_string_output_capacity, UINT16_C({}))",
                        string_buffers.capacity
                    )),
                );
                expression = "emela_string_return_value".to_owned();
            }
            if !copies_string_to_caller
                && layouts.is_managed_value(function.ret)
                && let Operand::Var(variable) = operand
            {
                if function.params.contains(variable) {
                    emit_retain_operand(layouts, function, operand, writer);
                } else {
                    writer.assign(&owned_variable_name(*variable), "INT32_C(0)");
                    moved = Some(*variable);
                }
            }
            emit_owned_cleanup(layouts, function, moved, writer);
            writer.ret(Some(&expression));
        }
        Term::Fail(error) => {
            if !function.fails {
                return Err(invalid_flow_ir(
                    &function.name,
                    "Fail appears in a function without a failure channel",
                ));
            }
            emit_error_assignment(layouts, function, error, "*emela_error_out", writer)?;
            emit_failure_return(layouts, function, writer);
        }
        Term::Defect(message, _) => emit_defect(layouts, function, message, writer)?,
        Term::Unreachable => writer
            .line("emela_board_defect(EMELA_DEFECT_ASSERT, \"unreachable code\", UINT16_C(16));"),
        other => return Err(unsupported(&function.name, term_name(other))),
    }

    let return_repr = layouts.type_repr(function.ret);
    if return_repr == Repr::Void && matches!(term, Term::Return(Some(_))) {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: void return has a value",
                function.name
            ),
        }]);
    }
    Ok(())
}

fn operand_expression(operand: &Operand) -> Result<String, &'static str> {
    match operand {
        Operand::Var(variable) => Ok(variable_name(*variable)),
        Operand::Const(value) => constant_expression(value)?.ok_or("Unit return operand"),
    }
}

fn constant_expression(value: &emela_lir::Const) -> Result<Option<String>, &'static str> {
    match value {
        emela_lir::Const::Int(value) => Ok(Some(int32_literal(*value))),
        emela_lir::Const::Bool(value) => Ok(Some(if *value {
            "INT32_C(1)".to_owned()
        } else {
            "INT32_C(0)".to_owned()
        })),
        emela_lir::Const::Unit => Ok(None),
        emela_lir::Const::Int64(value) => Ok(Some(int64_literal(*value))),
        emela_lir::Const::Float(value) => Ok(Some(float_literal(*value))),
        emela_lir::Const::String(value) => {
            let length = u16::try_from(value.len()).map_err(|_| "String constant is too long")?;
            Ok(Some(format!(
                "(EmelaString){{ .data = {}, .length = UINT16_C({length}) }}",
                c_string_literal(value.as_bytes())
            )))
        }
    }
}

fn c_string_literal(bytes: &[u8]) -> String {
    let escaped = bytes
        .iter()
        .map(|byte| format!("\\x{byte:02x}"))
        .collect::<String>();
    format!("\"{escaped}\"")
}

fn int32_literal(value: i32) -> String {
    if value == i32::MIN {
        "(-INT32_C(2147483647) - INT32_C(1))".to_owned()
    } else if value < 0 {
        format!("(-INT32_C({}))", -value)
    } else {
        format!("INT32_C({value})")
    }
}

fn int64_literal(value: i64) -> String {
    if value == i64::MIN {
        "(-INT64_C(9223372036854775807) - INT64_C(1))".to_owned()
    } else {
        format!("INT64_C({value})")
    }
}

fn float_literal(value: f64) -> String {
    if value.is_nan() {
        "NAN".to_owned()
    } else if value == f64::INFINITY {
        "INFINITY".to_owned()
    } else if value == f64::NEG_INFINITY {
        "(-INFINITY)".to_owned()
    } else {
        let literal = format!("{value:?}");
        if literal.contains(['.', 'e', 'E']) {
            literal
        } else {
            format!("{literal}.0")
        }
    }
}

fn signature(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    name: &str,
    is_static: bool,
    takes_error_out: bool,
    takes_string_output: bool,
) -> String {
    let return_type = layouts.c_type(function.ret);
    let mut params: Vec<String> = function
        .params
        .iter()
        .map(|&variable| {
            let data = &function.vars[variable];
            format!("{} {}", layouts.c_type(data.ty), variable_name(variable))
        })
        .collect::<Vec<_>>();
    if takes_error_out {
        params.push("EmelaError *emela_error_out".to_owned());
    }
    if takes_string_output {
        params.push("char *emela_string_output".to_owned());
        params.push("uint16_t emela_string_capacity".to_owned());
    }
    let params = if params.is_empty() {
        "void".to_owned()
    } else {
        params.join(", ")
    };
    let linkage = if is_static { "static " } else { "" };
    format!("{linkage}{return_type} {name}({params})")
}

fn closure_signature(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    name: &str,
    is_static: bool,
) -> String {
    let signature = signature(layouts, function, name, is_static, function.fails, false);
    let open = signature
        .find('(')
        .expect("function signature contains an opening parenthesis");
    let params = &signature[open + 1..signature.len() - 1];
    if params == "void" {
        format!("{}(void *emela_env)", &signature[..open])
    } else {
        format!("{}(void *emela_env, {params})", &signature[..open])
    }
}

fn function_ref_adapters(
    flow: &emela_lir::Module,
    emitted: &HashSet<FnId>,
    layouts: &TypeLayouts<'_>,
) -> Vec<(FnId, TypeId)> {
    let mut adapters = HashSet::new();
    for (function_id, function) in flow.functions.iter() {
        if !emitted.contains(&function_id) {
            continue;
        }
        for (_, statement) in function.stmts.iter() {
            let Stmt::Let {
                dst: Some(dst),
                value: Value::FnRef(target),
                ..
            } = statement
            else {
                continue;
            };
            if !layouts.closure_functions.contains_key(target) {
                adapters.insert((*target, function.vars[*dst].ty));
            }
        }
    }
    let mut adapters: Vec<_> = adapters.into_iter().collect();
    adapters.sort_by_key(|(target, function_type)| {
        (u32::from(target.into_raw()), function_type.index())
    });
    adapters
}

fn function_ref_adapter_name(target: FnId, function_type: TypeId) -> String {
    format!(
        "emela_fn_ref_adapter_{}_{}",
        u32::from(target.into_raw()),
        function_type.index()
    )
}

fn function_coerce_adapter_name(index: usize) -> String {
    format!("emela_fn_coerce_adapter_{index}")
}

fn function_ref_adapter_signature(
    layouts: &TypeLayouts<'_>,
    function_type: TypeId,
    name: &str,
) -> String {
    let Type::Fn(function_type) = layouts.types[function_type].clone() else {
        unreachable!("function reference adapter has a non-function type")
    };
    let mut params = vec!["void *emela_env".to_owned()];
    if function_type.suspends {
        params.insert(0, "emela_fiber *fiber".to_owned());
    }
    for (index, &parameter) in function_type.params.iter().enumerate() {
        if layouts.type_repr(parameter) != Repr::Void {
            params.push(format!("{} emela_arg_{index}", layouts.c_type(parameter)));
        }
    }
    if function_type.ev {
        params.push("void *emela_evidence".to_owned());
    }
    if function_type.fails {
        params.push("EmelaError *emela_error_out".to_owned());
    }
    if function_type.suspends && layouts.type_repr(function_type.ret) != Repr::Void {
        params.push(format!("{} *result", layouts.c_type(function_type.ret)));
    }
    format!(
        "static {} {name}({})",
        if function_type.suspends {
            "void".to_owned()
        } else {
            layouts.c_type(function_type.ret)
        },
        params.join(", ")
    )
}

fn emit_function_ref_adapter(
    layouts: &TypeLayouts<'_>,
    function_type: TypeId,
    target_name: &str,
    adapter_name: &str,
    takes_string_output: bool,
    string_capacity: u16,
    writer: &mut Writer,
) {
    let Type::Fn(function_type_data) = layouts.types[function_type].clone() else {
        unreachable!("function reference adapter has a non-function type")
    };
    writer.begin_block(&function_ref_adapter_signature(
        layouts,
        function_type,
        adapter_name,
    ));
    writer.line("(void)emela_env;");
    let mut args = Vec::new();
    if function_type_data.suspends {
        args.push("fiber".to_owned());
    }
    for (index, &parameter) in function_type_data.params.iter().enumerate() {
        if layouts.type_repr(parameter) != Repr::Void {
            args.push(format!("emela_arg_{index}"));
        }
    }
    if function_type_data.ev {
        args.push("emela_evidence".to_owned());
    }
    if function_type_data.fails {
        args.push("emela_error_out".to_owned());
    }
    if function_type_data.suspends {
        if layouts.type_repr(function_type_data.ret) != Repr::Void {
            args.push("result".to_owned());
        }
        writer.line(&format!("{target_name}_push({});", args.join(", ")));
        writer.end_block();
        return;
    }
    if takes_string_output {
        writer.line(&format!("char buffer[{string_capacity}];"));
        args.push("buffer".to_owned());
        args.push(format!("UINT16_C({string_capacity})"));
    }
    let call = format!("{target_name}({})", args.join(", "));
    if takes_string_output {
        writer.line(&format!("EmelaString returned = {call};"));
        writer.ret(Some("emela_heap_own_string(returned)"));
        writer.end_block();
        return;
    }
    if layouts.type_repr(function_type_data.ret) == Repr::Void {
        writer.line(&format!("{call};"));
        writer.ret(None);
    } else {
        writer.ret(Some(&call));
    }
    writer.end_block();
}

fn emit_function_coerce_adapter(
    layouts: &TypeLayouts<'_>,
    adapter: &FunctionAdapterLayout,
    index: usize,
    writer: &mut Writer,
) {
    let Type::Fn(source_type) = layouts.types[adapter.source].clone() else {
        unreachable!("function adapter source has non-function type")
    };
    let Type::Fn(destination_type) = layouts.types[adapter.destination].clone() else {
        unreachable!("function adapter destination has non-function type")
    };
    let traps_failure = source_type.fails && !destination_type.fails;
    if traps_failure && source_type.suspends {
        writer.line(&format!(
            "typedef struct {{ emela_frame base; EmelaError error; }} emela_coerce_check_{index};"
        ));
        writer.line(&format!("_Static_assert(sizeof(emela_coerce_check_{index}) <= EMELA_FRAME_BYTES, \"increase frame_bytes for function coercion\");"));
        writer.begin_block(&format!(
            "static void emela_coerce_check_step_{index}(emela_fiber *fiber, emela_frame *base)"
        ));
        writer.line(&format!(
            "emela_coerce_check_{index} *frame = (emela_coerce_check_{index} *)base;"
        ));
        writer.line("if (frame->error.tag != 0) emela_board_defect(EMELA_DEFECT_ASSERT, \"failure in infallible coercion\", UINT16_C(30));");
        writer.line("emela_frame_return(fiber);");
        writer.end_block();
        writer.begin_block(&format!(
            "static void emela_coerce_check_drop_{index}(emela_frame *base)"
        ));
        writer.line(&format!(
            "emela_error_clear(&((emela_coerce_check_{index} *)base)->error);"
        ));
        writer.end_block();
    }
    writer.begin_block(&function_ref_adapter_signature(
        layouts,
        adapter.destination,
        &function_coerce_adapter_name(index),
    ));
    writer.line(&format!(
        "emela_fn_coerce_env_{index} *environment = (emela_fn_coerce_env_{index} *)emela_env;"
    ));
    writer.begin_block("if (environment == NULL || environment->source.call == NULL)");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_PANIC, \"invalid coerced function value\", UINT16_C(28));",
    );
    writer.end_block();

    let mut args = vec!["environment->source.env".to_owned()];
    if source_type.suspends {
        args.insert(0, "fiber".to_owned());
    }
    for (parameter_index, &parameter) in source_type.params.iter().enumerate() {
        if layouts.type_repr(parameter) != Repr::Void {
            args.push(format!("emela_arg_{parameter_index}"));
        }
    }
    if source_type.ev {
        args.push("emela_evidence".to_owned());
    } else if destination_type.ev {
        writer.line("(void)emela_evidence;");
    }
    if traps_failure {
        if source_type.suspends {
            writer.line(&format!("emela_coerce_check_{index} *check = (emela_coerce_check_{index} *)emela_frame_push(fiber, sizeof(*check), emela_coerce_check_step_{index});"));
            writer.line("check->error = (EmelaError){0};");
            writer.line(&format!(
                "check->base.drop = emela_coerce_check_drop_{index};"
            ));
            args.push("&check->error".into());
        } else {
            writer.line("EmelaError checked_error = {0};");
            args.push("&checked_error".into());
        }
    } else if source_type.fails {
        args.push("emela_error_out".to_owned());
    } else if destination_type.fails {
        writer.assign("*emela_error_out", "(EmelaError){0}");
    }
    if source_type.suspends && layouts.type_repr(source_type.ret) != Repr::Void {
        args.push("result".to_owned());
    }
    let call = format!("environment->source.call({})", args.join(", "));
    let trap = "if (checked_error.tag != 0) emela_board_defect(EMELA_DEFECT_ASSERT, \"failure in infallible coercion\", UINT16_C(30));";
    if destination_type.suspends {
        if source_type.suspends || layouts.type_repr(source_type.ret) == Repr::Void {
            writer.line(&format!("{call};"));
            if traps_failure && !source_type.suspends {
                writer.line(trap);
            }
        } else {
            writer.decl(&layouts.c_type(source_type.ret), "returned", Some(&call));
            if traps_failure {
                writer.line(trap);
            }
            writer.begin_block("if (result != NULL)");
            writer.assign("*result", "returned");
            writer.end_block();
            if layouts.is_managed_value(source_type.ret) {
                writer.begin_block("else");
                layouts.emit_release_value_expression(source_type.ret, "returned", writer);
                writer.end_block();
            }
        }
        writer.line("(void)fiber;");
        writer.end_block();
        return;
    }
    if layouts.type_repr(source_type.ret) == Repr::Void {
        writer.line(&format!("{call};"));
        if traps_failure {
            writer.line(trap);
        }
        writer.ret(None);
    } else if traps_failure {
        writer.decl(&layouts.c_type(source_type.ret), "returned", Some(&call));
        writer.line(trap);
        writer.ret(Some("returned"));
    } else {
        writer.ret(Some(&call));
    }
    writer.end_block();
}

fn emit_fallible_wrapper(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    public_name: &str,
    implementation_name: &str,
    takes_string_output: bool,
    writer: &mut Writer,
) {
    writer.begin_block(&signature(
        layouts,
        function,
        public_name,
        false,
        false,
        takes_string_output,
    ));
    writer.decl("EmelaError", "emela_error", Some("{0}"));
    let mut args: Vec<String> = function
        .params
        .iter()
        .map(|&variable| variable_name(variable))
        .collect();
    args.push("&emela_error".to_owned());
    if takes_string_output {
        args.push("emela_string_output".to_owned());
        args.push("emela_string_capacity".to_owned());
    }
    let call = format!("{implementation_name}({})", args.join(", "));
    if layouts.type_repr(function.ret) == Repr::Void {
        writer.line(&format!("{call};"));
    } else {
        writer.decl(&layouts.c_type(function.ret), "emela_result", Some(&call));
    }
    writer.begin_block("if (emela_error.tag != UINT32_C(0))");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_UNHANDLED_ERROR, \"unhandled error\", UINT16_C(15));",
    );
    writer.end_block();
    if layouts.type_repr(function.ret) == Repr::Void {
        writer.ret(None);
    } else {
        writer.ret(Some("emela_result"));
    }
    writer.end_block();
}

#[derive(Debug)]
struct AggregateLayout {
    ty: TypeId,
    kind: AggregateKind,
}

#[derive(Debug)]
enum AggregateKind {
    Tuple(Vec<TypeId>),
    Enum(Vec<Vec<TypeId>>),
}

struct TypeLayouts<'a> {
    discarded_results: crate::discarded::Results,
    core: &'a emela_core::Module,
    types: TypeTable,
    aggregates: Vec<AggregateLayout>,
    aggregate_ids: HashMap<TypeId, usize>,
    recursive_edges: HashSet<(TypeId, usize, usize)>,
    expanding_enums: HashSet<EnumId>,
    recursive_box_types: Vec<TypeId>,
    recursive_box_ids: HashMap<TypeId, usize>,
    list_types: Vec<TypeId>,
    list_ids: HashMap<TypeId, usize>,
    function_types: Vec<TypeId>,
    function_results: HashMap<FnId, TypeId>,
    closure_function_types: HashSet<TypeId>,
    closure_functions: HashMap<FnId, usize>,
    closure_environments: Vec<crate::static_closure::ClosureEnvironment>,
    owned_string_returns: HashSet<String>,
    runtime_consts: HashSet<FnId>,
    owned_aggregate_returns: HashSet<String>,
    cell_types: Vec<TypeId>,
    cell_ids: HashMap<TypeId, usize>,
    function_adapters: Vec<FunctionAdapterLayout>,
    function_adapter_ids: HashMap<(TypeId, TypeId), usize>,
    handler_layouts: Vec<HandlerLayout>,
    handler_layout_ids: HashMap<EffectId, usize>,
    resume_types: Vec<TypeId>,
    has_resumes: bool,
    has_unwind: bool,
    has_scopes: bool,
    deferred_pool: bool,
    deferred_complete_types: Vec<TypeId>,
    deferred_read_types: Vec<TypeId>,
    deferred_poll_types: Vec<TypeId>,
    deferred_await_types: Vec<TypeId>,
    child_join_types: Vec<TypeId>,
    child_await_types: Vec<TypeId>,
    child_interrupts: bool,
    has_causes: bool,
    has_cause_raise: bool,
    attempt_types: Vec<(TypeId, TypeId)>,
    fork_types: Vec<TypeId>,
    evidence_nodes_required: bool,
    any_abi: bool,
    any_box_required: bool,
    any_box_types: Vec<TypeId>,
    any_box_ids: HashMap<TypeId, usize>,
    error_types: Vec<(EnumId, TypeId)>,
    error_shows: Vec<(EnumId, FnId)>,
    has_error_abi: bool,
    managed_error_payloads: bool,
}

struct HandlerLayout {
    effect: EffectId,
    operations: Vec<TypeId>,
}

struct FunctionAdapterLayout {
    source: TypeId,
    destination: TypeId,
}

impl<'a> TypeLayouts<'a> {
    fn new(
        flow: &'a emela_lir::Module,
        emitted: &HashSet<FnId>,
        closures: &crate::static_closure::ClosureLowering,
        deferred_pool: bool,
    ) -> Result<Self, Vec<Error>> {
        let mut layouts = TypeLayouts {
            discarded_results: HashMap::new(),
            core: &flow.core,
            types: flow.core.types.clone(),
            aggregates: Vec::new(),
            aggregate_ids: HashMap::new(),
            recursive_edges: HashSet::new(),
            expanding_enums: crate::type_expansion::expanding_enums(&flow.core),
            recursive_box_types: Vec::new(),
            recursive_box_ids: HashMap::new(),
            list_types: Vec::new(),
            list_ids: HashMap::new(),
            function_types: Vec::new(),
            function_results: flow.functions.iter().map(|(id, f)| (id, f.ret)).collect(),
            closure_function_types: closures
                .environments
                .iter()
                .map(|environment| environment.function_type)
                .collect(),
            closure_functions: closures.functions.clone(),
            closure_environments: closures.environments.clone(),
            owned_aggregate_returns: HashSet::new(),
            runtime_consts: HashSet::new(),
            owned_string_returns: closures
                .functions
                .keys()
                .filter_map(|id| {
                    let f = &flow.functions[*id];
                    (!f.suspends && matches!(flow.core.types[f.ret], Type::String))
                        .then(|| f.name.clone())
                })
                .collect(),
            cell_types: Vec::new(),
            cell_ids: HashMap::new(),
            function_adapters: Vec::new(),
            function_adapter_ids: HashMap::new(),
            handler_layouts: Vec::new(),
            handler_layout_ids: HashMap::new(),
            resume_types: Vec::new(),
            has_resumes: suspend::has_operations(flow, emitted)
                || crate::fiber::has_scopes(flow, emitted),
            has_unwind: suspend::has_operations(flow, emitted)
                || crate::fiber::has_scopes(flow, emitted)
                || suspend::has_protects(flow, emitted)
                || (flow
                    .functions
                    .iter()
                    .any(|(id, f)| emitted.contains(&id) && f.suspends)
                    && flow.functions.iter().any(|(id, f)| {
                        emitted.contains(&id)
                            && f.stmts.iter().any(|(_, s)| {
                                matches!(
                                    s,
                                    Stmt::Let {
                                        value: Value::Fiber {
                                            op: emela_core::FiberOp::CauseRaise { .. },
                                            ..
                                        },
                                        ..
                                    }
                                )
                            })
                    })),
            has_scopes: crate::fiber::has_scopes(flow, emitted),
            deferred_pool,
            deferred_complete_types: Vec::new(),
            deferred_read_types: Vec::new(),
            deferred_poll_types: Vec::new(),
            deferred_await_types: Vec::new(),
            child_join_types: Vec::new(),
            child_await_types: Vec::new(),
            child_interrupts: false,
            has_causes: flow.functions.iter().any(|(id, f)| {
                emitted.contains(&id)
                    && f.stmts.iter().any(|(_, s)| {
                        matches!(
                            s,
                            Stmt::Let {
                                value: Value::Fiber {
                                    op: emela_core::FiberOp::Attempt { .. }
                                        | emela_core::FiberOp::Await { .. }
                                        | emela_core::FiberOp::CauseKind { .. }
                                        | emela_core::FiberOp::CauseToString
                                        | emela_core::FiberOp::CauseRaise { .. },
                                    ..
                                },
                                ..
                            }
                        )
                    })
            }),
            has_cause_raise: flow.functions.iter().any(|(id, f)| {
                emitted.contains(&id)
                    && f.stmts.iter().any(|(_, s)| {
                        matches!(
                            s,
                            Stmt::Let {
                                value: Value::Fiber {
                                    op: emela_core::FiberOp::CauseRaise { .. },
                                    ..
                                },
                                ..
                            }
                        )
                    })
            }),
            attempt_types: Vec::new(),
            fork_types: Vec::new(),
            evidence_nodes_required: false,
            any_abi: false,
            any_box_required: false,
            any_box_types: Vec::new(),
            any_box_ids: HashMap::new(),
            error_types: Vec::new(),
            error_shows: flow.error_shows.clone(),
            managed_error_payloads: false,
            has_error_abi: flow.functions.iter().any(|(id, function)| {
                emitted.contains(&id)
                    && (function.fails
                        || function
                            .seqs
                            .iter()
                            .any(|(_, sequence)| matches!(sequence.term, Term::Fail(_)))
                        || function.stmts.iter().any(|(_, statement)| match statement {
                            Stmt::Let { value, .. } => match value {
                                Value::Call { on_fail, .. } | Value::Fiber { on_fail, .. } => {
                                    on_fail.is_some()
                                }
                                Value::IsError { .. } | Value::AsError { .. } => true,
                                _ => false,
                            },
                            _ => false,
                        }))
            }),
        };
        let mut validated = HashSet::new();
        for (id, function) in flow.functions.iter() {
            if !emitted.contains(&id) {
                continue;
            }
            for ty in std::iter::once(function.ret).chain(function.vars.iter().map(|(_, v)| v.ty)) {
                layouts.validate_finite_types(
                    ty,
                    &function.name,
                    &mut HashSet::new(),
                    &mut validated,
                )?;
            }
        }
        let mut states = HashMap::new();
        for (id, function) in flow.functions.iter() {
            if !emitted.contains(&id) {
                continue;
            }
            layouts.collect(function.ret, &function.name, &mut states)?;
            for (_, variable) in function.vars.iter() {
                layouts.collect(variable.ty, &function.name, &mut states)?;
            }
            for (_, statement) in function.stmts.iter() {
                let Stmt::Let { dst, value, .. } = statement else {
                    continue;
                };
                match value {
                    Value::Builtin {
                        op: Builtin::CellNew,
                        args,
                    } => {
                        if let Some(content) =
                            args.first().and_then(|arg| operand_type(function, arg))
                        {
                            let cell = layouts.types.intern(Type::Cell(content));
                            layouts.collect(cell, &function.name, &mut states)?;
                        }
                    }
                    Value::Fiber {
                        op: emela_core::FiberOp::Attempt { exit, .. },
                        args,
                        ..
                    } => {
                        let [Operand::Var(body)] = args.as_slice() else {
                            return Err(invalid_flow_ir(&function.name, "Attempt body"));
                        };
                        let body_ty = function.vars[*body].ty;
                        let Type::Fn(ft) = layouts.types[body_ty].clone() else {
                            return Err(invalid_flow_ir(&function.name, "Attempt function type"));
                        };
                        if ft.ev || !ft.params.is_empty() {
                            return Err(unsupported(&function.name, "Attempt body ABI"));
                        }
                        let mut params = vec![TypeId::UNIT; layouts.core.enums[*exit].params];
                        if params.is_empty() {
                            return Err(invalid_flow_ir(
                                &function.name,
                                "Attempt Exit type parameters",
                            ));
                        }
                        params[0] = ft.ret;
                        let result = dst
                            .map(|v| function.vars[v].ty)
                            .unwrap_or_else(|| layouts.types.enum_type(*exit, params));
                        layouts.collect(result, &function.name, &mut states)?;
                        if child::success_type(&layouts, result) != Some(ft.ret) {
                            return Err(invalid_flow_ir(&function.name, "Attempt result layout"));
                        }
                        if !layouts.attempt_types.contains(&(body_ty, result)) {
                            layouts.attempt_types.push((body_ty, result));
                        }
                    }
                    Value::Fiber {
                        op: emela_core::FiberOp::Interrupt,
                        args,
                        ..
                    } => {
                        let [handle] = args.as_slice() else {
                            return Err(invalid_flow_ir(&function.name, "Interrupt arity"));
                        };
                        child::value_type(&layouts, function, handle)?;
                        layouts.child_interrupts = true;
                    }
                    Value::Fiber {
                        op: emela_core::FiberOp::Await { exit },
                        args,
                        ..
                    } => {
                        let [handle] = args.as_slice() else {
                            return Err(invalid_flow_ir(&function.name, "Await arity"));
                        };
                        let ty = child::value_type(&layouts, function, handle)?;
                        let mut params = vec![TypeId::UNIT; layouts.core.enums[*exit].params];
                        let Some(first) = params.first_mut() else {
                            return Err(invalid_flow_ir(
                                &function.name,
                                "Await Exit type parameters",
                            ));
                        };
                        *first = ty;
                        let result = layouts.types.enum_type(*exit, params);
                        if dst.is_some_and(|dst| {
                            layouts.types.erase_sets(function.vars[dst].ty)
                                != layouts.types.erase_sets(result)
                        }) {
                            return Err(invalid_flow_ir(&function.name, "Await result type"));
                        }
                        layouts.collect(result, &function.name, &mut states)?;
                        if child::success_type(&layouts, result) != Some(ty) {
                            return Err(invalid_flow_ir(&function.name, "Await Success payload"));
                        }
                        layouts.add_any_box_type(ty);
                        if !layouts.child_await_types.contains(&result) {
                            layouts.child_await_types.push(result);
                        }
                    }
                    Value::Fiber {
                        op: emela_core::FiberOp::Join,
                        args,
                        ..
                    } => {
                        let [handle] = args.as_slice() else {
                            return Err(invalid_flow_ir(&function.name, "Join arity"));
                        };
                        let ty = child::value_type(&layouts, function, handle)?;
                        if dst.is_some_and(|dst| function.vars[dst].ty != ty) {
                            return Err(invalid_flow_ir(&function.name, "Join result type"));
                        }
                        layouts.add_any_box_type(ty);
                        if !layouts.child_join_types.contains(&ty) {
                            layouts.child_join_types.push(ty);
                        }
                    }
                    Value::Fiber {
                        op: emela_core::FiberOp::DeferredAwait,
                        args,
                        ..
                    } => {
                        let ty = deferred::value_type(&layouts, function, &args[0])?;
                        layouts.add_any_box_type(ty);
                        if !layouts.deferred_read_types.contains(&ty) {
                            layouts.deferred_read_types.push(ty);
                        }
                        if !layouts.deferred_await_types.contains(&ty) {
                            layouts.deferred_await_types.push(ty);
                        }
                    }
                    Value::Fiber {
                        op: emela_core::FiberOp::Poll,
                        args,
                        ..
                    } => {
                        let ty = deferred::value_type(&layouts, function, &args[0])?;
                        layouts.add_any_box_type(ty);
                        if !layouts.deferred_read_types.contains(&ty) {
                            layouts.deferred_read_types.push(ty);
                        }
                        if let Some(dst) = dst {
                            let option = function.vars[*dst].ty;
                            if option_payload_type(&layouts, option) != Some(ty) {
                                return Err(invalid_flow_ir(&function.name, "Poll result type"));
                            }
                            if !layouts.deferred_poll_types.contains(&option) {
                                layouts.deferred_poll_types.push(option);
                            }
                        }
                    }
                    Value::Fiber {
                        op: emela_core::FiberOp::Complete,
                        args,
                        ..
                    } => {
                        let ty = deferred::value_type(&layouts, function, &args[0])?;
                        layouts.add_any_box_type(ty);
                        if !layouts.deferred_complete_types.contains(&ty) {
                            layouts.deferred_complete_types.push(ty);
                        }
                    }
                    Value::Fiber {
                        op: emela_core::FiberOp::Fork,
                        args,
                        ..
                    } => {
                        if let Some(Operand::Var(body)) = args.get(1) {
                            let ty = function.vars[*body].ty;
                            if !layouts.fork_types.contains(&ty) {
                                layouts.fork_types.push(ty);
                            }
                        }
                        layouts.any_abi = true;
                        layouts.any_box_required = true;
                    }
                    Value::Fiber {
                        op: emela_core::FiberOp::Deferred,
                        ..
                    } => {
                        layouts.any_abi = true;
                        layouts.any_box_required = true;
                    }
                    Value::Handler { effect, ops, .. } => {
                        let operations = ops
                            .iter()
                            .map(|&operation| function.vars[operation].ty)
                            .collect::<Vec<_>>();
                        for (index, &ty) in operations.iter().enumerate() {
                            if !flow.core.effects[*effect].ops[index].suspend {
                                continue;
                            }
                            let Type::Fn(ft) = &layouts.types[ty] else {
                                unreachable!()
                            };
                            let resume = *ft
                                .params
                                .last()
                                .expect("suspend handler has a resume parameter");
                            if !layouts.resume_types.contains(&resume) {
                                layouts.resume_types.push(resume);
                            }
                            layouts.closure_function_types.insert(resume);
                        }
                        if let Some(previous) = layouts
                            .handler_layout_ids
                            .get(effect)
                            .and_then(|index| layouts.handler_layouts.get(*index))
                        {
                            if previous.operations != operations {
                                return Err(invalid_flow_ir(
                                    &function.name,
                                    "handlers for one effect have inconsistent operation function types",
                                ));
                            }
                        } else {
                            let index = layouts.handler_layouts.len();
                            layouts.handler_layouts.push(HandlerLayout {
                                effect: *effect,
                                operations,
                            });
                            layouts.handler_layout_ids.insert(*effect, index);
                        }
                    }
                    Value::EvExtend { bindings, .. } if !bindings.is_empty() => {
                        layouts.evidence_nodes_required = true;
                    }
                    Value::EvLookup { .. } => {
                        layouts.evidence_nodes_required = true;
                    }
                    Value::Coerce(source) => {
                        if let Some(destination) = dst {
                            let source_ty = function.vars[*source].ty;
                            let destination_ty = function.vars[*destination].ty;
                            if matches!(layouts.types[source_ty], Type::Any)
                                && !matches!(layouts.types[destination_ty], Type::Any)
                            {
                                layouts.add_any_box_type(destination_ty);
                            } else if matches!(layouts.types[destination_ty], Type::Any)
                                && !matches!(layouts.types[source_ty], Type::Any)
                            {
                                layouts.add_any_box_type(source_ty);
                            }
                            if matches!(layouts.types[source_ty], Type::Fn(_))
                                && matches!(layouts.types[destination_ty], Type::Fn(_))
                            {
                                layouts.add_function_adapter(
                                    source_ty,
                                    destination_ty,
                                    &function.name,
                                )?;
                            }
                        }
                    }
                    Value::Call {
                        callee: Callee::Op { effect, op, .. },
                        args,
                        ..
                    } if flow.core.effects[*effect].ops[*op].type_params > 0 => {
                        for argument in args {
                            if let Some(ty) = operand_type(function, argument) {
                                layouts.add_any_box_type(ty);
                            }
                        }
                        if let Some(destination) = dst {
                            layouts.add_any_box_type(function.vars[*destination].ty);
                        }
                    }
                    _ => {}
                }
            }
        }
        // Signature edges do not embed their parameter/result layouts in a function record.
        // Collect them after the current inline aggregate has finished, so a callback returning
        // a tuple containing its enclosing enum does not create an artificial recursive box.
        let mut function_index = 0;
        let mut list_index = 0;
        let mut cell_index = 0;
        loop {
            if let Some(&ty) = layouts.function_types.get(function_index) {
                let Type::Fn(ft) = layouts.types[ty].clone() else {
                    unreachable!()
                };
                function_index += 1;
                for dependency in ft.params.into_iter().chain([ft.ret]) {
                    layouts.collect(dependency, "function value signature", &mut states)?;
                }
            } else if let Some(&ty) = layouts.list_types.get(list_index) {
                let Type::List(element) = layouts.types[ty] else {
                    unreachable!()
                };
                list_index += 1;
                layouts.collect(element, "List element", &mut states)?;
            } else if let Some(&ty) = layouts.cell_types.get(cell_index) {
                let Type::Cell(content) = layouts.types[ty] else {
                    unreachable!()
                };
                cell_index += 1;
                layouts.collect(content, "Cell content", &mut states)?;
            } else {
                break;
            }
        }
        layouts.has_error_abi |= layouts.function_types.iter().any(
            |&ty| matches!(&layouts.types[ty], Type::Fn(function_type) if function_type.fails),
        );
        if layouts.has_error_abi {
            let error_ids: Vec<EnumId> = flow
                .core
                .enums
                .iter()
                .filter_map(|(id, def)| {
                    (def.error
                        && states.keys().any(
                            |ty| matches!(layouts.types[*ty], Type::Enum(used, _) if used == id),
                        ))
                    .then_some(id)
                })
                .collect();
            for id in error_ids {
                let ty = layouts.types.enum_type(id, Vec::new());
                layouts.collect(ty, "EmelaError", &mut states)?;
                layouts.error_types.push((id, ty));
            }
        }
        // EmelaAny is an EmelaError record, which embeds each concrete error payload.
        // An inline Any within an error payload would therefore make its C layout infinite.
        // Reuse recursive field boxes for these edges, including ones nested in tuples.
        let error_dependencies = layouts.error_declaration_dependencies();
        for &index in &error_dependencies {
            let aggregate = &layouts.aggregates[index];
            let variants = match &aggregate.kind {
                AggregateKind::Tuple(fields) => vec![fields.as_slice()],
                AggregateKind::Enum(variants) => variants.iter().map(Vec::as_slice).collect(),
            };
            for (variant, fields) in variants.into_iter().enumerate() {
                for (field, &ty) in fields.iter().enumerate() {
                    if matches!(layouts.types[ty], Type::Any) {
                        layouts
                            .recursive_edges
                            .insert((aggregate.ty, variant, field));
                    }
                }
            }
        }
        for aggregate in &layouts.aggregates {
            let fields = match &aggregate.kind {
                AggregateKind::Tuple(fields) => vec![fields.as_slice()],
                AggregateKind::Enum(variants) => variants.iter().map(Vec::as_slice).collect(),
            };
            for (variant, fields) in fields.into_iter().enumerate() {
                for (field, &field_ty) in fields.iter().enumerate() {
                    if layouts
                        .recursive_edges
                        .contains(&(aggregate.ty, variant, field))
                        && layouts.has_materialized_payload(field_ty)
                        && !layouts.recursive_box_ids.contains_key(&field_ty)
                    {
                        let index = layouts.recursive_box_types.len();
                        layouts.recursive_box_types.push(field_ty);
                        layouts.recursive_box_ids.insert(field_ty, index);
                    }
                }
            }
        }
        layouts.managed_error_payloads = layouts
            .error_types
            .iter()
            .any(|&(_, ty)| layouts.is_managed_value(ty));
        for (id, function) in flow.functions.iter() {
            if !emitted.contains(&id) {
                continue;
            }
            let mut types = function
                .vars
                .iter()
                .map(|(_, variable)| variable.ty)
                .collect::<Vec<_>>();
            types.push(function.ret);
            types.extend(layouts.list_types.iter().copied());
            for ty in types {
                if !layouts.contains_heap_ref(ty, &mut HashSet::new()) {
                    continue;
                }
                match layouts.types[ty].clone() {
                    Type::List(_) => {}
                    Type::Cell(_) => {}
                    Type::Handler(_) | Type::Evidence | Type::FiberScope => {}
                    Type::Fn(_) => {}
                    Type::Any => {}
                    Type::Tuple(_) | Type::Enum(_, _) => {}
                    _ => {
                        return Err(unsupported(
                            &function.name,
                            "managed List reference in an unsupported value type",
                        ));
                    }
                }
            }
        }
        Ok(layouts)
    }

    fn add_any_box_type(&mut self, ty: TypeId) {
        self.any_abi = true;
        if self.type_repr(ty) == Repr::Void || matches!(self.types[ty], Type::Any) {
            return;
        }
        self.any_box_required = true;
        if self.is_runtime_handle(ty) {
            return;
        }
        if !self.any_box_ids.contains_key(&ty) {
            let index = self.any_box_types.len();
            self.any_box_types.push(ty);
            self.any_box_ids.insert(ty, index);
        }
    }

    fn add_function_adapter(
        &mut self,
        source: TypeId,
        destination: TypeId,
        function: &str,
    ) -> Result<(), Vec<Error>> {
        let (Type::Fn(source_fn), Type::Fn(destination_fn)) =
            (self.types[source].clone(), self.types[destination].clone())
        else {
            return Err(invalid_flow_ir(
                function,
                "function Coerce has non-function types",
            ));
        };
        if self.closure_function_types.contains(&source) {
            self.closure_function_types.insert(destination);
        }
        if source_fn.params == destination_fn.params
            && source_fn.ret == destination_fn.ret
            && source_fn.fails == destination_fn.fails
            && source_fn.suspends == destination_fn.suspends
            && source_fn.ev == destination_fn.ev
        {
            return Ok(());
        }
        let compatible = source_fn.params == destination_fn.params
            && source_fn.ret == destination_fn.ret
            && (!source_fn.suspends || destination_fn.suspends)
            && source_fn.sets.is_none()
            && destination_fn.sets.is_none()
            && (!source_fn.ev || destination_fn.ev);
        if !compatible {
            return Err(unsupported(
                function,
                &format!(
                    "function-value coercion with incompatible parameters, result, or effect flags ({source_fn:?} -> {destination_fn:?})"
                ),
            ));
        }
        if self
            .function_adapter_ids
            .contains_key(&(source, destination))
        {
            return Ok(());
        }
        let index = self.function_adapters.len();
        self.function_adapters.push(FunctionAdapterLayout {
            source,
            destination,
        });
        self.function_adapter_ids
            .insert((source, destination), index);
        self.closure_function_types.insert(destination);
        Ok(())
    }

    fn validate_finite_types(
        &mut self,
        ty: TypeId,
        _function: &str,
        active: &mut HashSet<TypeId>,
        validated: &mut HashSet<TypeId>,
    ) -> Result<(), Vec<Error>> {
        if validated.contains(&ty) || !active.insert(ty) {
            return Ok(());
        }
        let dependencies = match self.types[ty].clone() {
            Type::Fn(ft) => ft.params.into_iter().chain([ft.ret]).collect(),
            Type::Tuple(fields) => fields,
            Type::List(element) | Type::Cell(element) => vec![element],
            Type::Enum(id, args) if self.enum_layout_required(id) => self.core.enums[id]
                .variants
                .iter()
                .flat_map(|variant| &variant.tys)
                .map(|&field| self.types.subst(field, &args))
                .collect(),
            _ => vec![],
        };
        let lazy_fields =
            matches!(self.types[ty], Type::Enum(id, _) if self.expanding_enums.contains(&id));
        for dependency in dependencies {
            if lazy_fields && self.type_repr(dependency) == Repr::Ref {
                continue;
            }
            self.validate_finite_types(dependency, _function, active, validated)?;
        }
        active.remove(&ty);
        validated.insert(ty);
        Ok(())
    }

    fn collect(
        &mut self,
        ty: TypeId,
        function: &str,
        states: &mut HashMap<TypeId, u8>,
    ) -> Result<(), Vec<Error>> {
        if let Type::Enum(id, args) = self.types[ty].clone()
            && self.core.enums[id].name == "Std.Fiber.Exit"
            && args
                .iter()
                .any(|&arg| matches!(self.types[arg], Type::Set(_)))
        {
            // Flow erases the error set of Await to Unit. A declared Exit[A, {}] uses the
            // same runtime layout, including across direct function result boundaries.
            let args = args
                .into_iter()
                .map(|arg| {
                    if matches!(self.types[arg], Type::Set(_)) {
                        TypeId::UNIT
                    } else {
                        arg
                    }
                })
                .collect();
            let canonical = self.types.enum_type(id, args);
            self.collect(canonical, function, states)?;
            let index = self.aggregate_ids[&canonical];
            self.aggregate_ids.insert(ty, index);
            states.insert(ty, 2);
            return Ok(());
        }
        if matches!(self.types[ty], Type::Cell(_)) {
            if !self.cell_ids.contains_key(&ty) {
                self.cell_ids.insert(ty, self.cell_types.len());
                self.cell_types.push(ty);
            }
            return Ok(());
        }
        if matches!(self.types[ty], Type::Fn(_)) {
            if self.function_types.contains(&ty) {
                return Ok(());
            }
            self.function_types.push(ty);
            return Ok(());
        }
        if matches!(self.types[ty], Type::Any) {
            self.any_abi = true;
            return Ok(());
        }
        if matches!(self.types[ty], Type::List(_)) {
            if !self.list_ids.contains_key(&ty) {
                let index = self.list_types.len();
                self.list_types.push(ty);
                self.list_ids.insert(ty, index);
            }
            return Ok(());
        }
        let kind = match self.types[ty].clone() {
            Type::Tuple(fields) => Some(AggregateKind::Tuple(fields)),
            Type::Enum(id, args) if self.enum_layout_required(id) => {
                let raw_fields: Vec<Vec<TypeId>> = self.core.enums[id]
                    .variants
                    .iter()
                    .map(|variant| variant.tys.clone())
                    .collect();
                let fields = raw_fields
                    .iter()
                    .map(|variant| {
                        variant
                            .iter()
                            .map(|&field| self.types.subst(field, &args))
                            .collect()
                    })
                    .collect();
                Some(AggregateKind::Enum(fields))
            }
            _ => None,
        };
        let Some(kind) = kind else {
            return Ok(());
        };

        match states.get(&ty) {
            Some(2) => return Ok(()),
            Some(1) => {
                return Err(unsupported(
                    function,
                    &format!("recursive inline aggregate type {}", type_name(ty)),
                ));
            }
            _ => {}
        }
        states.insert(ty, 1);

        let variants = match &kind {
            AggregateKind::Tuple(fields) => vec![fields.as_slice()],
            AggregateKind::Enum(variants) => variants.iter().map(Vec::as_slice).collect(),
        };
        for (variant, fields) in variants.into_iter().enumerate() {
            for (index, &field) in fields.iter().enumerate() {
                if self.type_repr(field) == Repr::Void {
                    continue;
                }
                let lazy_field = matches!(self.types[ty], Type::Enum(id, _) if self.expanding_enums.contains(&id))
                    && self.type_repr(field) == Repr::Ref;
                if lazy_field || matches!(states.get(&field), Some(1)) {
                    self.recursive_edges.insert((ty, variant, index));
                } else {
                    self.collect(field, function, states)?;
                }
            }
        }

        let index = self.aggregates.len();
        self.aggregate_ids.insert(ty, index);
        self.aggregates.push(AggregateLayout { ty, kind });
        states.insert(ty, 2);
        Ok(())
    }

    fn enum_is_i32(&self, id: emela_core::EnumId) -> bool {
        let def = &self.core.enums[id];
        !def.error
            && !def.external
            && !def.variants.is_empty()
            && def.variants.iter().all(|variant| variant.fields.is_empty())
    }

    fn enum_layout_required(&self, id: emela_core::EnumId) -> bool {
        let def = &self.core.enums[id];
        // Cause's source field describes its error set; it is never a runtime closure.
        def.name != "Std.Cause.Cause"
            && !def.external
            && !def.variants.is_empty()
            && !self.enum_is_i32(id)
    }

    fn type_repr(&self, ty: TypeId) -> Repr {
        match self.types[ty] {
            Type::Int | Type::Bool => Repr::I32,
            Type::Int64 => Repr::I64,
            Type::Float => Repr::F64,
            Type::Unit | Type::Never | Type::Set(_) => Repr::Void,
            Type::Enum(id, _) if self.enum_is_i32(id) => Repr::I32,
            _ => Repr::Ref,
        }
    }

    fn c_type(&self, ty: TypeId) -> String {
        match self.types[ty] {
            Type::Int | Type::Bool => "int32_t".to_owned(),
            Type::Int64 => "int64_t".to_owned(),
            Type::Float => "double".to_owned(),
            Type::Unit | Type::Never | Type::Set(_) => "void".to_owned(),
            Type::String => "EmelaString".to_owned(),
            Type::Tuple(_) => type_name(ty),
            Type::Fn(_) => function_type_name(ty),
            Type::Enum(id, _) if self.enum_is_i32(id) => "int32_t".to_owned(),
            Type::Enum(id, _) if self.enum_layout_required(id) => {
                type_name(self.aggregates[self.aggregate_ids[&ty]].ty)
            }
            Type::Any => "EmelaAny".to_owned(),
            _ => c_type(self.type_repr(ty)).to_owned(),
        }
    }

    fn is_named_external(&self, ty: TypeId, name: &str) -> bool {
        matches!(&self.types[ty], Type::Enum(id, _) if self.core.enums[*id].external && self.core.enums[*id].name == name)
    }

    fn is_runtime_handle(&self, ty: TypeId) -> bool {
        self.is_named_external(ty, "Std.Fiber.Fiber")
            || self.is_named_external(ty, "Std.Deferred.Deferred")
            || self.is_cause(ty)
    }

    fn is_cause(&self, ty: TypeId) -> bool {
        matches!(&self.types[ty], Type::Enum(id, _) if self.core.enums[*id].name == "Std.Cause.Cause")
    }

    fn error_enum(&self, ty: TypeId) -> Option<EnumId> {
        match self.types[ty] {
            Type::Enum(id, _) if self.core.enums[id].error => Some(id),
            _ => None,
        }
    }

    fn error_type(&self, enum_id: EnumId) -> Option<TypeId> {
        self.error_types
            .iter()
            .find_map(|(id, ty)| (*id == enum_id).then_some(*ty))
    }

    fn enum_fields(&self, ty: TypeId) -> Option<&[Vec<TypeId>]> {
        let index = *self.aggregate_ids.get(&ty)?;
        match &self.aggregates[index].kind {
            AggregateKind::Enum(variants) => Some(variants),
            AggregateKind::Tuple(_) => None,
        }
    }

    fn has_cell_cycles(&self) -> bool {
        self.cell_types.iter().any(|&ty| {
            let Type::Cell(content) = self.types[ty] else {
                unreachable!()
            };
            self.cell_content_can_cycle(content, &mut HashSet::new())
        })
    }

    fn cell_content_can_cycle(&self, ty: TypeId, seen: &mut HashSet<TypeId>) -> bool {
        if !seen.insert(ty) {
            return false;
        }
        if self.is_runtime_handle(ty) {
            return true;
        }
        match self.types[ty].clone() {
            Type::Any
            | Type::Cell(_)
            | Type::Fn(_)
            | Type::Handler(_)
            | Type::Evidence
            | Type::FiberScope => true,
            Type::List(element) => self.cell_content_can_cycle(element, seen),
            _ => self.aggregate_ids.get(&ty).is_some_and(|&index| {
                let fields = match &self.aggregates[index].kind {
                    AggregateKind::Tuple(fields) => vec![fields.as_slice()],
                    AggregateKind::Enum(variants) => variants.iter().map(Vec::as_slice).collect(),
                };
                fields
                    .into_iter()
                    .flatten()
                    .any(|&field| self.cell_content_can_cycle(field, seen))
            }),
        }
    }

    fn cell_content_c_type(&self, content: TypeId) -> String {
        if self.type_repr(content) == Repr::Void {
            "uint8_t".into()
        } else {
            self.c_type(content)
        }
    }

    fn cell_kind(&self, cell: TypeId) -> usize {
        self.list_types.len() + self.recursive_box_types.len() + self.cell_ids[&cell]
    }

    fn list_node_name(&self, ty: TypeId) -> String {
        format!("emela_list_node_{}", self.list_ids[&ty])
    }

    fn list_index(&self, ty: TypeId) -> u32 {
        u32::try_from(self.list_ids[&ty]).expect("too many concrete List layouts")
    }

    fn is_managed_list(&self, ty: TypeId) -> bool {
        matches!(self.types[ty], Type::List(_))
    }

    fn is_managed_value(&self, ty: TypeId) -> bool {
        self.contains_owned_ref(ty, &mut HashSet::new())
    }

    fn contains_owned_ref(&self, ty: TypeId, seen: &mut HashSet<TypeId>) -> bool {
        if self.is_runtime_handle(ty) {
            return true;
        }
        if (self.any_box_required || self.has_error_ownership())
            && matches!(self.types[ty], Type::Any)
        {
            return true;
        }
        if matches!(
            self.types[ty],
            Type::String
                | Type::List(_)
                | Type::Cell(_)
                | Type::Handler(_)
                | Type::Evidence
                | Type::FiberScope
        ) {
            return true;
        }
        if matches!(self.types[ty], Type::Fn(_)) {
            return self.closure_function_types.contains(&ty);
        }
        if !seen.insert(ty) {
            return false;
        }
        let Some(&aggregate_index) = self.aggregate_ids.get(&ty) else {
            return false;
        };
        let aggregate = &self.aggregates[aggregate_index];
        match &aggregate.kind {
            AggregateKind::Tuple(fields) => fields.iter().enumerate().any(|(index, &field)| {
                self.recursive_edges.contains(&(ty, 0, index))
                    || self.contains_owned_ref(field, seen)
            }),
            AggregateKind::Enum(variants) => {
                variants.iter().enumerate().any(|(variant, fields)| {
                    fields.iter().enumerate().any(|(index, &field)| {
                        self.recursive_edges.contains(&(ty, variant, index))
                            || self.contains_owned_ref(field, seen)
                    })
                })
            }
        }
    }

    fn contains_heap_ref(&self, ty: TypeId, seen: &mut HashSet<TypeId>) -> bool {
        if self.is_runtime_handle(ty) {
            return true;
        }
        if (self.any_box_required || self.has_error_ownership())
            && matches!(self.types[ty], Type::Any)
        {
            return true;
        }
        if matches!(
            self.types[ty],
            Type::List(_) | Type::Cell(_) | Type::Handler(_) | Type::Evidence | Type::FiberScope
        ) {
            return true;
        }
        if matches!(self.types[ty], Type::Fn(_)) {
            return self.closure_function_types.contains(&ty);
        }
        if !seen.insert(ty) {
            return false;
        }
        let Some(&aggregate_index) = self.aggregate_ids.get(&ty) else {
            return false;
        };
        let aggregate = &self.aggregates[aggregate_index];
        match &aggregate.kind {
            AggregateKind::Tuple(fields) => fields.iter().enumerate().any(|(index, &field)| {
                self.recursive_edges.contains(&(ty, 0, index))
                    || self.contains_heap_ref(field, seen)
            }),
            AggregateKind::Enum(variants) => {
                variants.iter().enumerate().any(|(variant, fields)| {
                    fields.iter().enumerate().any(|(index, &field)| {
                        self.recursive_edges.contains(&(ty, variant, index))
                            || self.contains_heap_ref(field, seen)
                    })
                })
            }
        }
    }

    fn is_recursive_box_field(&self, owner: TypeId, variant: usize, field: usize) -> bool {
        self.recursive_edges.contains(&(owner, variant, field))
    }

    fn has_materialized_payload(&self, ty: TypeId) -> bool {
        match self.types[ty] {
            Type::Enum(id, _) if self.enum_layout_required(id) => {
                self.aggregate_ids.contains_key(&ty)
            }
            Type::Tuple(_) => self.aggregate_ids.contains_key(&ty),
            Type::Fn(_) => self.function_types.contains(&ty),
            Type::List(_) => self.list_ids.contains_key(&ty),
            Type::Cell(_) => self.cell_ids.contains_key(&ty),
            Type::Any => self.any_abi,
            _ => true,
        }
    }

    fn recursive_box_kind(&self, ty: TypeId) -> Option<u32> {
        let index = *self.recursive_box_ids.get(&ty)?;
        u32::try_from(self.list_types.len() + index).ok()
    }

    fn string_owner_kind(&self) -> u32 {
        u32::try_from(
            self.list_types.len() + self.recursive_box_types.len() + self.cell_types.len(),
        )
        .expect("too many fixed-heap object kinds")
    }

    fn any_box_kind(&self) -> u32 {
        self.runtime_value_kind_base()
            + u32::try_from(self.function_adapters.len()).expect("too many function adapters")
            + u32::from(self.evidence_nodes_required)
            + u32::try_from(self.handler_layouts.len()).expect("too many handler effects")
    }

    fn closure_environment_kind(&self, index: usize) -> u32 {
        u32::try_from(self.string_owner_kind() as usize + 1 + index)
            .expect("too many fixed-heap object kinds")
    }

    fn aggregate_helper_index(&self, ty: TypeId) -> Option<usize> {
        let index = *self.aggregate_ids.get(&ty)?;
        self.is_managed_value(ty).then_some(index)
    }

    fn emit_retain_value_expression(&self, ty: TypeId, expression: &str, writer: &mut Writer) {
        if self.is_managed_list(ty)
            || self.is_runtime_handle(ty)
            || matches!(
                self.types[ty],
                Type::Cell(_) | Type::Handler(_) | Type::Evidence | Type::FiberScope
            )
        {
            writer.line(&format!("emela_heap_retain({expression});"));
        } else if matches!(self.types[ty], Type::Any) {
            writer.line(&format!("emela_any_retain({expression});"));
        } else if matches!(self.types[ty], Type::String) {
            writer.line(&format!("emela_heap_retain(({expression}).owner);"));
        } else if matches!(self.types[ty], Type::Fn(_)) {
            writer.line(&format!("emela_heap_retain(({expression}).env);"));
        } else if let Some(index) = self.aggregate_helper_index(ty) {
            writer.line(&format!(
                "emela_heap_retain_value_{index}(&({expression}));"
            ));
        }
    }

    fn emit_enqueue_value_expression(
        &self,
        ty: TypeId,
        expression: &str,
        pending: &str,
        tail: &str,
        writer: &mut Writer,
    ) {
        if self.is_managed_list(ty)
            || self.is_runtime_handle(ty)
            || matches!(
                self.types[ty],
                Type::Cell(_) | Type::Handler(_) | Type::Evidence | Type::FiberScope
            )
        {
            writer.line(&format!(
                "emela_heap_enqueue({expression}, {pending}, {tail});"
            ));
        } else if matches!(self.types[ty], Type::Any) {
            writer.line(&format!(
                "emela_any_enqueue({expression}, {pending}, {tail});"
            ));
        } else if matches!(self.types[ty], Type::String) {
            writer.line(&format!(
                "emela_heap_enqueue(({expression}).owner, {pending}, {tail});"
            ));
        } else if matches!(self.types[ty], Type::Fn(_)) {
            writer.line(&format!(
                "emela_heap_enqueue(({expression}).env, {pending}, {tail});"
            ));
        } else if let Some(index) = self.aggregate_helper_index(ty) {
            writer.line(&format!(
                "emela_heap_enqueue_value_{index}(&({expression}), {pending}, {tail});"
            ));
        }
    }

    fn emit_release_value_expression(&self, ty: TypeId, expression: &str, writer: &mut Writer) {
        if self.is_managed_list(ty)
            || self.is_runtime_handle(ty)
            || matches!(
                self.types[ty],
                Type::Cell(_) | Type::Handler(_) | Type::Evidence | Type::FiberScope
            )
        {
            writer.line(&format!("emela_heap_release({expression});"));
        } else if matches!(self.types[ty], Type::Any) {
            writer.line(&format!("emela_any_release({expression});"));
        } else if matches!(self.types[ty], Type::String) {
            writer.line(&format!("emela_heap_release(({expression}).owner);"));
        } else if matches!(self.types[ty], Type::Fn(_)) {
            writer.line(&format!("emela_heap_release(({expression}).env);"));
        } else if let Some(index) = self.aggregate_helper_index(ty) {
            writer.line(&format!(
                "emela_heap_release_value_{index}(&({expression}));"
            ));
        }
    }

    fn emit_noheap_ownership_stubs(&self, writer: &mut Writer) {
        for (index, aggregate) in self.aggregates.iter().enumerate() {
            if !self.is_managed_value(aggregate.ty)
                || self.contains_heap_ref(aggregate.ty, &mut HashSet::new())
            {
                continue;
            }
            let aggregate_type = self.c_type(aggregate.ty);
            writer.begin_block(&format!(
                "static inline EMELA_UNUSED void emela_heap_retain_value_{index}(const {aggregate_type} *value)"
            ));
            writer.line("(void)value;");
            writer.end_block();
            writer.begin_block(&format!(
                "static inline EMELA_UNUSED void emela_heap_release_value_{index}({aggregate_type} *value)"
            ));
            writer.line("(void)value;");
            writer.end_block();
            writer.blank();
        }
    }

    fn emit_heap_ownership_helpers(&self, writer: &mut Writer) {
        for (index, aggregate) in self.aggregates.iter().enumerate() {
            if !self.is_managed_value(aggregate.ty) {
                continue;
            }
            let aggregate_type = self.c_type(aggregate.ty);
            writer.begin_block(&format!(
                "static void emela_heap_retain_value_{index}(const {aggregate_type} *value)"
            ));
            self.emit_aggregate_retain_body(aggregate, writer);
            writer.end_block();

            writer.begin_block(&format!(
                "static void emela_heap_enqueue_value_{index}(const {aggregate_type} *value, emela_heap_block **pending, emela_heap_block **tail)"
            ));
            self.emit_aggregate_enqueue_body(aggregate, writer);
            writer.end_block();

            writer.begin_block(&format!(
                "static void emela_heap_release_value_{index}(const {aggregate_type} *value)"
            ));
            writer.line("emela_heap_block *pending = NULL;");
            writer.line("emela_heap_block *tail = NULL;");
            writer.line(&format!(
                "emela_heap_enqueue_value_{index}(value, &pending, &tail);"
            ));
            writer.line("emela_heap_drain(pending, tail);");
            writer.end_block();
            writer.blank();
        }
    }

    fn emit_aggregate_retain_body(&self, aggregate: &AggregateLayout, writer: &mut Writer) {
        match &aggregate.kind {
            AggregateKind::Tuple(fields) => {
                for (index, &field_ty) in fields.iter().enumerate() {
                    if self.is_recursive_box_field(aggregate.ty, 0, index) {
                        writer.line(&format!("emela_heap_retain(value->f{index});"));
                        continue;
                    }
                    if self.is_managed_value(field_ty) {
                        self.emit_retain_value_expression(
                            field_ty,
                            &format!("value->f{index}"),
                            writer,
                        );
                    }
                }
            }
            AggregateKind::Enum(variants) => {
                writer.begin_block("switch (value->tag)");
                for (variant, fields) in variants.iter().enumerate() {
                    writer.line(&format!("case INT32_C({variant}):"));
                    writer.begin_block("");
                    for (index, &field_ty) in fields.iter().enumerate() {
                        if self.is_recursive_box_field(aggregate.ty, variant, index) {
                            writer.line(&format!(
                                "emela_heap_retain(value->payload.v{variant}.f{index});"
                            ));
                            continue;
                        }
                        if self.is_managed_value(field_ty) {
                            self.emit_retain_value_expression(
                                field_ty,
                                &format!("value->payload.v{variant}.f{index}"),
                                writer,
                            );
                        }
                    }
                    writer.line("break;");
                    writer.end_block();
                }
                writer.line("default:");
                writer.begin_block("");
                writer.line("break;");
                writer.end_block();
                writer.end_block();
            }
        }
    }

    fn emit_aggregate_enqueue_body(&self, aggregate: &AggregateLayout, writer: &mut Writer) {
        match &aggregate.kind {
            AggregateKind::Tuple(fields) => {
                for (index, &field_ty) in fields.iter().enumerate() {
                    if self.is_recursive_box_field(aggregate.ty, 0, index) {
                        writer.line(&format!(
                            "emela_heap_enqueue(value->f{index}, pending, tail);"
                        ));
                        continue;
                    }
                    if self.is_managed_value(field_ty) {
                        self.emit_enqueue_value_expression(
                            field_ty,
                            &format!("value->f{index}"),
                            "pending",
                            "tail",
                            writer,
                        );
                    }
                }
            }
            AggregateKind::Enum(variants) => {
                writer.begin_block("switch (value->tag)");
                for (variant, fields) in variants.iter().enumerate() {
                    writer.line(&format!("case INT32_C({variant}):"));
                    writer.begin_block("");
                    for (index, &field_ty) in fields.iter().enumerate() {
                        if self.is_recursive_box_field(aggregate.ty, variant, index) {
                            writer.line(&format!(
                                "emela_heap_enqueue(value->payload.v{variant}.f{index}, pending, tail);"
                            ));
                            continue;
                        }
                        if self.is_managed_value(field_ty) {
                            self.emit_enqueue_value_expression(
                                field_ty,
                                &format!("value->payload.v{variant}.f{index}"),
                                "pending",
                                "tail",
                                writer,
                            );
                        }
                    }
                    writer.line("break;");
                    writer.end_block();
                }
                writer.line("default:");
                writer.begin_block("");
                writer.line("break;");
                writer.end_block();
                writer.end_block();
            }
        }
    }

    fn emit_heap_destroy_switch(
        &self,
        environments: &[crate::static_closure::ClosureEnvironment],
        writer: &mut Writer,
    ) {
        if self.has_resumes && self.has_error_ownership() {
            writer.line("static void emela_resume_enqueue_error(void *value, emela_heap_block **pending, emela_heap_block **tail);");
        }
        writer.begin_block(
            "static void emela_heap_destroy(uint32_t kind, void *value, emela_heap_block **pending, emela_heap_block **tail)",
        );
        writer.line("(void)value; (void)pending; (void)tail;");
        writer.begin_block("switch (kind)");
        for &list_ty in &self.list_types {
            let Type::List(element) = self.types[list_ty] else {
                unreachable!("list layout has a non-List type")
            };
            let node_type = self.list_node_name(list_ty);
            let kind = self.list_index(list_ty);
            writer.line(&format!("case UINT32_C({kind}):"));
            writer.begin_block("");
            writer.line(&format!("{node_type} *node = ({node_type} *)value;"));
            if self.type_repr(element) != Repr::Void && self.is_managed_value(element) {
                self.emit_enqueue_value_expression(
                    element,
                    "node->head",
                    "pending",
                    "tail",
                    writer,
                );
            }
            writer.line("emela_heap_enqueue(node->tail, pending, tail);");
            writer.line("break;");
            writer.end_block();
        }
        for &box_ty in &self.recursive_box_types {
            let box_index = self.recursive_box_ids[&box_ty];
            let kind = self.list_types.len() + box_index;
            writer.line(&format!("case UINT32_C({kind}):"));
            writer.begin_block("");
            self.emit_enqueue_value_expression(
                box_ty,
                &format!("(*(({} const *)value))", self.c_type(box_ty)),
                "pending",
                "tail",
                writer,
            );
            writer.line("break;");
            writer.end_block();
        }
        for (index, &cell_ty) in self.cell_types.iter().enumerate() {
            let Type::Cell(content) = self.types[cell_ty] else {
                unreachable!()
            };
            let kind = self.list_types.len() + self.recursive_box_types.len() + index;
            writer.line(&format!("case UINT32_C({kind}):"));
            writer.begin_block("");
            self.emit_enqueue_value_expression(
                content,
                &format!("(*(({} *)value))", self.cell_content_c_type(content)),
                "pending",
                "tail",
                writer,
            );
            writer.line("break;");
            writer.end_block();
        }
        let string_owner_kind = self.string_owner_kind();
        writer.line(&format!("case UINT32_C({string_owner_kind}):"));
        writer.begin_block("");
        writer.line("break;");
        writer.end_block();
        if self.has_resumes {
            if self.has_error_ownership() {
                writer.line(&format!(
                    "case UINT32_C({}): emela_resume_enqueue_error(value, pending, tail); break;",
                    self.resume_kind()
                ));
            } else {
                writer.line(&format!("case UINT32_C({}): break;", self.resume_kind()));
            }
        }
        if self.has_causes {
            writer.line(&format!("case UINT32_C({}): {{", self.cause_kind()));
            if self.has_error_ownership() {
                writer.line("emela_cause *cause = (emela_cause *)value;");
                writer.line(
                    "if (cause->kind == 0) emela_error_enqueue(cause->error, pending, tail);",
                );
            }
            writer.line("break; }");
        }
        if self.has_error_notes() {
            writer.line(&format!("case UINT32_C({}): emela_heap_enqueue(((emela_error_notes *)value)->head, pending, tail); break;", self.error_notes_kind()));
            writer.line(&format!("case UINT32_C({}): emela_heap_enqueue(((emela_error_note *)value)->next, pending, tail); break;", self.error_note_kind()));
        }
        if self.has_scopes {
            writer.line(&format!("case UINT32_C({}): {{", self.scope_kind()));
            if self.has_error_ownership() {
                writer.line(
                    "emela_error_enqueue(((emela_scope *)value)->first_error, pending, tail);",
                );
            }
            writer.line("break; }");
            writer.line(&format!("case UINT32_C({}): {{", self.child_kind()));
            writer.line("emela_child *child = (emela_child *)value;");
            if self.has_cell_cycles() {
                writer.line("if (emela_heap_trace_mode == 0 || emela_heap_trace_mode == 3) emela_child_unlink(child);");
            } else {
                writer.line("emela_child_unlink(child);");
            }
            writer.line("emela_heap_enqueue(child->scope, pending, tail);");
            if self.has_error_ownership() {
                writer.line("emela_error_enqueue(child->error, pending, tail);");
            }
            self.emit_enqueue_value_expression(
                TypeId::ANY,
                "child->result",
                "pending",
                "tail",
                writer,
            );
            writer.line("break; }");
            writer.line(&format!("case UINT32_C({}): {{", self.deferred_kind()));
            if self.deferred_pool {
                writer.line("emela_deferred *deferred = (emela_deferred *)value;");
                writer.begin_block("if (deferred->slot != EMELA_SLOT_NONE)");
                if !self.has_cell_cycles() {
                    writer.line("EmelaAny stored = emela_deferred_recycle(deferred);");
                } else {
                    writer.line("EmelaAny stored = emela_heap_trace_mode == 1 || emela_heap_trace_mode == 2 ? emela_deferred_slot_get(deferred)->value : emela_deferred_recycle(deferred);");
                }
                self.emit_enqueue_value_expression(
                    TypeId::ANY,
                    "stored",
                    "pending",
                    "tail",
                    writer,
                );
                writer.end_block();
            }
            writer.line("emela_heap_enqueue(((emela_deferred *)value)->scope, pending, tail);");
            writer.line("break; }");
        }
        for (index, environment) in environments.iter().enumerate() {
            let kind = self.closure_environment_kind(index);
            writer.line(&format!("case UINT32_C({kind}):"));
            writer.begin_block("");
            writer.line(&format!(
                "emela_closure_env_{index} *environment = (emela_closure_env_{index} *)value;"
            ));
            writer.line("(void)environment;");
            for (capture_index, capture) in environment.captures.iter().enumerate() {
                if self.type_repr(capture.ty) != Repr::Void && self.is_managed_value(capture.ty) {
                    self.emit_enqueue_value_expression(
                        capture.ty,
                        &format!("environment->c{capture_index}"),
                        "pending",
                        "tail",
                        writer,
                    );
                }
            }
            writer.line("break;");
            writer.end_block();
        }
        for (index, adapter) in self.function_adapters.iter().enumerate() {
            let kind = self.function_adapter_kind(index);
            writer.line(&format!("case UINT32_C({kind}):"));
            writer.begin_block("");
            writer.line(&format!(
                "emela_fn_coerce_env_{index} *environment = (emela_fn_coerce_env_{index} *)value;"
            ));
            writer.line("(void)environment;");
            if self.is_managed_value(adapter.source) {
                self.emit_enqueue_value_expression(
                    adapter.source,
                    "environment->source",
                    "pending",
                    "tail",
                    writer,
                );
            }
            writer.line("break;");
            writer.end_block();
        }
        if self.evidence_nodes_required {
            let kind = self.evidence_node_kind();
            writer.line(&format!("case UINT32_C({kind}):"));
            writer.begin_block("");
            writer.line("emela_evidence_node *entry = (emela_evidence_node *)value;");
            writer.line("emela_heap_enqueue(entry->handler, pending, tail);");
            writer.line("emela_heap_enqueue(entry->parent, pending, tail);");
            writer.line("break;");
            writer.end_block();
        }
        for layout in &self.handler_layouts {
            let kind = self.handler_kind(layout.effect);
            let handler_type = Self::handler_type_name(layout.effect);
            writer.line(&format!("case UINT32_C({kind}):"));
            writer.begin_block("");
            writer.line(&format!(
                "{handler_type} *handler = ({handler_type} *)value;"
            ));
            writer.line("(void)handler;");
            for (index, &operation) in layout.operations.iter().enumerate() {
                if self.is_managed_value(operation) {
                    self.emit_enqueue_value_expression(
                        operation,
                        &format!("handler->op_{index}"),
                        "pending",
                        "tail",
                        writer,
                    );
                }
            }
            writer.line("break;");
            writer.end_block();
        }
        if self.any_box_required {
            let kind = self.any_box_kind();
            writer.line(&format!("case UINT32_C({kind}):"));
            writer.begin_block("");
            writer.line("emela_any_box *box = (emela_any_box *)value;");
            writer.begin_block("switch (box->type)");
            for (index, &ty) in self.any_box_types.iter().enumerate() {
                writer.line(&format!("case UINT32_C({index}):"));
                writer.begin_block("");
                if self.is_managed_value(ty) {
                    self.emit_enqueue_value_expression(
                        ty,
                        &format!("box->payload.v{index}"),
                        "pending",
                        "tail",
                        writer,
                    );
                }
                writer.line("break;");
                writer.end_block();
            }
            writer.line("default:");
            writer.begin_block("");
            writer.line(
                "emela_board_defect(EMELA_DEFECT_PANIC, \"invalid Any box type\", UINT16_C(20));",
            );
            writer.line("break;");
            writer.end_block();
            writer.end_block();
            writer.line("break;");
            writer.end_block();
        }
        writer.line("default:");
        writer.begin_block("");
        writer.line(&format!(
            "emela_board_defect(EMELA_DEFECT_PANIC, {}, UINT16_C({}));",
            c_string_literal(b"invalid fixed heap reference"),
            b"invalid fixed heap reference".len()
        ));
        writer.line("break;");
        writer.end_block();
        writer.end_block();
        writer.end_block();
        writer.blank();
    }

    fn emit_public_release_declarations(&self, writer: &mut Writer) {
        for (index, aggregate) in self.aggregates.iter().enumerate() {
            if !self.is_managed_value(aggregate.ty) {
                continue;
            }
            let aggregate_type = self.c_type(aggregate.ty);
            writer.line("/* Retains heap references before copying this aggregate value. */");
            writer.line(&format!(
                "void emela_embedded_retain_value_{index}(const {aggregate_type} *value);"
            ));
            writer.line("/* Releases the heap references owned by this aggregate value. */");
            writer.line(&format!(
                "void emela_embedded_release_value_{index}({aggregate_type} *value);"
            ));
        }
    }

    fn emit_public_release_helpers(&self, writer: &mut Writer) {
        for (index, aggregate) in self.aggregates.iter().enumerate() {
            if !self.is_managed_value(aggregate.ty) {
                continue;
            }
            let aggregate_type = self.c_type(aggregate.ty);
            writer.begin_block(&format!(
                "void emela_embedded_retain_value_{index}(const {aggregate_type} *value)"
            ));
            writer.begin_block("if (value != NULL)");
            writer.line(&format!("emela_heap_retain_value_{index}(value);"));
            writer.end_block();
            writer.end_block();
            writer.begin_block(&format!(
                "void emela_embedded_release_value_{index}({aggregate_type} *value)"
            ));
            writer.begin_block("if (value != NULL)");
            writer.line(&format!("emela_heap_release_value_{index}(value);"));
            writer.line(&format!("*value = ({aggregate_type}){{0}};"));
            writer.end_block();
            writer.end_block();
            writer.blank();
        }
    }

    fn emit_public_function_ownership_declarations(&self, writer: &mut Writer) {
        for &ty in &self.function_types {
            if !self.closure_function_types.contains(&ty) {
                continue;
            }
            let function_type = self.c_type(ty);
            writer.line("/* Retains the environment owned by this function value. */");
            writer.line(&format!(
                "void emela_embedded_retain_function_{}(const {function_type} *value);",
                ty.index()
            ));
            writer.line("/* Releases the environment owned by this function value. */");
            writer.line(&format!(
                "void emela_embedded_release_function_{}({function_type} *value);",
                ty.index()
            ));
        }
    }

    fn emit_public_function_ownership_helpers(&self, writer: &mut Writer) {
        for &ty in &self.function_types {
            if !self.closure_function_types.contains(&ty) {
                continue;
            }
            let function_type = self.c_type(ty);
            writer.begin_block(&format!(
                "void emela_embedded_retain_function_{}(const {function_type} *value)",
                ty.index()
            ));
            writer.begin_block("if (value != NULL)");
            writer.line("emela_heap_retain(value->env);");
            writer.end_block();
            writer.end_block();
            writer.blank();
            writer.begin_block(&format!(
                "void emela_embedded_release_function_{}({function_type} *value)",
                ty.index()
            ));
            writer.begin_block("if (value != NULL)");
            writer.line("emela_heap_release(value->env);");
            writer.end_block();
            writer.end_block();
            writer.blank();
        }
    }

    fn emit_list_node_declarations(&self, writer: &mut Writer) {
        for &ty in &self.list_types {
            let Type::List(element) = self.types[ty].clone() else {
                unreachable!("list layout has a non-List type")
            };
            writer.line(&format!("typedef struct {} {{", self.list_node_name(ty)));
            if self.type_repr(element) == Repr::Void {
                writer.line("  uint8_t _unit;");
            } else {
                writer.line(&format!("  {} head;", self.c_type(element)));
            }
            writer.line("  void *tail;");
            writer.line(&format!("}} {};", self.list_node_name(ty)));
            writer.blank();
        }
    }

    fn emit_runtime_value_declarations(&self, writer: &mut Writer) {
        if self.evidence_nodes_required {
            writer.line("typedef struct emela_evidence_node {");
            writer.line("  uint32_t effect;");
            writer.line("  void *handler;");
            writer.line("  void *parent;");
            writer.line("} emela_evidence_node;");
            writer.blank();
        }
        for (index, adapter) in self.function_adapters.iter().enumerate() {
            writer.line(&format!("typedef struct emela_fn_coerce_env_{index} {{"));
            writer.line(&format!("  {} source;", self.c_type(adapter.source)));
            writer.line(&format!("}} emela_fn_coerce_env_{index};"));
            writer.blank();
        }
        if self.any_box_required {
            writer.line("typedef struct emela_any_box {");
            writer.line("  uint32_t type;");
            writer.line("  union {");
            writer.line("    uint8_t _empty;");
            for (index, &ty) in self.any_box_types.iter().enumerate() {
                writer.line(&format!("    {} v{index};", self.c_type(ty)));
            }
            writer.line("  } payload;");
            writer.line("} emela_any_box;");
            writer.blank();
        }
        for layout in &self.handler_layouts {
            writer.line(&format!(
                "typedef struct {} {{",
                Self::handler_type_name(layout.effect)
            ));
            writer.line("  int32_t active;");
            writer.line("  const char *released_message;");
            writer.line("  uint16_t released_message_length;");
            for (index, _) in layout
                .operations
                .iter()
                .enumerate()
                .filter(|(i, _)| self.core.effects[layout.effect].ops[*i].suspend)
            {
                writer.line(&format!("  const char *resume_error_{index};"));
                writer.line(&format!("  uint16_t resume_error_length_{index};"));
            }
            if layout.operations.is_empty() {
                writer.line("  uint8_t _empty;");
            } else {
                for (index, &operation) in layout.operations.iter().enumerate() {
                    writer.line(&format!("  {} op_{index};", self.c_type(operation)));
                }
            }
            writer.line(&format!("}} {};", Self::handler_type_name(layout.effect)));
            writer.blank();
        }
    }

    fn handler_type_name(effect: EffectId) -> String {
        format!("emela_handler_{}", u32::from(effect.into_raw()))
    }

    fn runtime_value_kind_base(&self) -> u32 {
        if self.closure_environments.is_empty() {
            self.string_owner_kind() + 1
        } else {
            self.closure_environment_kind(self.closure_environments.len() - 1) + 1
        }
    }

    fn function_adapter_kind(&self, index: usize) -> u32 {
        self.runtime_value_kind_base() + u32::try_from(index).expect("too many function adapters")
    }

    fn resume_kind(&self) -> u32 {
        self.any_box_kind() + 1
    }

    fn scope_kind(&self) -> u32 {
        self.resume_kind() + 1
    }
    fn child_kind(&self) -> u32 {
        self.resume_kind() + 2
    }
    fn deferred_kind(&self) -> u32 {
        self.resume_kind() + 3
    }
    fn cause_kind(&self) -> u32 {
        self.resume_kind() + 4
    }

    fn evidence_node_kind(&self) -> u32 {
        self.runtime_value_kind_base()
            + u32::try_from(self.function_adapters.len()).expect("too many function adapters")
    }

    fn handler_kind(&self, effect: EffectId) -> u32 {
        let handler_index = self.handler_layout_ids[&effect];
        self.runtime_value_kind_base()
            + u32::try_from(self.function_adapters.len()).expect("too many function adapters")
            + u32::from(self.evidence_nodes_required)
            + u32::try_from(handler_index).expect("too many handler effects")
    }

    fn error_declaration_dependencies(&self) -> HashSet<usize> {
        let mut pending: Vec<_> = self.error_types.iter().map(|&(_, ty)| ty).collect();
        let mut visited = HashSet::new();
        while let Some(ty) = pending.pop() {
            let Some(&index) = self.aggregate_ids.get(&ty) else {
                continue;
            };
            if !visited.insert(index) {
                continue;
            }
            let aggregate = &self.aggregates[index];
            let variants = match &aggregate.kind {
                AggregateKind::Tuple(fields) => vec![fields.as_slice()],
                AggregateKind::Enum(variants) => variants.iter().map(Vec::as_slice).collect(),
            };
            for (variant, fields) in variants.into_iter().enumerate() {
                for (field, &ty) in fields.iter().enumerate() {
                    if !self.is_recursive_box_field(aggregate.ty, variant, field) {
                        pending.push(ty);
                    }
                }
            }
        }
        visited
    }

    fn emit_declarations(&self, writer: &mut Writer) {
        // All named value types are visible in prototypes, even when an aggregate contains
        // a callback whose signature returns that aggregate. Only inline fields require a
        // complete definition, so function records can precede the ordered aggregates.
        for &ty in &self.function_types {
            let name = function_type_name(ty);
            writer.line(&format!("typedef struct {name} {name};"));
        }
        for aggregate in &self.aggregates {
            let name = type_name(aggregate.ty);
            writer.line(&format!("typedef struct {name} {name};"));
        }
        writer.blank();
        for &ty in &self.function_types {
            let Type::Fn(function_type) = self.types[ty].clone() else {
                unreachable!("function type registry contains a non-function")
            };
            let mut parameters = vec!["void *emela_env".to_owned()];
            if function_type.suspends {
                parameters.insert(0, "emela_fiber *fiber".to_owned());
            }
            parameters.extend(
                function_type
                    .params
                    .iter()
                    .filter(|&&parameter| self.type_repr(parameter) != Repr::Void)
                    .map(|&parameter| self.c_type(parameter)),
            );
            if function_type.ev {
                parameters.push("void *emela_evidence".to_owned());
            }
            if function_type.fails {
                parameters.push("EmelaError *emela_error_out".to_owned());
            }
            if function_type.suspends && self.type_repr(function_type.ret) != Repr::Void {
                parameters.push(format!("{} *result", self.c_type(function_type.ret)));
            }
            writer.line(&format!("struct {} {{", function_type_name(ty)));
            writer.line(&format!(
                "  {} (*call)({});",
                if function_type.suspends {
                    "void".to_owned()
                } else {
                    self.c_type(function_type.ret)
                },
                parameters.join(", ")
            ));
            writer.line("  void *env;");
            writer.line("};");
        }
        if !self.function_types.is_empty() {
            writer.blank();
        }
        let error_dependencies = self.error_declaration_dependencies();
        let declarations = self
            .aggregates
            .iter()
            .enumerate()
            .filter(|(index, _)| error_dependencies.contains(index))
            .map(|(_, aggregate)| Some(aggregate))
            .chain(std::iter::once(None))
            .chain(
                self.aggregates
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| !error_dependencies.contains(index))
                    .map(|(_, aggregate)| Some(aggregate)),
            );
        for aggregate in declarations {
            let Some(aggregate) = aggregate else {
                self.emit_error_declaration(writer);
                continue;
            };
            writer.line(&format!("struct {} {{", type_name(aggregate.ty)));
            match &aggregate.kind {
                AggregateKind::Tuple(fields) => {
                    let mut represented = false;
                    for (index, &field) in fields.iter().enumerate() {
                        if self.type_repr(field) == Repr::Void {
                            continue;
                        }
                        represented = true;
                        let field_type = if self.is_recursive_box_field(aggregate.ty, 0, index) {
                            "void *".to_owned()
                        } else {
                            self.c_type(field)
                        };
                        writer.line(&format!("  {field_type} f{index};"));
                    }
                    if !represented {
                        writer.line("  uint8_t _unit;");
                    }
                }
                AggregateKind::Enum(variants) => {
                    writer.line("  int32_t tag;");
                    writer.line("  union {");
                    for (variant_index, fields) in variants.iter().enumerate() {
                        let represented_fields = fields
                            .iter()
                            .enumerate()
                            .filter(|(_, field)| self.type_repr(**field) != Repr::Void)
                            .map(|(field_index, field)| {
                                let field_type = if self.is_recursive_box_field(
                                    aggregate.ty,
                                    variant_index,
                                    field_index,
                                ) {
                                    "void *".to_owned()
                                } else {
                                    self.c_type(*field)
                                };
                                format!("{field_type} f{field_index};")
                            })
                            .collect::<Vec<_>>();
                        let body = if represented_fields.is_empty() {
                            "uint8_t _unit;".to_owned()
                        } else {
                            represented_fields.join(" ")
                        };
                        writer.line(&format!("    struct {{ {body} }} v{variant_index};"));
                    }
                    writer.line("  } payload;");
                }
            }
            writer.line("};");
            writer.blank();
        }
    }

    fn emit_string_declaration(&self, writer: &mut Writer) {
        writer.line(
            "typedef struct { const char *data; uint16_t length; void *owner; } EmelaString;",
        );
        writer.blank();
    }

    fn emit_error_declaration(&self, writer: &mut Writer) {
        if !self.has_error_abi && !self.any_abi {
            return;
        }
        writer.line("struct EmelaError {");
        writer.line("  uint32_t tag;");
        if self.has_error_notes() {
            writer.line("  void *notes;");
        }
        writer.line("  union {");
        if self.any_abi {
            writer.line("    void *any_box;");
        }
        if self.error_types.is_empty() {
            if !self.any_abi {
                writer.line("    uint8_t _empty;");
            }
        } else {
            for (enum_id, ty) in &self.error_types {
                writer.line(&format!(
                    "    {} e{};",
                    self.c_type(*ty),
                    enum_index(*enum_id)
                ));
            }
        }
        writer.line("  } payload;");
        writer.line("};");
        writer.blank();
    }
}

fn type_name(ty: TypeId) -> String {
    format!("emela_type_{}", ty.index())
}

fn function_type_name(ty: TypeId) -> String {
    format!("emela_function_type_{}", ty.index())
}

fn c_type(repr: Repr) -> &'static str {
    match repr {
        Repr::I32 => "int32_t",
        Repr::I64 => "int64_t",
        Repr::F64 => "double",
        Repr::Ref => "void *",
        Repr::Void => "void",
    }
}

fn function_name(id: FnId, function: &Function) -> String {
    let index = u32::from(id.into_raw());
    if function.exported {
        format!("emela_export_{}_{}", c_identifier(&function.name), index)
    } else {
        format!("emela_fn_{index}")
    }
}

fn enum_index(id: EnumId) -> u32 {
    u32::from(id.into_raw())
}

fn c_identifier(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "function".to_owned()
    } else {
        sanitized
    }
}

fn variable_name(id: VarId) -> String {
    format!("emela_v_{}", u32::from(id.into_raw()))
}

fn owned_variable_name(id: VarId) -> String {
    format!("emela_owned_v_{}", u32::from(id.into_raw()))
}

fn statement_name(statement: &emela_lir::Stmt) -> &'static str {
    match statement {
        emela_lir::Stmt::Let { .. } => "Stmt::Let",
        emela_lir::Stmt::If { .. } => "Stmt::If",
        emela_lir::Stmt::Switch { .. } => "Stmt::Switch",
        emela_lir::Stmt::Loop { .. } => "Stmt::Loop",
        emela_lir::Stmt::Block { .. } => "Stmt::Block",
        emela_lir::Stmt::Protect { .. } => "Stmt::Protect",
        emela_lir::Stmt::Dup(_) => "Stmt::Dup",
        emela_lir::Stmt::Drop(_) => "Stmt::Drop",
    }
}

fn value_name(value: &Value) -> &'static str {
    match value {
        Value::Const(_) => "Value::Const",
        Value::Prim { .. } => "Value::Prim",
        Value::Call { .. } => "Value::Call",
        Value::Builtin { .. } => "Value::Builtin",
        Value::Make { .. } => "Value::Make",
        Value::Field { .. } => "Value::Field",
        Value::Tag(_) => "Value::Tag",
        Value::Tuple(_) => "Value::Tuple",
        Value::TupleGet { .. } => "Value::TupleGet",
        Value::List { .. } => "Value::List",
        Value::ListIsEmpty(_) => "Value::ListIsEmpty",
        Value::ListHead(_) => "Value::ListHead",
        Value::ListTail(_) => "Value::ListTail",
        Value::FnRef(_) => "Value::FnRef",
        Value::Closure(_) => "Value::Closure",
        Value::MakeClosure { .. } => "Value::MakeClosure",
        Value::EnvGet { .. } => "Value::EnvGet",
        Value::EvLookup { .. } => "Value::EvLookup",
        Value::EvExtend { .. } => "Value::EvExtend",
        Value::Handler { .. } => "Value::Handler",
        Value::Fiber { .. } => "Value::Fiber",
        Value::IsError { .. } => "Value::IsError",
        Value::AsError { .. } => "Value::AsError",
        Value::Runtime { .. } => "Value::Runtime",
        Value::Coerce { .. } => "Value::Coerce",
    }
}

fn term_name(term: &Term) -> &'static str {
    match term {
        Term::Yield(_) => "Term::Yield",
        Term::Return(_) => "Term::Return",
        Term::Fail(_) => "Term::Fail",
        Term::Break { .. } => "Term::Break",
        Term::Recur(_) => "Term::Recur",
        Term::Defect(_, _) => "Term::Defect",
        Term::Unwind => "Term::Unwind",
        Term::Unreachable => "Term::Unreachable",
    }
}

fn unsupported(function: &str, construct: &str) -> Vec<Error> {
    vec![Error {
        kind: ErrorKind::Unsupported,
        message: format!(
            "embedded C codegen does not support {construct} in function `{function}` yet"
        ),
    }]
}

#[allow(clippy::too_many_arguments)]
fn emit_sync_interrupt_exit(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    strings: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    writer.begin_block("if (emela_current_fiber != NULL && emela_current_fiber->forced)");
    writer.line("emela_current_fiber->forced = 0;");
    writer.line("emela_interrupt_hold(emela_current_fiber);");
    emit_pending_cleanups(
        layouts, function, 0, writer, names, strings, loops, blocks, protects, next_temp,
        next_label,
    )?;
    writer.line("emela_current_fiber->forced = 1;");
    if function.fails {
        writer.line("emela_error_clear(emela_error_out);");
    }
    emit_failure_return(layouts, function, writer);
    writer.end_block();
    Ok(())
}
