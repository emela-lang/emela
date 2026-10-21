//! Owned Cause records and typed Attempt boundaries.
use super::*;

pub(super) fn emit_types(layouts: &TypeLayouts<'_>, writer: &mut Writer) {
    if !layouts.has_causes {
        return;
    }
    writer.line("typedef struct { uint32_t kind;");
    if layouts.has_error_abi {
        writer.line("  EmelaError error;");
    }
    writer.line("} emela_cause;");
}

pub(super) fn emit_infallible_raise(layouts: &TypeLayouts<'_>, writer: &mut Writer) {
    if !layouts.has_cause_raise {
        return;
    }
    writer.begin_block("static inline void emela_infallible_cause_fail(EmelaError error)");
    writer.line("(void)error;");
    if layouts.has_scopes && layouts.has_error_abi {
        writer.begin_block("if (emela_current_fiber != NULL)");
        writer.begin_block("for (uint32_t slot = 0; slot < EMELA_FIBER_CAPACITY; slot++)");
        writer.line("emela_fiber *owner = &emela_fibers[slot];");
        writer.line("if (owner->state == EMELA_FIBER_FREE || !emela_same_fiber(owner->root, emela_current_fiber->root)) continue;");
        writer.begin_block("for (emela_frame *at = owner->top; at != NULL; at = at->parent)");
        writer.line("if (at->has_failure != emela_scope_has_failure || ((emela_scope_frame *)at)->scope->first_error.tag == 0) continue;");
        // The scope already owns this failure and its notes; do not report it a second time.
        writer.line("if (owner->failure_stop == NULL) owner->failure_stop = at;");
        writer.begin_block("for (uint32_t index = 0; index < EMELA_FIBER_CAPACITY; index++)");
        writer.line("emela_fiber *context = &emela_fibers[index];");
        writer.line("if (context->state != EMELA_FIBER_FREE && emela_same_fiber(context->root, owner->root)) context->forced = 1;");
        writer.end_block();
        writer.line("return;");
        writer.end_block();
        writer.end_block();
        writer.end_block();
    }
    writer.line("emela_board_defect(EMELA_DEFECT_ASSERT, \"assertion failed: empty Cause failed\", UINT16_C(34));");
    writer.end_block();
}

pub(super) fn emit_runtime(layouts: &TypeLayouts<'_>, writer: &mut Writer) {
    if !layouts.has_causes {
        return;
    }
    writer.begin_block("static inline emela_cause *emela_cause_checked(void *value)");
    writer.line(&format!("if (value == NULL || emela_heap_header(value)->kind != UINT32_C({})) emela_board_defect(EMELA_DEFECT_ASSERT, \"invalid Cause\", UINT16_C(13));", layouts.cause_kind()));
    writer.line("emela_cause *cause = (emela_cause *)value;");
    writer.line("if (cause->kind > 2) emela_board_defect(EMELA_DEFECT_ASSERT, \"invalid Cause\", UINT16_C(13));");
    writer.line("return cause;");
    writer.end_block();
    writer.begin_block("static inline void *emela_cause_interrupt(void)");
    writer.line(&format!(
        "emela_cause *cause = (emela_cause *)emela_heap_allocate(sizeof(*cause), UINT32_C({}));",
        layouts.cause_kind()
    ));
    writer.line(&format!(
        "cause->kind = UINT32_C({});",
        emela_core::CAUSE_INTERRUPT
    ));
    writer.line("return cause;");
    writer.end_block();
    if layouts.has_error_abi {
        writer.begin_block("static inline void *emela_cause_fail(EmelaError error)");
        writer.line(&format!("emela_cause *cause = (emela_cause *)emela_heap_allocate(sizeof(*cause), UINT32_C({}));", layouts.cause_kind()));
        writer.line(&format!(
            "cause->kind = UINT32_C({}); cause->error = error;",
            emela_core::CAUSE_FAIL
        ));
        writer.line("emela_error_retain(error);");
        writer.line("return cause;");
        writer.end_block();
    }
}

