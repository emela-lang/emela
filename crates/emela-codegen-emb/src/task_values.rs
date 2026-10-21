//! Typed completion producers for external tasks with non-scalar success values.
use super::*;

pub(super) fn result_types(flow: &emela_lir::Module, emitted: &HashSet<FnId>) -> Vec<TypeId> {
    let mut types: Vec<_> = flow
        .functions
        .iter()
        .filter_map(|(id, f)| {
            (emitted.contains(&id)
                && f.suspends
                && f.external.is_some()
                && !suspend::scalar(&flow.core.types, f.ret))
            .then_some(f.ret)
        })
        .collect();
    types.sort_by_key(|ty| ty.index());
    types.dedup();
    types
}

pub(super) fn validate_types(
    flow: &emela_lir::Module,
    emitted: &HashSet<FnId>,
    layouts: &TypeLayouts<'_>,
) -> Result<(), Vec<Error>> {
    for (id, function) in flow.functions.iter() {
        if !emitted.contains(&id) || !function.suspends || function.external.is_none() {
            continue;
        }
        for ty in std::iter::once(function.ret)
            .chain(function.params.iter().map(|&p| function.vars[p].ty))
        {
            if !boundary_type(layouts, ty, &mut HashSet::new()) {
                return Err(unsupported(
                    &function.name,
                    "external task signature containing a callback or runtime capability",
                ));
            }
        }
    }
    Ok(())
}

fn boundary_type(layouts: &TypeLayouts<'_>, ty: TypeId, seen: &mut HashSet<TypeId>) -> bool {
    if !seen.insert(ty) {
        return true;
    }
    match &layouts.types[ty] {
        Type::Int
        | Type::Int64
        | Type::Float
        | Type::Bool
        | Type::Unit
        | Type::Never
        | Type::String
        | Type::Any => true,
        Type::List(element) => boundary_type(layouts, *element, seen),
        Type::Tuple(_) | Type::Enum(_, _) => {
            if layouts.is_runtime_handle(ty) {
                return false;
            }
            if let Type::Enum(id, _) = layouts.types[ty]
                && (layouts.core.enums[id].external || layouts.enum_is_i32(id))
            {
                return true;
            }
            let Some(&index) = layouts.aggregate_ids.get(&ty) else {
                return false;
            };
            let fields: Vec<_> = match &layouts.aggregates[index].kind {
                AggregateKind::Tuple(fields) => vec![fields.as_slice()],
                AggregateKind::Enum(variants) => variants.iter().map(Vec::as_slice).collect(),
            };
            fields
                .into_iter()
                .flatten()
                .all(|&field| boundary_type(layouts, field, seen))
        }
        _ => false,
    }
}

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
