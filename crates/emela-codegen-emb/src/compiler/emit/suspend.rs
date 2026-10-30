//! Backend-owned resumable control flow. Input remains verified Flow IR.

use super::*;

pub(super) fn emit_task_failure_ownership(layouts: &TypeLayouts<'_>, writer: &mut Writer) {
    writer.begin_block("static void emela_task_own_error(EmelaError *error)");
    writer.begin_block("switch (error->tag)");
    for &(id, ty) in &layouts.runtime.error_types {
        writer.line(&format!("case UINT32_C({}):", enum_index(id) + 1));
        writer.begin_block("");
        let value = format!("error->payload.e{}", enum_index(id));
        let variant = if layouts.shapes.aggregate_ids.contains_key(&ty) {
            format!("{value}.tag")
        } else {
            value.clone()
        };
        writer.line(&format!(
            "if ((uint32_t)({variant}) >= UINT32_C({})) emela_scheduler_invalid();",
            layouts.shapes.core.enums[id].variants.len()
        ));
        layouts.emit_own_value_in_place(ty, &value, writer);
        writer.line("break;");
        writer.end_block();
    }
    writer.line("default: emela_scheduler_invalid();");
    writer.end_block();
    if layouts.has_error_notes() {
        writer.line("emela_heap_retain(error->notes);");
    }
    writer.end_block();
}

pub(super) fn task_signature(layouts: &TypeLayouts<'_>, function: &Function, name: &str) -> String {
    let mut params = vec!["EmelaTask task".to_owned()];
    params.extend(function.params.iter().map(|id| {
        format!(
            "{} {}",
            layouts.c_type(function.vars[*id].ty),
            variable_name(*id)
        )
    }));
    format!("void {name}({})", params.join(", "))
}

// Keep each generated runtime feature explicit at this emission boundary.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_runtime(
    writer: &mut Writer,
    options: &Options,
    tasks: bool,
    resumes: bool,
    unwind: bool,
    scopes: bool,
    deferred: bool,
    failure: bool,
) {
    if failure {
        writer.line("#define EMELA_FAILURE_RUNTIME 1");
    }
    if deferred {
        writer.line(&format!(
            "#define EMELA_DEFERRED_CAPACITY {}",
            options.deferred_capacity()
        ));
    }
    if scopes {
        writer.line("#define EMELA_SCOPE_RUNTIME 1");
    }
    if resumes {
        writer.line("#define EMELA_RESUME_RUNTIME 1");
    }
    if unwind {
        writer.line("#define EMELA_UNWIND_RUNTIME 1");
    }
    writer.line(&format!(
        "#define EMELA_FIBER_CAPACITY {}",
        options.fiber_capacity()
    ));
    writer.line(&format!(
        "#define EMELA_FRAME_CAPACITY {}",
        options.frame_capacity()
    ));
    writer.line(&format!(
        "#define EMELA_FRAME_BYTES {}",
        options.frame_bytes()
    ));
    if tasks {
        writer.line(&format!(
            "#define EMELA_TASK_CAPACITY {}",
            options.task_capacity()
        ));
        writer.line(&format!(
            "#define EMELA_EVENT_CAPACITY {}",
            options.event_capacity()
        ));
    }
    writer.line(include_str!("runtime/scheduler.c"));
    if tasks {
        writer.line(include_str!("runtime/task.c"));
    }
}

fn parameters(layouts: &TypeLayouts<'_>, function: &Function) -> Vec<String> {
    let mut params = function
        .params
        .iter()
        .map(|id| {
            format!(
                "{} {}",
                layouts.c_type(function.vars[*id].ty),
                variable_name(*id)
            )
        })
        .collect::<Vec<_>>();
    if function.fails {
        params.push("EmelaError *emela_error_out".into());
    }
    if layouts.type_repr(function.ret) != Repr::Void {
        params.push(format!("{} *result", layouts.c_type(function.ret)));
    }
    params
}

pub(super) fn start_signature(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    name: &str,
) -> String {
    let mut params = parameters(layouts, function);
    if function.fails {
        params
            .remove(params.len() - 1 - usize::from(layouts.type_repr(function.ret) != Repr::Void));
    }
    format!(
        "EmelaFiber {name}_start({})",
        if params.is_empty() {
            "void".to_owned()
        } else {
            params.join(", ")
        }
    )
}

fn push_signature(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    name: &str,
    environment: Option<usize>,
) -> String {
    let mut params = vec!["emela_fiber *fiber".to_owned()];
    if environment.is_some() {
        params.push("void *emela_env".to_owned());
    }
    params.extend(parameters(layouts, function));
    format!("static void {name}_push({})", params.join(", "))
}

