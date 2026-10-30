//! Function signatures, ownership helpers, closures, and public wrappers.
use super::{
    StringBufferPlan, TypeLayouts,
    c::{c_string_literal, function_coerce_adapter_name, owned_variable_name, variable_name},
};
use crate::contract::ir::{FnId, Function, Operand, Repr, VarId};
use crate::contract::ir::{Type, TypeId};
use crate::semantics::abi::LayoutRead;
use crate::writer::Writer;

pub(in crate::compiler::emit) fn emit_local_declarations(
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

pub(in crate::compiler::emit) fn emit_closure_environment_declarations(
    layouts: &TypeLayouts<'_>,
    closures: &crate::contract::lowered::ClosureMetadata,
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

pub(in crate::compiler::emit) fn emit_closure_string_copy_helper(
    layouts: &TypeLayouts<'_>,
    writer: &mut Writer,
) {
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

pub(in crate::compiler::emit) fn emit_closure_capture_loads(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    environment: usize,
    writer: &mut Writer,
) {
    let captures = &layouts.functions.closure_environments[environment].captures;
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

pub(in crate::compiler::emit) fn emit_release_if_owned(
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

pub(in crate::compiler::emit) fn emit_retain_operand(
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

pub(in crate::compiler::emit) fn emit_owned_cleanup(
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
    if layouts.runtime.has_cause_raise
        && layouts.runtime.has_unwind
        && !function.suspends
        && !function.constant
    {
        writer.line(
            "if (emela_current_fiber != NULL) emela_current_fiber->holds = emela_entry_holds;",
        );
    }
}

pub(in crate::compiler::emit) fn emit_const_getter(
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
    let is_void = matches!(layouts.shapes.types[function.ret], Type::Unit | Type::Never);
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
        if layouts.has_borrowed_views(function.ret) {
            layouts.emit_freeze_value_in_place(function.ret, &cache, writer);
        }
    }
    writer.line("emela_const_current = context.previous;");
    writer.line(&format!("{state}.status = 2;"));
    writer.end_block();
    if is_void {
        if matches!(layouts.shapes.types[function.ret], Type::Never) {
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

pub(in crate::compiler::emit) fn emit_any_ownership_declarations(
    layouts: &TypeLayouts<'_>,
    writer: &mut Writer,
) {
    if !layouts.runtime.any_abi {
        return;
    }
    writer.line("static inline EMELA_UNUSED void emela_any_retain(EmelaAny value);");
    writer.line("static inline EMELA_UNUSED void emela_any_own(EmelaAny *value);");
    writer.line("static inline EMELA_UNUSED void emela_any_enqueue(EmelaAny value, emela_heap_block **pending, emela_heap_block **tail);");
    writer.line("static inline EMELA_UNUSED void emela_any_release(EmelaAny value);");
}

pub(in crate::compiler::emit) fn emit_any_ownership(
    layouts: &TypeLayouts<'_>,
    heap: bool,
    writer: &mut Writer,
) {
    if !layouts.runtime.any_abi {
        return;
    }
    writer.begin_block("static inline EMELA_UNUSED void emela_any_retain(EmelaAny value)");
    writer.line("(void)value;");
    if heap && layouts.runtime.any_box_required {
        writer.line("if (value.tag == 0) emela_heap_retain(value.payload.any_box);");
    }
    if layouts.has_error_ownership() {
        writer.line("if (value.tag != 0) emela_error_retain(value);");
    }
    writer.end_block();
    writer.begin_block("static inline EMELA_UNUSED void emela_any_own(EmelaAny *value)");
    writer.line("(void)value;");
    if heap && layouts.runtime.any_box_required {
        writer.line("if (value->tag == 0) emela_heap_retain(value->payload.any_box);");
    }
    if layouts.has_error_ownership() {
        writer.line("if (value->tag != 0) emela_error_own(value);");
    }
    writer.end_block();
    if heap {
        writer.begin_block("static inline EMELA_UNUSED void emela_any_enqueue(EmelaAny value, emela_heap_block **pending, emela_heap_block **tail)");
        writer.line("(void)value; (void)pending; (void)tail;");
        if layouts.runtime.any_box_required {
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

pub(in crate::compiler::emit) fn emit_public_string_ownership(writer: &mut Writer) {
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

pub(in crate::compiler::emit) fn signature(
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

pub(in crate::compiler::emit) fn closure_signature(
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

pub(in crate::compiler::emit) fn function_ref_adapter_signature(
    layouts: &TypeLayouts<'_>,
    function_type: TypeId,
    name: &str,
) -> String {
    let Type::Fn(function_type) = layouts.shapes.types[function_type].clone() else {
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

pub(in crate::compiler::emit) fn emit_function_ref_adapter(
    layouts: &TypeLayouts<'_>,
    function_type: TypeId,
    target_name: &str,
    adapter_name: &str,
    takes_string_output: bool,
    string_capacity: u16,
    writer: &mut Writer,
) {
    let Type::Fn(function_type_data) = layouts.shapes.types[function_type].clone() else {
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

pub(in crate::compiler::emit) fn emit_function_coerce_adapter(
    layouts: &TypeLayouts<'_>,
    adapter: &FunctionAdapterLayout,
    index: usize,
    writer: &mut Writer,
) {
    let Type::Fn(source_type) = layouts.shapes.types[adapter.source].clone() else {
        unreachable!("function adapter source has non-function type")
    };
    let Type::Fn(destination_type) = layouts.shapes.types[adapter.destination].clone() else {
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

pub(in crate::compiler::emit) fn emit_fallible_wrapper(
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

use super::layouts::LayoutEmission;

use crate::contract::abi::FunctionAdapterLayout;
