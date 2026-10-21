use super::*;

// Heap containers own their complete payloads. Recursive boxes and List nodes establish this
// invariant at construction, so promotion only walks inline fields and retains heap edges.
// The generated call graph follows the acyclic inline layout, not the runtime value depth.
pub(super) fn emit_helpers(layouts: &TypeLayouts<'_>, writer: &mut Writer) {
    for (index, aggregate) in layouts.aggregates.iter().enumerate() {
        if !layouts.is_managed_value(aggregate.ty) {
            continue;
        }
        writer.begin_block(&format!(
            "static inline void emela_heap_own_value_{index}({} *value)",
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
        if layouts.has_inline_string(aggregate.ty) {
            writer.begin_block(&format!(
                "static inline void emela_heap_freeze_value_{index}({} *value)",
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

impl TypeLayouts<'_> {
    // The value already owns all existing references; only borrowed inline String views need
    // backing. This is different from acquiring a second owning copy at a call boundary.
    pub(super) fn emit_freeze_value_in_place(&self, ty: TypeId, value: &str, writer: &mut Writer) {
        if matches!(self.types[ty], Type::String) {
            writer.begin_block(&format!("if (({value}).owner == NULL)"));
            writer.assign(value, &format!("emela_heap_copy_string({value})"));
            writer.end_block();
        } else if self.has_inline_string(ty) {
            let index = self.aggregate_ids[&ty];
            writer.line(&format!("emela_heap_freeze_value_{index}(&({value}));"));
        }
    }

    pub(super) fn has_inline_string(&self, ty: TypeId) -> bool {
        if matches!(self.types[ty], Type::String) {
            return true;
        }
        let Some(&index) = self.aggregate_ids.get(&ty) else {
            return false;
        };
        let fields = match &self.aggregates[index].kind {
            AggregateKind::Tuple(fields) => vec![fields.as_slice()],
            AggregateKind::Enum(variants) => variants.iter().map(Vec::as_slice).collect(),
        };
        fields.iter().enumerate().any(|(variant, fields)| {
            fields.iter().enumerate().any(|(field, &field_ty)| {
                !self.is_recursive_box_field(ty, variant, field) && self.has_inline_string(field_ty)
            })
        })
    }

    // Acquire a new owning reference from a borrowed copy. The destination must be empty or
    // borrowed; replacing an already owned destination requires releasing its previous value.
    pub(super) fn emit_own_value_in_place(&self, ty: TypeId, value: &str, writer: &mut Writer) {
        if matches!(self.types[ty], Type::Any) && self.is_managed_value(ty) {
            writer.line(&format!("emela_any_own(&({value}));"));
        } else if matches!(self.types[ty], Type::String) {
            writer.assign(value, &format!("emela_heap_own_string({value})"));
        } else if let Some(index) = self.aggregate_helper_index(ty) {
            writer.line(&format!("emela_heap_own_value_{index}(&({value}));"));
        } else if self.is_managed_value(ty) {
            self.emit_retain_value_expression(ty, value, writer);
        }
    }

    pub(super) fn emit_owned_value_store(
        &self,
        ty: TypeId,
        source: &str,
        target: &str,
        writer: &mut Writer,
    ) {
        writer.assign(target, source);
        self.emit_own_value_in_place(ty, target, writer);
    }
}