pub(super) fn emit_declarations(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    name: &str,
    environment: Option<usize>,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    writer.line(&format!(
        "{};",
        push_signature(layouts, function, name, environment)
    ));
    Ok(())
}

use crate::contract::control::ControlNode as Node;

// The entry combines independent layout, ownership, and runtime plans.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_function(
    flow: &crate::contract::ir::Program,
    layouts: &TypeLayouts<'_>,
    _function_id: FnId,
    function: &Function,
    name: &str,
    names: &HashMap<FnId, String>,
    _output_calls: &HashSet<FnId>,
    environment: Option<usize>,
    _options: &Options,
    writer: &mut Writer,
    suspension: Option<&crate::contract::control::SuspensionPlan<'_>>,
) -> Result<(), Vec<Error>> {
    if function.external.is_some() {
        writer.begin_block(&push_signature(layouts, function, name, None));
        let kind = task_kind(layouts, function.ret, &function.name)?;
        let result = if layouts.type_repr(function.ret) == Repr::Void {
            "NULL"
        } else {
            "result"
        };
        let error = if function.fails {
            "emela_error_out"
        } else {
            "NULL"
        };
        writer.line(&format!(
            "EmelaTask task = emela_task_begin(fiber, {result}, EMELA_TASK_{kind}, {error});"
        ));
        let mut args = vec!["task".to_owned()];
        args.extend(function.params.iter().copied().map(variable_name));
        writer.line(&format!("{name}({});", args.join(", ")));
        writer.end_block();
        return Ok(());
    }
    let suspension = suspension.expect("non-external suspension has a prepared frame plan");
    let function = suspension.function;
    let call_error_var = suspension.error_variable;
    let call_error_label = suspension.error_label;
    let graph = &suspension.graph;
    let entry = graph.entry;
    let active = &suspension.active;
    let strings = &suspension.strings;
    let has_result = suspension.has_result;
    writer.line(&format!("typedef struct {name}_frame {{"));
    writer.line("  emela_frame base;");
    let has_failure = suspension.has_failure;
    if has_failure {
        writer.line("  EmelaError call_error;");
        writer.line("  int call_error_owned;");
        writer.line(&format!(
            "  EmelaError pending_error[{}];",
            graph.max_error_depth + 1
        ));
    }
    if function.fails {
        writer.line("  EmelaError *error_out;");
    }
    if graph.max_depth != 0 {
        writer.line(&format!("  emela_protection guards[{}];", graph.max_depth));
        writer.line("  uint32_t guard_depth;");
    }
    if has_result {
        writer.line(&format!("  {} *result;", layouts.c_type(function.ret)));
    }
    for (id, var) in function.vars.iter().filter(|(id, _)| active.contains(id)) {
        writer.line(&format!(
            "  {} local_{};",
            layouts.c_type(var.ty),
            u32::from(id.into_raw())
        ));
        if layouts.is_managed_value(var.ty) {
            writer.line(&format!("  int owned_{};", u32::from(id.into_raw())));
        }
    }
    for (name, capacity) in &strings.declarations {
        writer.line(&format!("  char {name}[{capacity}];"));
    }
    writer.line(&format!("}} {name}_frame;"));
    writer.line(&format!("_Static_assert(sizeof({name}_frame) <= EMELA_FRAME_BYTES, \"increase Options::with_frame_bytes for {name}\");"));
    if has_failure {
        writer.begin_block(&format!(
            "static int {name}_has_failure(emela_fiber *fiber, emela_frame *base)"
        ));
        writer.line("(void)fiber;");
        writer.line(&format!("{name}_frame *frame = ({name}_frame *)base;"));
        writer.line("if (frame->call_error.tag != 0) return 1;");
        writer.line(&format!(
            "for (uint32_t i = 0; i < {}; i++) if (frame->pending_error[i].tag != 0) return 1;",
            graph.max_error_depth + 1
        ));
        writer.line("return 0;");
        writer.end_block();
    }
    if graph.max_depth != 0 {
        writer.begin_block(&format!(
            "static int {name}_unwind(emela_fiber *fiber, emela_frame *base)"
        ));
        writer.line(&format!("{name}_frame *frame = ({name}_frame *)base;"));
        if layouts.runtime.has_error_abi {
            writer.line("while (frame->guard_depth != 0 && frame->guards[frame->guard_depth - 1].running) {");
            writer.line("  if (!fiber->forced) emela_scheduler_invalid();");
            writer.line("  fiber->holds = frame->guards[--frame->guard_depth].holds;");
            writer.line("}");
        }
        writer.line("if (frame->guard_depth == 0) return 0;");
        writer.line("emela_protection_leave(fiber, frame->guards, frame->guard_depth, 0, UINT32_MAX, &base->pc);");
        writer.line("return 1;");
        writer.end_block();
    }
    writer.begin_block(&format!(
        "static void {name}_step(emela_fiber *fiber, emela_frame *base)"
    ));
    writer.line(&format!("{name}_frame *frame = ({name}_frame *)base;"));
    if has_failure {
        writer.line(&format!(
            "#define {} (frame->call_error)",
            variable_name(call_error_var)
        ));
        writer.line(&format!(
            "#define {} (frame->call_error_owned)",
            owned_variable_name(call_error_var)
        ));
    }
    // Aliases let the shared emitter address persistent fields without stack copies.
    for (id, _) in function.vars.iter().filter(|(id, _)| active.contains(id)) {
        writer.line(&format!(
            "#define {} (frame->local_{})",
            variable_name(id),
            u32::from(id.into_raw())
        ));
        let ty = function.vars[id].ty;
        if layouts.is_managed_value(ty) {
            writer.line(&format!(
                "#define {} (frame->owned_{})",
                owned_variable_name(id),
                u32::from(id.into_raw())
            ));
        }
    }
    for (name, _) in &strings.declarations {
        writer.line(&format!("#define {name} (frame->{name})"));
    }
    writer.begin_block("for (;;)");
    if layouts.runtime.has_unwind {
        writer.line("if (emela_should_unwind(fiber)) return;");
    }
    writer.begin_block("switch (frame->base.pc)");
    let mut next_temp = 0;
    for (index, node) in graph.nodes.iter().enumerate() {
        writer.begin_block(&format!("case {index}:"));
        match node {
            Node::Statement(id, next) => {
                let statement = &suspension.statements[id];
                let failing = matches!(
                    statement,
                    Stmt::Let {
                        value: Value::Call {
                            on_fail: Some(_),
                            ..
                        } | Value::Fiber {
                            on_fail: Some(_),
                            ..
                        },
                        ..
                    }
                );
                if failing {
                    writer.line("emela_error_clear(&frame->call_error);");
                }
                if let Stmt::Let {
                    dst: _dst,
                    value:
                        Value::Fiber {
                            op: crate::contract::ir::FiberOp::Attempt { suspends: true, .. },
                            args: _args,
                            ..
                        },
                    ..
                } = statement
                {
                    super::features::fibers::attempt::emit_suspend(
                        &mut super::features::SuspendContext {
                            flow,
                            layouts,
                            function,
                            writer,
                            names,
                            strings,
                            next_temp: &mut next_temp,
                            next: *next,
                            index,
                            failing,
                            call_error_var,
                            call_error_label,
                        },
                        statement,
                    )?;
                } else if let Stmt::Let {
                    dst: _dst,
                    value:
                        Value::Fiber {
                            op: crate::contract::ir::FiberOp::DeferredAwait,
                            args: _args,
                            on_fail: None,
                        },
                    ..
                } = statement
                {
                    super::features::fibers::deferred_await::emit_suspend(
                        &mut super::features::SuspendContext {
                            flow,
                            layouts,
                            function,
                            writer,
                            names,
                            strings,
                            next_temp: &mut next_temp,
                            next: *next,
                            index,
                            failing,
                            call_error_var,
                            call_error_label,
                        },
                        statement,
                    )?;
                } else if let Stmt::Let {
                    dst: _dst,
                    value:
                        Value::Fiber {
                            op:
                                op @ (crate::contract::ir::FiberOp::Join
                                | crate::contract::ir::FiberOp::Await { .. }
                                | crate::contract::ir::FiberOp::Interrupt),
                            args: _args,
                            on_fail: None,
                        },
                    ..
                } = statement
                {
                    super::features::fibers::emit_waiting(
                        &mut super::features::SuspendContext {
                            flow,
                            layouts,
                            function,
                            writer,
                            names,
                            strings,
                            next_temp: &mut next_temp,
                            next: *next,
                            index,
                            failing,
                            call_error_var,
                            call_error_label,
                        },
                        statement,
                        *op,
                    )?;
                } else if let Stmt::Let {
                    dst: _dst,
                    value:
                        Value::Fiber {
                            op: crate::contract::ir::FiberOp::Scope { .. },
                            args: _args,
                            on_fail: _on_fail,
                        },
                    ..
                } = statement
                {
                    super::features::fibers::scope::emit_suspend(
                        &mut super::features::SuspendContext {
                            flow,
                            layouts,
                            function,
                            writer,
                            names,
                            strings,
                            next_temp: &mut next_temp,
                            next: *next,
                            index,
                            failing,
                            call_error_var,
                            call_error_label,
                        },
                        statement,
                    )?;
                } else if let Stmt::Let {
                    dst: _dst,
                    value:
                        Value::Call {
                            callee:
                                Callee::Op {
                                    handler: _handler,
                                    effect,
                                    op,
                                },
                            args: _args,
                            on_fail: _on_fail,
                            ..
                        },
                    ..
                } = statement
                    && layouts.shapes.core.effects[*effect].ops[*op].suspend
                {
                    super::features::values::call::emit_suspend_op(
                        &mut super::features::SuspendContext {
                            flow,
                            layouts,
                            function,
                            writer,
                            names,
                            strings,
                            next_temp: &mut next_temp,
                            next: *next,
                            index,
                            failing,
                            call_error_var,
                            call_error_label,
                        },
                        statement,
                    )?;
                } else if let Stmt::Let {
                    dst: _dst,
                    value:
                        Value::Call {
                            callee: Callee::Indirect(callee),
                            args: _args,
                            on_fail: _on_fail,
                            ..
                        },
                    ..
                } = statement
                    && matches ! (& layouts.shapes.types [function . vars [* callee] . ty] , Type :: Fn (ft) if ft . suspends)
                {
                    super::features::values::call::emit_suspend_indirect(
                        &mut super::features::SuspendContext {
                            flow,
                            layouts,
                            function,
                            writer,
                            names,
                            strings,
                            next_temp: &mut next_temp,
                            next: *next,
                            index,
                            failing,
                            call_error_var,
                            call_error_label,
                        },
                        statement,
                    )?;
                } else if let Stmt::Let {
                    dst: _dst,
                    value:
                        Value::Call {
                            callee: Callee::Direct(_target),
                            args: _args,
                            suspends: true,
                            on_fail: _on_fail,
                            ..
                        },
                    ..
                } = statement
                {
                    super::features::values::call::emit_suspend_direct(
                        &mut super::features::SuspendContext {
                            flow,
                            layouts,
                            function,
                            writer,
                            names,
                            strings,
                            next_temp: &mut next_temp,
                            next: *next,
                            index,
                            failing,
                            call_error_var,
                            call_error_label,
                        },
                        statement,
                    )?;
                } else {
                    emit_sync_statement(
                        layouts,
                        function,
                        statement,
                        failing,
                        call_error_var,
                        call_error_label,
                        index,
                        *next,
                        writer,
                        names,
                        strings,
                        &mut next_temp,
                    )?;
                }
            }
            Node::Assign(_values, _next) => {
                super::features::control_utilities::emit_assignment(
                    &mut super::features::SuspendContext {
                        flow,
                        layouts,
                        function,
                        writer,
                        names,
                        strings,
                        next_temp: &mut next_temp,
                        next: 0,
                        index,
                        failing: false,
                        call_error_var,
                        call_error_label,
                    },
                    node,
                )?;
            }
            Node::Branch(_cond, _yes, _no) => {
                super::features::statements::if_::emit_suspend(
                    &mut super::features::SuspendContext {
                        flow,
                        layouts,
                        function,
                        writer,
                        names,
                        strings,
                        next_temp: &mut next_temp,
                        next: 0,
                        index,
                        failing: false,
                        call_error_var,
                        call_error_label,
                    },
                    node,
                )?;
            }
            Node::Switch(_on, _cases, _default) => {
                super::features::statements::switch::emit_suspend(
                    &mut super::features::SuspendContext {
                        flow,
                        layouts,
                        function,
                        writer,
                        names,
                        strings,
                        next_temp: &mut next_temp,
                        next: 0,
                        index,
                        failing: false,
                        call_error_var,
                        call_error_label,
                    },
                    node,
                )?;
            }
            Node::Return(_value) => {
                super::features::terms::return_::emit_suspend(
                    &mut super::features::SuspendContext {
                        flow,
                        layouts,
                        function,
                        writer,
                        names,
                        strings,
                        next_temp: &mut next_temp,
                        next: 0,
                        index,
                        failing: false,
                        call_error_var,
                        call_error_label,
                    },
                    node,
                )?;
            }
            Node::SetError(_error, _next, _slot) => {
                super::features::terms::fail::emit_set_error(
                    &mut super::features::SuspendContext {
                        flow,
                        layouts,
                        function,
                        writer,
                        names,
                        strings,
                        next_temp: &mut next_temp,
                        next: 0,
                        index,
                        failing: false,
                        call_error_var,
                        call_error_label,
                    },
                    node,
                )?;
            }
            Node::CheckError {
                normal: _,
                failed: _,
                slot: _,
            } => {
                super::features::failures::emit_check(
                    &mut super::features::SuspendContext {
                        flow,
                        layouts,
                        function,
                        writer,
                        names,
                        strings,
                        next_temp: &mut next_temp,
                        next: 0,
                        index,
                        failing: false,
                        call_error_var,
                        call_error_label,
                    },
                    node,
                )?;
            }
            Node::Catch(_dst, _next, _slot) => {
                super::features::failures::emit_catch(
                    &mut super::features::SuspendContext {
                        flow,
                        layouts,
                        function,
                        writer,
                        names,
                        strings,
                        next_temp: &mut next_temp,
                        next: 0,
                        index,
                        failing: false,
                        call_error_var,
                        call_error_label,
                    },
                    node,
                )?;
            }
            Node::Fail(_slot) => {
                super::features::terms::fail::emit_suspend(
                    &mut super::features::SuspendContext {
                        flow,
                        layouts,
                        function,
                        writer,
                        names,
                        strings,
                        next_temp: &mut next_temp,
                        next: 0,
                        index,
                        failing: false,
                        call_error_var,
                        call_error_label,
                    },
                    node,
                )?;
            }
            Node::Defect(_value) => {
                super::features::terms::defect::emit_suspend(
                    &mut super::features::SuspendContext {
                        flow,
                        layouts,
                        function,
                        writer,
                        names,
                        strings,
                        next_temp: &mut next_temp,
                        next: 0,
                        index,
                        failing: false,
                        call_error_var,
                        call_error_label,
                    },
                    node,
                )?;
            }
            Node::Unreachable => {
                super::features::terms::unreachable::emit_suspend(
                    &mut super::features::SuspendContext {
                        flow,
                        layouts,
                        function,
                        writer,
                        names,
                        strings,
                        next_temp: &mut next_temp,
                        next: 0,
                        index,
                        failing: false,
                        call_error_var,
                        call_error_label,
                    },
                    node,
                )?;
            }
            Node::Protect { cleanup, body } => {
                super::features::statements::protect::emit_enter(
                    writer,
                    *cleanup,
                    *body,
                    graph.max_depth,
                );
            }
            Node::Leave { keep, next } => {
                super::features::statements::protect::emit_leave(writer, *keep, *next);
            }
            Node::CleanupEnd => {
                super::features::statements::protect::emit_finish(writer);
            }
        }
        writer.end_block();
    }
    writer.line("default: emela_scheduler_invalid();");
    writer.end_block();
    writer.end_block();
    writer.end_block();
    writer.begin_block(&format!("static void {name}_drop(emela_frame *base)"));
    writer.line(&format!("{name}_frame *frame = ({name}_frame *)base;"));
    writer.line("(void)frame;");
    if has_failure {
        writer.line("emela_error_clear(&frame->call_error);");
        writer.line(&format!(
            "for (uint32_t i = 0; i < {}; i++) emela_error_clear(&frame->pending_error[i]);",
            graph.max_error_depth + 1
        ));
    }
    for (id, _) in function.vars.iter().filter(|(id, _)| active.contains(id)) {
        emit_release_if_owned(layouts, function, id, writer);
    }
    writer.end_block();
    for (id, _) in function.vars.iter().filter(|(id, _)| active.contains(id)) {
        writer.line(&format!("#undef {}", variable_name(id)));
        writer.line(&format!("#undef {}", owned_variable_name(id)));
    }
    for (name, _) in &strings.declarations {
        writer.line(&format!("#undef {name}"));
    }
    if has_failure {
        writer.line(&format!("#undef {}", variable_name(call_error_var)));
        writer.line(&format!("#undef {}", owned_variable_name(call_error_var)));
    }
    writer.begin_block(&push_signature(layouts, function, name, environment));
    writer.line(&format!(
        "emela_frame *base = emela_frame_push(fiber, sizeof({name}_frame), {name}_step);"
    ));
    writer.line(&format!("{name}_frame *frame = ({name}_frame *)base;"));
    // Clear in place: a struct compound literal can put the entire frame on the C stack at O0.
    writer.line("for (size_t offset = sizeof(emela_frame); offset < sizeof(*frame); offset++) ((unsigned char *)frame)[offset] = 0;");
    writer.line(&format!("frame->base.pc = {entry};"));
    writer.line(&format!("frame->base.drop = {name}_drop;"));
    if has_failure {
        writer.line(&format!("frame->base.has_failure = {name}_has_failure;"));
    }
    if graph.max_depth != 0 {
        writer.line(&format!("frame->base.unwind = {name}_unwind;"));
    }
    if has_result {
        writer.line("frame->result = result;");
    }
    if function.fails {
        writer.line("if (emela_error_out != NULL) *emela_error_out = (EmelaError){0};");
        writer.line("frame->error_out = emela_error_out;");
    }
    for &id in &function.params {
        writer.line(&format!(
            "frame->local_{} = {};",
            u32::from(id.into_raw()),
            variable_name(id)
        ));
        let field = format!("frame->local_{}", u32::from(id.into_raw()));
        retain_frame_value(layouts, function.vars[id].ty, &field, writer);
        if layouts.is_managed_value(function.vars[id].ty) {
            writer.line(&format!(
                "frame->owned_{} = INT32_C(1);",
                u32::from(id.into_raw())
            ));
        }
    }
    if let Some(environment) = environment {
        writer.line("if (emela_env == NULL) emela_scheduler_invalid();");
        for (index, capture) in layouts.functions.closure_environments[environment]
            .captures
            .iter()
            .enumerate()
        {
            if layouts.type_repr(capture.ty) == Repr::Void {
                continue;
            }
            let id = u32::from(capture.local.into_raw());
            writer.line(&format!(
                "frame->local_{id} = ((emela_closure_env_{environment} *)emela_env)->c{index};"
            ));
            retain_frame_value(layouts, capture.ty, &format!("frame->local_{id}"), writer);
            if layouts.is_managed_value(capture.ty) {
                writer.line(&format!("frame->owned_{id} = INT32_C(1);"));
            }
        }
    }
    writer.end_block();
    if function.exported {
        writer.begin_block(&start_signature(layouts, function, name));
        writer.line("EmelaFiber handle = emela_fiber_create();");
        writer.line("emela_fiber *fiber = emela_fiber_get(handle);");
        let mut args = vec!["fiber".to_owned()];
        args.extend(function.params.iter().map(|id| variable_name(*id)));
        if function.fails {
            args.push("NULL".into());
        }
        if has_result {
            args.push("result".to_owned());
        }
        writer.line(&format!("{name}_push({});", args.join(", ")));
        writer.line("emela_ready_push(fiber);");
        writer.line("return handle;");
        writer.end_block();
    }
    Ok(())
}

