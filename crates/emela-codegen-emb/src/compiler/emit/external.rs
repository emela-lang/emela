use super::*;
use crate::ExternalResult;
use crate::contract::bindings::{ExternalBindingPlan as Binding, ExternalBindings as Bindings};
fn boundary_types(flow: &crate::contract::ir::Program, bindings: &Bindings) -> Vec<TypeId> {
    let mut types: Vec<_> = bindings
        .iter()
        .filter(|(_, b)| b.adapted)
        .filter(|(id, _)| flow.functions[**id].derivation.is_none())
        .filter(|(_, binding)| !binding.target_owned)
        .flat_map(|(&id, _)| {
            let f = &flow.functions[id];
            f.params
                .iter()
                .map(|&p| f.vars[p].ty)
                .chain(std::iter::once(f.ret))
        })
        .collect();
    types.sort_by_key(|ty| ty.index());
    types.dedup();
    types
}

pub(in crate::compiler::emit) fn emit_header(
    flow: &crate::contract::ir::Program,
    bindings: &Bindings,
    layouts: &TypeLayouts<'_>,
    heap: bool,
    writer: &mut Writer,
) {
    let mut ids: Vec<_> = bindings.keys().copied().collect();
    ids.sort_by_key(|id| u32::from(id.into_raw()));
    for id in ids {
        let b = &bindings[&id];
        if !b.adapted || flow.functions[id].derivation.is_some() || b.target_owned {
            continue;
        }
        writer.line(
            "/* Synchronous generated-value adapter. Arguments are borrowed during this call.",
        );
        writer.line(
            " * No scheduler/start/cancel reentry or automatic conversion to native C callbacks.",
        );
        match b . result { ExternalResult :: Borrowed => writer . line (" * Result remains valid until the generated wrapper finishes acquiring an owning copy; never return callee-stack bytes.") , ExternalResult :: Owned => writer . line (" * Result transfers all owned references from this module's fixed heap; never malloc or callee-stack owners.") , ExternalResult :: StringBuffer => writer . line (" * Check capacity before writing. Return owner=NULL, data=emela_string_output, length<=capacity.") , }
        writer.line(" */");
        if flow.functions[id].fails {
            writer . line ("/* Non-NULL error output starts zero. On failure set a module error tag; the initialized C return value is ignored, including its managed fields.") ;
            writer . line (" * Owned transfers the error; Borrowed/StringBuffer keep its payload valid until the wrapper snapshots it. Success leaves error output zero. */") ;
        }
        writer.line(&format!(
            "{};",
            signature(
                layouts,
                &flow.functions[id],
                &b.symbol,
                false,
                flow.functions[id].fails,
                b.result == ExternalResult::StringBuffer
            )
        ));
    }
    if heap {
        for ty in boundary_types(flow, bindings) {
            if !layouts.is_managed_value(ty) {
                continue;
            }
            writer . line ("/* Acquire a complete owning copy in a borrowed record, or release and zero an owned record. */") ;
            writer.line(&format!(
                "void emela_external_own_{}({} *value);",
                ty.index(),
                layouts.c_type(ty)
            ));
            writer.line(&format!(
                "void emela_external_release_{}({} *value);",
                ty.index(),
                layouts.c_type(ty)
            ));
        }
        if bindings
            .iter()
            .any(|(&id, b)| b.adapted && flow.functions[id].fails)
        {
            writer.line("/* Acquire a borrowed failure, or release and zero an owned failure. */");
            writer.line("void emela_external_own_error(EmelaError *error);");
            writer.line("void emela_external_release_error(EmelaError *error);");
        }
    }
}

