//! Typed completion producers for external tasks with non-scalar success values.
use super::*;

pub(super) fn emit_header(layouts: &TypeLayouts<'_>, types: &[TypeId], writer: &mut Writer) {
    if types.is_empty() {
        return;
    }
    writer.line(
        "/* ISR-safe typed notifications queue only a producer/context lease. On acceptance, main",
    );
    writer.line(
        " * calls the producer with a zero-initialized output, acquires a complete owning copy,",
    );
    writer.line(
        " * and releases the context exactly once (if non-NULL), including abandoned events.",
    );
    writer.line(
        " * Producer output is borrowed until release; all referenced bytes must remain pinned.",
    );
    writer.line(
        " * Return zero for NULL producers, stale/duplicate handles, or mismatched result kinds;",
    );
    writer
        .line(" * zero takes no lease. Full queues defect after unlocking without taking a lease.");
    writer.line(
        " * Cancellation skips the producer; main must keep pumping to release queued contexts.",
    );
    writer
        .line(" * Values must follow the generated C layout and Emela heap ownership invariants.");
    writer.line(" */");
    for &ty in types {
        writer.line(&format!(
            "typedef void (*EmelaTaskValueProducer_{})(void *context, {} *value);",
            ty.index(),
            layouts.c_type(ty)
        ));
        writer.line(&format!("int emela_task_complete_value_{}(EmelaTask task, EmelaTaskValueProducer_{} producer, void *context, EmelaTaskContextRelease release);", ty.index(), ty.index()));
    }
}

pub(super) fn emit_union(types: &[TypeId], writer: &mut Writer) {
    writer.line("#define EMELA_TASK_VALUE_RUNTIME 1");
    for (index, ty) in types.iter().enumerate() {
        writer.line(&format!(
            "#define EMELA_TASK_VALUE_{} UINT32_C({})",
            ty.index(),
            index + 4
        ));
    }
    writer.line("typedef union {");
    for ty in types {
        writer.line(&format!(
            "  EmelaTaskValueProducer_{} t{};",
            ty.index(),
            ty.index()
        ));
    }
    writer.line("} emela_task_value_producer;");
}

pub(super) fn emit_dispatch(layouts: &TypeLayouts<'_>, types: &[TypeId], writer: &mut Writer) {
    if types.is_empty() {
        return;
    }
    writer.begin_block("static void emela_task_deliver_value(uint32_t kind, emela_task_value_producer producer, void *context, EmelaTaskContextRelease release, void *result)");
    writer.begin_block("switch (kind)");
    for &ty in types {
        let id = ty.index();
        let c_type = layouts.c_type(ty);
        writer.line(&format!("case EMELA_TASK_VALUE_{id}:"));
        writer.begin_block("");
        writer.line(&format!(
            "{c_type} value = {{0}}; producer.t{id}(context, &value);"
        ));
        layouts.emit_own_value_in_place(ty, "value", writer);
        writer.line("if (release != NULL) release(context);");
        writer.begin_block("if (result != NULL)");
        writer.line(&format!("*(({c_type} *)result) = value;"));
        writer.end_block();
        writer.begin_block("else");
        layouts.emit_release_value_expression(ty, "value", writer);
        writer.end_block();
        writer.line("return;");
        writer.end_block();
    }
    writer.line("default: emela_scheduler_invalid();");
    writer.end_block();
    writer.end_block();
    for &ty in types {
        let id = ty.index();
        writer.begin_block(&format!("int emela_task_complete_value_{id}(EmelaTask task, EmelaTaskValueProducer_{id} producer, void *context, EmelaTaskContextRelease release)"));
        writer.line("if (producer == NULL) return 0;");
        writer.line(&format!("return emela_task_complete_value(task, EMELA_TASK_VALUE_{id}, (emela_task_value_producer){{.t{id} = producer}}, context, release);"));
        writer.end_block();
    }
}