pub(super) fn retain_frame_value(
    layouts: &TypeLayouts<'_>,
    ty: TypeId,
    value: &str,
    writer: &mut Writer,
) {
    layouts.emit_own_value_in_place(ty, value, writer);
}

fn freeze_statement_result(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    statement: &Stmt,
    strings: &StringBufferPlan,
    writer: &mut Writer,
) {
    if let Some(dst) = statement.dst() {
        if strings.discarded_results.contains(&dst) {
            // The synchronous emitter has already released the discarded result.
            return;
        }
        layouts.emit_freeze_value_in_place(function.vars[dst].ty, &variable_name(dst), writer);
    }
}

fn task_kind(layouts: &TypeLayouts<'_>, ty: TypeId, _function: &str) -> Result<String, Vec<Error>> {
    Ok(match layouts.shapes.types[ty] {
        Type::Int | Type::Bool => "INT".into(),
        Type::Int64 => "INT64".into(),
        Type::Float => "FLOAT".into(),
        Type::Unit | Type::Never => "UNIT".into(),
        _ => format!("VALUE_{}", ty.index()),
    })
}

pub(in crate::compiler::emit) fn prepare_result(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    writer: &mut Writer,
) {
    if let Some(dst) = dst {
        emit_release_if_owned(layouts, function, dst, writer);
        if layouts.is_managed_value(function.vars[dst].ty) {
            writer.assign(
                &variable_name(dst),
                &format!("({}){{0}}", layouts.c_type(function.vars[dst].ty)),
            );
            writer.assign(&owned_variable_name(dst), "INT32_C(1)");
        }
    }
}