pub(super) fn emit_attempts(
    layouts: &TypeLayouts<'_>,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    for (index, &(body, exit)) in layouts.attempt_types.iter().enumerate() {
        let Type::Fn(ft) = &layouts.types[body] else {
            unreachable!()
        };
        let value = ft.ret;
        let represented = layouts.type_repr(value) != Repr::Void;
        let ct = layouts.c_type(value);
        let ex = layouts.c_type(exit);
        let failure = layouts
            .enum_fields(exit)
            .and_then(|v| v.get(emela_core::EXIT_FAILURE));
        if !failure.is_some_and(|fields| fields.len() == 1 && layouts.is_cause(fields[0])) {
            return Err(invalid_flow_ir("Attempt", "Failure payload is not Cause"));
        }
        if ft.suspends {
            writer.line(&format!(
                "typedef struct {{ emela_frame base; {ex} *result; {} body;",
                layouts.c_type(body)
            ));
            if represented {
                writer.line(&format!("  {ct} value;"));
            }
            if ft.fails {
                writer.line("  EmelaError error;");
            }
            writer.line(&format!("}} emela_attempt_frame_{index};"));
            writer.line(&format!("_Static_assert(sizeof(emela_attempt_frame_{index}) <= EMELA_FRAME_BYTES, \"increase frame_bytes for Attempt\");"));
            if ft.fails {
                writer.begin_block(&format!("static int emela_attempt_has_failure_{index}(emela_fiber *fiber, emela_frame *base)"));
                writer.line("(void)fiber;");
                writer.line(&format!(
                    "return ((emela_attempt_frame_{index} *)base)->error.tag != 0;"
                ));
                writer.end_block();
            }
            writer.begin_block(&format!(
                "static void emela_attempt_drop_{index}(emela_frame *base)"
            ));
            writer.line(&format!(
                "emela_attempt_frame_{index} *frame = (emela_attempt_frame_{index} *)base;"
            ));
            layouts.emit_release_value_expression(body, "frame->body", writer);
            if represented {
                layouts.emit_release_value_expression(value, "frame->value", writer);
            }
            if ft.fails {
                writer.line("emela_error_clear(&frame->error);");
            }
            writer.end_block();
            writer.begin_block(&format!(
                "static void emela_attempt_step_{index}(emela_fiber *fiber, emela_frame *base)"
            ));
            writer.line(&format!(
                "emela_attempt_frame_{index} *frame = (emela_attempt_frame_{index} *)base;"
            ));
            writer.begin_block("if (base->pc == 0)");
            writer.line("base->pc = 1;");
            let mut args: Vec<String> = vec!["fiber".into(), "frame->body.env".into()];
            if ft.fails {
                args.push("&frame->error".into());
            }
            if represented {
                args.push("&frame->value".into());
            }
            writer.line(&format!("frame->body.call({}); return;", args.join(", ")));
            writer.end_block();
            writer.begin_block("if (frame->result != NULL)");
            if ft.fails {
                writer.begin_block("if (frame->error.tag != 0)");
                writer.line(&format!("*frame->result = ({ex}){{.tag = {}, .payload.v{}.f0 = emela_cause_fail(frame->error)}};", emela_core::EXIT_FAILURE,emela_core::EXIT_FAILURE));
                writer.end_block();
                writer.begin_block("else");
            }
            if represented {
                suspend::retain_frame_value(layouts, value, "frame->value", writer);
            }
            emit_success(
                writer,
                &ex,
                "*frame->result",
                represented.then_some("frame->value"),
            );
            if ft.fails {
                writer.end_block();
            }
            writer.end_block();
            writer.line("emela_frame_return(fiber);");
            writer.end_block();
            writer.begin_block(&format!("static inline void emela_attempt_{index}(emela_fiber *fiber, {} body, {ex} *result)", layouts.c_type(body)));
            writer.line("if (body.call == NULL) emela_scheduler_invalid();");
            writer.line(&format!("emela_attempt_frame_{index} *frame = (emela_attempt_frame_{index} *)emela_frame_push(fiber, sizeof(*frame), emela_attempt_step_{index});"));
            writer.line("for (size_t i = sizeof(emela_frame); i < sizeof(*frame); i++) ((unsigned char *)frame)[i] = 0;");
            writer.line(&format!("frame->base.drop = emela_attempt_drop_{index}; frame->body = body; frame->result = result;"));
            if ft.fails {
                writer.line(&format!(
                    "frame->base.has_failure = emela_attempt_has_failure_{index};"
                ));
            }
            layouts.emit_retain_value_expression(body, "body", writer);
            writer.end_block();
        } else {
            writer.begin_block(&format!(
                "static inline {ex} emela_attempt_{index}({} body)",
                layouts.c_type(body)
            ));
            writer.line("if (body.call == NULL) emela_board_defect(EMELA_DEFECT_PANIC, \"null function value\", UINT16_C(19));");
            if ft.fails {
                writer.line("EmelaError error = {0};");
            }
            let call = format!(
                "body.call(body.env{})",
                if ft.fails { ", &error" } else { "" }
            );
            if represented {
                writer.line(&format!("{ct} value = {call};"));
            } else {
                writer.line(&format!("{call};"));
            }
            if ft.fails {
                writer.begin_block("if (error.tag != 0)");
                writer.line("void *cause = emela_cause_fail(error); emela_error_clear(&error);");
                writer.line(&format!(
                    "return ({ex}){{.tag = {}, .payload.v{}.f0 = cause}};",
                    emela_core::EXIT_FAILURE,
                    emela_core::EXIT_FAILURE
                ));
                writer.end_block();
            }
            writer.line(&format!("{ex} result;"));
            emit_success(writer, &ex, "result", represented.then_some("value"));
            writer.line("return result;");
            writer.end_block();
        }
    }
    Ok(())
}

