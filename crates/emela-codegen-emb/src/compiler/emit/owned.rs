use super::*;

pub(super) fn emit_helpers(layouts: &TypeLayouts<'_>, writer: &mut Writer) {
    for (index, aggregate) in layouts.shapes.aggregates.iter().enumerate() {
        if !layouts.is_managed_value(aggregate.ty) {
            continue;
        }
        writer.begin_block(&format!(
            "static inline EMELA_UNUSED void emela_heap_own_value_{index}({} *value)",
            layouts.c_type(aggregate.ty)
        ));
        match &aggregate.kind {
            AggregateKind::Tuple(fields) => {
                emit_fields(layouts, aggregate.ty, 0, fields, "value->", writer);
            }
            AggregateKind::Enum(variants) => {
                writer.begin_block("switch (value->tag)");
                for (variant, fields) in variants.iter().enumerate() {
                    writer.line(&format!("case INT32_C({variant}):"));
                    writer.begin_block("");
                    emit_fields(
                        layouts,
                        aggregate.ty,
                        variant,
                        fields,
                        &format!("value->payload.v{variant}."),
                        writer,
                    );
                    writer.line("break;");
                    writer.end_block();
                }
                writer.line("default: break;");
                writer.end_block();
            }
        }
        writer.end_block();
        writer.blank();
        if layouts.has_borrowed_views(aggregate.ty) {
            writer.begin_block(&format!(
                "static inline EMELA_UNUSED void emela_heap_freeze_value_{index}({} *value)",
                layouts.c_type(aggregate.ty)
            ));
            match &aggregate.kind {
                AggregateKind::Tuple(fields) => {
                    emit_freeze_fields(layouts, aggregate.ty, 0, fields, "value->", writer);
                }
                AggregateKind::Enum(variants) => {
                    writer.begin_block("switch (value->tag)");
                    for (variant, fields) in variants.iter().enumerate() {
                        writer.line(&format!("case INT32_C({variant}):"));
                        writer.begin_block("");
                        emit_freeze_fields(
                            layouts,
                            aggregate.ty,
                            variant,
                            fields,
                            &format!("value->payload.v{variant}."),
                            writer,
                        );
                        writer.line("break;");
                        writer.end_block();
                    }
                    writer.line("default: break;");
                    writer.end_block();
                }
            }
            writer.end_block();
            writer.blank();
        }
    }
}

fn emit_freeze_fields(
    layouts: &TypeLayouts<'_>,
    owner: TypeId,
    variant: usize,
    fields: &[TypeId],
    prefix: &str,
    writer: &mut Writer,
) {
    for (index, &ty) in fields.iter().enumerate() {
        if !layouts.is_recursive_box_field(owner, variant, index) {
            layouts.emit_freeze_value_in_place(ty, &format!("{prefix}f{index}"), writer);
        }
    }
}

fn emit_fields(
    layouts: &TypeLayouts<'_>,
    owner: TypeId,
    variant: usize,
    fields: &[TypeId],
    prefix: &str,
    writer: &mut Writer,
) {
    for (index, &ty) in fields.iter().enumerate() {
        let field = format!("{prefix}f{index}");
        if layouts.is_recursive_box_field(owner, variant, index) {
            writer.line(&format!("emela_heap_retain({field});"));
        } else {
            layouts.emit_own_value_in_place(ty, &field, writer);
        }
    }
}