pub(super) fn emit_resume_runtime(layouts: &TypeLayouts<'_>, writer: &mut Writer) {
    writer.line(&format!(
        "#define EMELA_RESUME_KIND UINT32_C({})",
        layouts.resume_kind()
    ));
    writer.line(include_str!("runtime/resume.c"));
    if layouts.has_error_ownership() {
        writer.begin_block("static void emela_resume_enqueue_error(void *value, emela_heap_block **pending, emela_heap_block **tail)");
        writer.line("emela_error_enqueue(((emela_resume *)value)->error, pending, tail);");
        writer.end_block();
    }
    for &ty in &layouts.runtime.resume_types {
        let Type::Fn(ft) = &layouts.shapes.types[ty] else {
            unreachable!()
        };
        writer.begin_block(
            &function_ref_adapter_signature(
                layouts,
                ty,
                &format!("emela_resume_value_{}", ty.index()),
            )
            .replacen("static ", "static inline EMELA_UNUSED ", 1),
        );
        writer.line("emela_resume *resume = emela_resume_prepare(emela_env);");
        writer.line("if (resume == NULL) return;");
        let result = ft.params[0];
        if layouts.type_repr(result) != Repr::Void {
            writer.begin_block("if (resume->result != NULL)");
            writer.decl(&layouts.c_type(result), "returned", Some("emela_arg_0"));
            retain_frame_value(layouts, result, "returned", writer);
            writer.line(&format!(
                "*(({} *)resume->result) = returned;",
                layouts.c_type(result)
            ));
            writer.end_block();
        }
        writer.line("emela_resume_commit(resume);");
        writer.end_block();
    }
}