pub(in crate::compiler::emit) fn emit_ownership_helpers(
    flow: &crate::contract::ir::Program,
    bindings: &Bindings,
    layouts: &TypeLayouts<'_>,
    writer: &mut Writer,
) {
    for ty in boundary_types(flow, bindings) {
        if !layouts.is_managed_value(ty) {
            continue;
        }
        writer.begin_block(&format!(
            "void emela_external_own_{}({} *value)",
            ty.index(),
            layouts.c_type(ty)
        ));
        writer.begin_block("if (value != NULL)");
        layouts.emit_own_value_in_place(ty, "*value", writer);
        writer.end_block();
        writer.end_block();
        writer.begin_block(&format!(
            "void emela_external_release_{}({} *value)",
            ty.index(),
            layouts.c_type(ty)
        ));
        writer.begin_block("if (value != NULL)");
        layouts.emit_release_value_expression(ty, "*value", writer);
        writer.line(&format!("*value = ({}){{0}};", layouts.c_type(ty)));
        writer.end_block();
        writer.end_block();
    }
    if bindings
        .iter()
        .any(|(&id, b)| b.adapted && flow.functions[id].fails)
    {
        writer.begin_block("void emela_external_own_error(EmelaError *error)");
        writer.line("if (error != NULL) emela_error_own(error);");
        writer.end_block();
        writer.begin_block("void emela_external_release_error(EmelaError *error)");
        writer.line("if (error != NULL) emela_error_clear(error);");
        writer.end_block();
    }
}

fn emit_failed_result(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    binding: &Binding,
    writer: &mut Writer,
) {
    if !function.fails {
        return;
    }
    writer.begin_block("if (emela_error_out->tag != 0)");
    writer.begin_block("switch (emela_error_out->tag)");
    for &(id, ty) in &layouts.runtime.error_types {
        writer.line(&format!("case UINT32_C({}):", enum_index(id) + 1));
        let payload = format!("emela_error_out->payload.e{}", enum_index(id));
        let variant = if layouts.shapes.aggregate_ids.contains_key(&ty) {
            format!("{payload}.tag")
        } else {
            payload
        };
        writer . line (& format ! ("if ((uint32_t)({variant}) >= UINT32_C({})) emela_board_defect(EMELA_DEFECT_ASSERT, \"invalid external error\", UINT16_C(22));" , layouts . shapes . core . enums [id] . variants . len ())) ;
        writer.line("break;");
    }
    writer . line ("default: emela_board_defect(EMELA_DEFECT_ASSERT, \"invalid external error\", UINT16_C(22));") ;
    writer.end_block();
    if binding.result != ExternalResult::Owned {
        writer.line("emela_error_own(emela_error_out);");
    }
    if layouts.type_repr(function.ret) == Repr::Void {
        writer.ret(None);
    } else {
        writer.ret(Some(&format!("({}){{0}}", layouts.c_type(function.ret))));
    }
    writer.end_block();
}

pub(in crate::compiler::emit) fn emit_adapter(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    binding: &Binding,
    name: &str,
    writer: &mut Writer,
) {
    if !binding.adapted {
        return;
    }
    let output = binding.result == ExternalResult::StringBuffer;
    writer.begin_block(&signature(
        layouts,
        function,
        name,
        true,
        function.fails,
        output,
    ));
    let mut args: Vec<_> = function.params.iter().map(|&p| variable_name(p)).collect();
    if function.fails {
        args.push("emela_error_out".into());
        writer.line("*emela_error_out = (EmelaError){0};");
    }
    if output {
        args.extend(["emela_string_output".into(), "emela_string_capacity".into()]);
    }
    let call = format!("{}({})", binding.symbol, args.join(", "));
    if layouts.type_repr(function.ret) == Repr::Void {
        writer.line(&format!("{call};"));
        emit_failed_result(layouts, function, binding, writer);
        writer.end_block();
        return;
    }
    writer.line(&format!(
        "{} returned = {call};",
        layouts.c_type(function.ret)
    ));
    emit_failed_result(layouts, function, binding, writer);
    if binding.result == ExternalResult::Borrowed {
        layouts.emit_own_value_in_place(function.ret, "returned", writer);
    }
    if output {
        writer . begin_block ("if (returned.owner != NULL || returned.length > emela_string_capacity || (returned.length != 0 && (returned.data != emela_string_output || emela_string_output == NULL)))") ;
        writer . line ("emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"invalid external String buffer\", UINT16_C(30));") ;
        writer.end_block();
    }
    writer.ret(Some("returned"));
    writer.end_block();
    writer.blank();
}