fn emit_success(writer: &mut Writer, ex: &str, result: &str, value: Option<&str>) {
    let field = value.map_or_else(|| "_unit = 0".into(), |v| format!("f0 = {v}"));
    writer.line(&format!(
        "{result} = ({ex}){{.tag = {}, .payload.v{}.{field}}};",
        emela_core::EXIT_SUCCESS,
        emela_core::EXIT_SUCCESS
    ));
}

pub(super) fn emit_attempt_call(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    args: &[Operand],
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let [Operand::Var(body)] = args else {
        return Err(invalid_flow_ir(&function.name, "Attempt body"));
    };
    let ty = function.vars[*body].ty;
    let (index, _) = layouts
        .attempt_types
        .iter()
        .enumerate()
        .find(|(_, (b, ex))| *b == ty && dst.is_none_or(|d| function.vars[d].ty == *ex))
        .ok_or_else(|| invalid_flow_ir(&function.name, "Attempt layout"))?;
    let Type::Fn(ft) = &layouts.types[ty] else {
        unreachable!()
    };
    let body = variable_name(*body);
    if ft.suspends {
        writer.line(&format!(
            "emela_attempt_{index}(fiber, {body}, {});",
            dst.map_or_else(|| "NULL".into(), |d| format!("&{}", variable_name(d)))
        ));
    } else if let Some(dst) = dst {
        writer.assign(
            &variable_name(dst),
            &format!("emela_attempt_{index}({body})"),
        );
    } else {
        writer.begin_block("");
        let ex = layouts.attempt_types[index].1;
        writer.line(&format!(
            "{} discarded_attempt = emela_attempt_{index}({body});",
            layouts.c_type(ex)
        ));
        layouts.emit_release_value_expression(ex, "discarded_attempt", writer);
        writer.end_block();
    }
    Ok(())
}

pub(super) fn emit_kind(
    function: &Function,
    dst: Option<VarId>,
    args: &[Operand],
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let [cause] = args else {
        return Err(invalid_flow_ir(&function.name, "Cause.kind arity"));
    };
    let cause = operand_expression(cause).map_err(|why| unsupported(&function.name, why))?;
    let value = format!("(int32_t)emela_cause_checked({cause})->kind");
    if let Some(dst) = dst {
        writer.assign(&variable_name(dst), &value);
    } else {
        writer.line(&format!("(void)({value});"));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_to_string(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    args: &[Operand],
    _names: &HashMap<FnId, String>,
    strings: &StringBufferPlan,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let [cause] = args else {
        return Err(invalid_flow_ir(&function.name, "Cause.to_string arity"));
    };
    let cause = operand_expression(cause).map_err(|why| unsupported(&function.name, why))?;
    writer.begin_block("");
    let (buffer, capacity) = if let Some(dst) = dst {
        strings.buffer(dst).ok_or_else(|| {
            invalid_flow_ir(&function.name, "Cause.to_string has no planned buffer")
        })?
    } else {
        writer.line(&format!("char discarded_cause_text[{}];", strings.capacity));
        (
            "discarded_cause_text".into(),
            format!("UINT16_C({})", strings.capacity),
        )
    };
    writer.line(&format!(
        "emela_cause *cause = emela_cause_checked({cause});"
    ));
    writer.line("EmelaString text = {0};");
    writer.line(&format!(
        "char *output = {buffer}; uint16_t capacity = {capacity};"
    ));
    writer.begin_block("if (cause->kind == 2)");
    writer.line("text = (EmelaString){.data = \"interrupted\", .length = 11};");
    writer.end_block();
    if layouts.has_error_abi {
        writer.begin_block("else if (cause->kind == 0)");
        writer.line("text = emela_error_to_string(cause->error, output, capacity);");
        writer.end_block();
    }
    writer.begin_block("else");
    writer.line("emela_board_defect(EMELA_DEFECT_ASSERT, \"invalid Cause\", UINT16_C(13));");
    writer.end_block();
    writer.line("if (text.length > capacity) emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"string capacity exceeded\", UINT16_C(24));");
    writer.line("if (text.data != output) for (uint32_t i = 0; i < text.length; i++) output[i] = text.data[i];");
    if let Some(dst) = dst {
        writer.assign(
            &variable_name(dst),
            "(EmelaString){.data = output, .length = text.length}",
        );
    }
    writer.end_block();
    Ok(())
}