pub(super) fn emit_scope_runtime(
    layouts: &TypeLayouts<'_>,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    for (name, kind) in [
        ("SCOPE", layouts.scope_kind()),
        ("CHILD", layouts.child_kind()),
        ("DEFERRED", layouts.deferred_kind()),
    ] {
        writer.line(&format!("#define EMELA_{name}_KIND UINT32_C({kind})"));
    }
    writer.line(include_str!("runtime/scope.c"));
    for &ty in &layouts.runtime.fork_types {
        let Type::Fn(ft) = &layouts.shapes.types[ty] else {
            return Err(unsupported("Fork", "child is not a function"));
        };
        if ft.ev || !ft.suspends || !ft.params.is_empty() || ft.ret != TypeId::ANY {
            return Err(unsupported(
                "Fork",
                "child must be a suspending fn() -> Any",
            ));
        }
        let index = ty.index();
        let cty = layouts.c_type(ty);
        writer.line(&format!(
            "typedef struct {{ emela_frame base; {cty} body; }} emela_fork_frame_{index};"
        ));
        writer.line(&format!("_Static_assert(sizeof(emela_fork_frame_{index}) <= EMELA_FRAME_BYTES, \"increase frame_bytes for Fork\");"));
        writer.begin_block(&format!(
            "static void emela_fork_drop_{index}(emela_frame *base)"
        ));
        writer.line(&format!(
            "emela_fork_frame_{index} *frame = (emela_fork_frame_{index} *)base;"
        ));
        writer.line("(void)frame;");
        if layouts.is_managed_value(ty) {
            layouts.emit_release_value_expression(ty, "frame->body", writer);
        }
        writer.end_block();
        writer.begin_block(&format!(
            "static void emela_fork_step_{index}(emela_fiber *fiber, emela_frame *base)"
        ));
        writer.line(&format!(
            "emela_fork_frame_{index} *frame = (emela_fork_frame_{index} *)base;"
        ));
        writer.begin_block("if (base->pc == 0)");
        writer.line("base->pc = 1;");
        writer.line("if (frame->body.call == NULL) emela_scheduler_invalid();");
        writer.line(&format!(
            "frame->body.call(fiber, frame->body.env{}, &fiber->child->result);",
            if ft.fails {
                ", &fiber->child->error"
            } else {
                ""
            }
        ));
        writer.line("return;");
        writer.end_block();
        writer.line("emela_frame_return(fiber);");
        writer.end_block();
        writer.begin_block(&format!(
            "static inline EMELA_UNUSED EmelaAny emela_fork_{index}(emela_scope *scope, {cty} body)"
        ));
        writer.line("emela_fiber *fiber = emela_child_begin(scope);");
        writer.line(&format!("emela_fork_frame_{index} *frame = (emela_fork_frame_{index} *)emela_frame_push(fiber, sizeof(*frame), emela_fork_step_{index});"));
        writer.line(&format!("frame->base.drop = emela_fork_drop_{index};"));
        if ft.fails {
            writer.line("frame->base.has_failure = emela_child_has_failure;");
        }
        writer.line("frame->body = body;");
        if layouts.is_managed_value(ty) {
            layouts.emit_retain_value_expression(ty, "body", writer);
        }
        writer.line("emela_ready_push(fiber);");
        writer.line("emela_heap_retain(fiber->child);");
        writer.line("return (EmelaAny){.payload.any_box = fiber->child};");
        writer.end_block();
    }
    Ok(())
}

// Forward the operation operands and emission context without changing their ABI.

// The suspension boundary needs both operation state and the caller emission context.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_operation(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    handler: VarId,
    effect: EffectId,
    operation: usize,
    args: &[Operand],
    caller: &str,
    result: &str,
    error_out: Option<&str>,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let index = *layouts
        .shapes
        .handler_layout_ids
        .get(&effect)
        .ok_or_else(|| {
            invalid_flow_ir(&function.name, "suspend operation without handler layout")
        })?;
    let ty = layouts.shapes.handler_layouts[index].operations[operation];
    let Type::Fn(ft) = &layouts.shapes.types[ty] else {
        unreachable!()
    };
    if ft.ev || !ft.suspends {
        return Err(unsupported(
            &function.name,
            "suspend operation with failure or unsupported effect ABI",
        ));
    }
    let resume_ty = *ft.params.last().expect("resume parameter");
    let pointer = format!(
        "(({} *){})",
        crate::semantics::abi::handler_type_name(effect),
        variable_name(handler)
    );
    writer.line(&format!(
        "if ({pointer} == NULL) emela_scheduler_invalid();"
    ));
    writer.line(&format!("if (!{pointer}->active) emela_board_defect(EMELA_DEFECT_PANIC, {pointer}->released_message, {pointer}->released_message_length);"));
    if let Some(dst) = dst {
        prepare_result(layouts, function, Some(dst), writer);
    }
    let error_arg = if layouts.runtime.has_error_abi {
        format!(", {}", error_out.unwrap_or("NULL"))
    } else {
        String::new()
    };
    writer.line(&format!("emela_fiber *implementation = emela_operation_begin({caller}, {result}, {pointer}->resume_error_{operation}, {pointer}->resume_error_length_{operation}{error_arg});"));
    let mut arguments = vec![
        "implementation".to_owned(),
        format!("{pointer}->op_{operation}.env"),
    ];
    arguments.extend(
        args.iter()
            .map(operand_expression)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|why| unsupported(&function.name, why))?,
    );
    arguments.push(format!(
        "({}){{emela_resume_value_{}, implementation->perform}}",
        layouts.c_type(resume_ty),
        resume_ty.index()
    ));
    if ft.fails {
        arguments.push("&implementation->perform->error".into());
    }
    writer.line(&format!(
        "if ({pointer}->op_{operation}.call == NULL) emela_scheduler_invalid();"
    ));
    writer.line("emela_fiber *saved_context = emela_current_fiber;");
    writer.line("emela_current_fiber = implementation;");
    writer.line(&format!(
        "{pointer}->op_{operation}.call({});",
        arguments.join(", ")
    ));
    writer.line("emela_current_fiber = saved_context;");
    writer.begin_block(
        "if (implementation->top == NULL && implementation->state == EMELA_FIBER_RUNNING)",
    );
    writer.line("implementation->state = EMELA_FIBER_DONE;");
    writer.line("emela_resume_after_step(implementation);");
    writer.end_block();
    writer.begin_block("else if (implementation->state == EMELA_FIBER_RUNNING)");
    writer.line("emela_ready_push(implementation);");
    writer.end_block();
    Ok(())
}

// The synchronous adapter shares the suspension emission context.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_sync_operation(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    handler: VarId,
    effect: EffectId,
    operation: usize,
    args: &[Operand],
    error_out: Option<&str>,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    writer.begin_block("");
    writer.line("EmelaFiber sync_handle = emela_fiber_create();");
    writer.line("emela_fiber *sync_caller = emela_fiber_get(sync_handle);");
    writer.line("if (emela_current_fiber != NULL) sync_caller->root = emela_current_fiber->root;");
    let result = dst
        .map(|id| format!("&{}", variable_name(id)))
        .unwrap_or_else(|| "NULL".to_owned());
    emit_operation(
        layouts,
        function,
        dst,
        handler,
        effect,
        operation,
        args,
        "sync_caller",
        &result,
        error_out,
        writer,
    )?;
    writer.begin_block("if (implementation->state == EMELA_FIBER_READY)");
    writer.line("emela_ready_remove(implementation); emela_resume_drive(implementation);");
    writer.end_block();
    writer.begin_block("if (sync_caller->state != EMELA_FIBER_READY)");
    writer.line("emela_board_defect(EMELA_DEFECT_PANIC, \"immediate operation did not resume\", UINT16_C(34));");
    writer.end_block();
    writer.line("emela_ready_remove(sync_caller); sync_caller->finishing = 1; sync_caller->state = EMELA_FIBER_WAITING;");
    writer.line("emela_unwind_sweep();");
    writer.line("if (sync_caller->state != EMELA_FIBER_DONE) emela_scheduler_invalid();");
    writer.line("emela_fiber_release(sync_handle);");
    writer.end_block();
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(in crate::compiler::emit) fn emit_sync_statement(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    statement: &Stmt,
    failing: bool,
    error: VarId,
    label: LabelId,
    index: usize,
    next: usize,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    strings: &StringBufferPlan,
    next_temp: &mut usize,
) -> Result<(), Vec<Error>> {
    let mut blocks = if failing {
        vec![BlockTarget {
            label,
            dst: Some(error),
            exit: format!("emela_call_failed_{index}"),
            used: false,
            protect_depth: 0,
        }]
    } else {
        vec![]
    };
    emit_statement(
        layouts,
        function,
        statement,
        writer,
        names,
        strings,
        &mut vec![],
        &mut blocks,
        &mut vec![],
        next_temp,
        &mut 0,
    )?;
    freeze_statement_result(layouts, function, statement, strings, writer);
    writer.line(&format!("frame->base.pc = {next}; continue;"));
    if failing {
        writer.line(&format!(
            "emela_call_failed_{index}: frame->base.pc = {next}; continue;"
        ));
    }
    Ok(())
}

pub(super) use crate::semantics::abi::scalar;
