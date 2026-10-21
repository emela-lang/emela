//! Explicit synchronous generated-value board adapters.
use super::*;
use crate::{ExternalAdapter, ExternalResult};

#[derive(Clone)]
pub(super) struct Binding {
    pub(super) symbol: String,
    pub(super) result: ExternalResult,
    pub(super) adapted: bool,
}

pub(super) type Bindings = HashMap<FnId, Binding>;

// Keep public entry ABI and external binding ABI independent, including suspend task entries.
pub(super) fn normalize_exports(module: &mut emela_lir::Module) {
    let ids: Vec<_> = module
        .functions
        .iter()
        .filter_map(|(id, f)| (f.exported && f.external.is_some()).then_some(id))
        .collect();
    for id in ids {
        let original = module.functions[id].clone();
        let mut target = original.clone();
        target.exported = false;
        target.name = format!("{}$external_{}", original.name, u32::from(id.into_raw()));
        let target = module.functions.alloc(target);
        let mut b = emela_lir::FnBuilder::new(&module.core, &original.name);
        b.set_exported().set_ret(original.ret);
        if original.fails {
            b.set_fails();
        }
        if original.suspends {
            b.set_suspends();
        }
        let args = original
            .params
            .iter()
            .map(|&p| b.param(&original.vars[p].hint, original.vars[p].ty).into())
            .collect();
        let value = Value::Call {
            callee: Callee::Direct(target),
            args,
            suspends: original.suspends,
            on_fail: original.fails.then_some(OnFail::Propagate),
        };
        if matches!(module.core.types[original.ret], Type::Unit | Type::Never) {
            b.let_(None, value);
            b.term(Term::Return(None));
        } else {
            let result = b.assign("result", original.ret, value);
            b.term(Term::Return(Some(result.into())));
        }
        module.functions[id] = b.finish();
    }
}

fn symbol(function: &Function) -> Result<String, Vec<Error>> {
    let binding = function
        .external
        .as_ref()
        .and_then(|e| e.binding("c"))
        .ok_or_else(|| unsupported(&function.name, "external function without a C binding"))?;
    let symbol = match &binding.host {
        emela_core::HostRef::Global(s) | emela_core::HostRef::Import { name: s, .. } => s.clone(),
    };
    if !is_c_identifier(&symbol) {
        return Err(unsupported(
            &function.name,
            "external function with an invalid C symbol",
        ));
    }
    Ok(symbol)
}

pub(super) fn resolve(
    flow: &emela_lir::Module,
    emitted: &HashSet<FnId>,
    layouts: &mut TypeLayouts<'_>,
    options: &Options,
) -> Result<Bindings, Vec<Error>> {
    let mut bindings = Bindings::new();
    let mut declarations: HashMap<String, (Vec<String>, ExternalResult)> = HashMap::new();
    let generated: HashSet<_> = flow
        .functions
        .iter()
        .filter(|(id, _)| emitted.contains(id))
        .map(|(id, f)| function_name(id, f))
        .collect();
    for (id, f) in flow
        .functions
        .iter()
        .filter(|(id, f)| emitted.contains(id) && f.external.is_some() && !f.suspends)
    {
        if peripheral::resolve(flow, f)?.is_some() {
            bindings.insert(
                id,
                Binding {
                    symbol: function_name(id, f),
                    result: ExternalResult::Owned,
                    adapted: true,
                },
            );
            continue;
        }
        if let Some(kind) = crate::derived::kind(f) {
            bindings.insert(
                id,
                Binding {
                    symbol: function_name(id, f),
                    result: if kind == crate::DerivedOperation::Show {
                        ExternalResult::StringBuffer
                    } else {
                        ExternalResult::Owned
                    },
                    adapted: true,
                },
            );
            continue;
        }
        if crate::fiber::is_infallible_cause_fail(f) {
            bindings.insert(
                id,
                Binding {
                    symbol: validate_external(flow, f)?,
                    result: ExternalResult::Owned,
                    adapted: false,
                },
            );
            continue;
        }
        let original = symbol(f)?;
        let adapter = options
            .external_adapters
            .iter()
            .find(|(name, _)| *name == original)
            .map(|(_, a)| a);
        let binding = if let Some(ExternalAdapter { symbol, result }) = adapter {
            if !is_c_identifier(symbol)
                || generated.contains(symbol)
                || symbol.starts_with("emela_")
            {
                return Err(unsupported(
                    &f.name,
                    "external adapter symbol conflicts with a generated declaration or is invalid",
                ));
            }
            if *result == ExternalResult::StringBuffer && f.ret != TypeId::STRING {
                return Err(unsupported(
                    &f.name,
                    "StringBuffer external adapter requires a String result",
                ));
            }
            Binding {
                symbol: symbol.clone(),
                result: *result,
                adapted: true,
            }
        } else {
            if f.fails {
                return Err(unsupported(
                    &f.name,
                    "failing synchronous external needs an explicit Options::with_external_adapter mapping",
                ));
            }
            Binding {
                symbol: validate_external(flow, f)?,
                result: ExternalResult::Owned,
                adapted: false,
            }
        };
        let mut abi = vec![layouts.c_type(f.ret)];
        abi.extend(f.params.iter().map(|&p| layouts.c_type(f.vars[p].ty)));
        if f.fails {
            abi.push("EmelaError *".into());
        }
        if binding.result == ExternalResult::StringBuffer {
            abi.extend(["char *".into(), "uint16_t".into()]);
        }
        if let Some(previous) =
            declarations.insert(binding.symbol.clone(), (abi.clone(), binding.result))
            && previous != (abi, binding.result)
        {
            return Err(unsupported(
                &f.name,
                "external symbol is instantiated with incompatible signatures or result ownership",
            ));
        }
        if binding.adapted {
            for ty in f
                .params
                .iter()
                .map(|&p| f.vars[p].ty)
                .chain(std::iter::once(f.ret))
            {
                if contains_any(layouts, ty, &mut HashSet::new()) {
                    layouts.any_box_required = true;
                }
            }
        }
        bindings.insert(id, binding);
    }
    for (_, f) in flow
        .functions
        .iter()
        .filter(|(id, f)| emitted.contains(id) && f.external.is_some() && f.suspends)
    {
        peripheral::resolve(flow, f)?;
        let symbol = suspend::external_symbol(f, options)?;
        let mut abi = vec!["void".into(), "EmelaTask".into()];
        abi.extend(f.params.iter().map(|&p| layouts.c_type(f.vars[p].ty)));
        if let Some(previous) = declarations.insert(symbol, (abi.clone(), ExternalResult::Owned))
            && previous != (abi, ExternalResult::Owned)
        {
            return Err(unsupported(
                &f.name,
                "external symbol is instantiated with incompatible synchronous and task signatures",
            ));
        }
    }
    layouts.managed_error_payloads = layouts
        .error_types
        .iter()
        .any(|&(_, ty)| layouts.is_managed_value(ty));
    Ok(bindings)
}

fn contains_any(layouts: &TypeLayouts<'_>, ty: TypeId, seen: &mut HashSet<TypeId>) -> bool {
    if !seen.insert(ty) {
        return false;
    }
    match &layouts.types[ty] {
        Type::Any => true,
        Type::List(t) | Type::Cell(t) => contains_any(layouts, *t, seen),
        _ => layouts.aggregate_ids.get(&ty).is_some_and(|&index| {
            match &layouts.aggregates[index].kind {
                AggregateKind::Tuple(fields) => {
                    fields.iter().any(|&t| contains_any(layouts, t, seen))
                }
                AggregateKind::Enum(variants) => variants
                    .iter()
                    .flatten()
                    .any(|&t| contains_any(layouts, t, seen)),
            }
        }),
    }
}

pub(super) fn needs_heap(
    function: &Function,
    binding: Option<&Binding>,
    layouts: &TypeLayouts<'_>,
    options: &Options,
) -> bool {
    binding.is_some_and(|b| {
        b.adapted
            && ((b.result == ExternalResult::Borrowed && layouts.is_managed_value(function.ret))
                || (options.heap_bytes() != 0
                    && std::iter::once(function.ret)
                        .chain(function.params.iter().map(|&p| function.vars[p].ty))
                        .any(|ty| layouts.is_managed_value(ty))))
    })
}

pub(super) fn isr_symbol<'a>(
    flow: &emela_lir::Module,
    function: &Function,
    native: &'a str,
    options: &'a Options,
    entry: &str,
) -> Result<&'a str, Vec<Error>> {
    if crate::fiber::is_infallible_cause_fail(function) {
        return Ok(native);
    }
    let original = symbol(function)?;
    if let Some((_, adapter)) = options
        .external_adapters
        .iter()
        .find(|(s, _)| *s == original)
    {
        if !std::iter::once(function.ret)
            .chain(function.params.iter().map(|&p| function.vars[p].ty))
            .all(|ty| suspend::scalar(&flow.core.types, ty))
        {
            return Err(unsupported(
                &function.name,
                &format!("non-scalar external adapter reachable from ISR entry `{entry}`"),
            ));
        }
        Ok(&adapter.symbol)
    } else {
        Ok(native)
    }
}

pub(super) fn extend_string_output(
    flow: &emela_lir::Module,
    emitted: &HashSet<FnId>,
    bindings: &Bindings,
    layouts: &TypeLayouts<'_>,
    output: &mut HashSet<FnId>,
) {
    output.extend(
        bindings
            .iter()
            .filter_map(|(&id, b)| (b.result == ExternalResult::StringBuffer).then_some(id)),
    );
    loop {
        let mut changed = false;
        for (id, f) in flow.functions.iter() {
            if !emitted.contains(&id)
                || f.external.is_some()
                || f.suspends
                || f.constant
                || layouts.owned_string_returns.contains(&f.name)
                || f.ret != TypeId::STRING
                || output.contains(&id)
            {
                continue;
            }
            if f.stmts.iter().any(|(_, s)| matches!(s, Stmt::Let { value: Value::Call { callee: Callee::Direct(target), .. }, .. } if output.contains(target))) {
                changed |= output.insert(id);
            }
        }
        if !changed {
            break;
        }
    }
}

fn boundary_types(flow: &emela_lir::Module, bindings: &Bindings) -> Vec<TypeId> {
    let mut types: Vec<_> = bindings
        .iter()
        .filter(|(_, b)| b.adapted)
        .filter(|(id, _)| crate::derived::kind(&flow.functions[**id]).is_none())
        .filter(|(id, _)| !peripheral::is_binding(&flow.functions[**id]))
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

pub(super) fn emit_header(
    flow: &emela_lir::Module,
    bindings: &Bindings,
    layouts: &TypeLayouts<'_>,
    heap: bool,
    writer: &mut Writer,
) {
    let mut ids: Vec<_> = bindings.keys().copied().collect();
    ids.sort_by_key(|id| u32::from(id.into_raw()));
    for id in ids {
        let b = &bindings[&id];
        if !b.adapted
            || crate::derived::kind(&flow.functions[id]).is_some()
            || peripheral::is_binding(&flow.functions[id])
        {
            continue;
        }
        writer.line(
            "/* Synchronous generated-value adapter. Arguments are borrowed during this call.",
        );
        writer.line(
            " * No scheduler/start/cancel reentry or automatic conversion to native C callbacks.",
        );
        match b.result {
            ExternalResult::Borrowed => writer.line(" * Result remains valid until the generated wrapper finishes acquiring an owning copy; never return callee-stack bytes."),
            ExternalResult::Owned => writer.line(" * Result transfers all owned references from this module's fixed heap; never malloc or callee-stack owners."),
            ExternalResult::StringBuffer => writer.line(" * Check capacity before writing. Return owner=NULL, data=emela_string_output, length<=capacity."),
        }
        writer.line(" */");
        if flow.functions[id].fails {
            writer.line("/* Non-NULL error output starts zero. On failure set a module error tag; the initialized C return value is ignored, including its managed fields.");
            writer.line(" * Owned transfers the error; Borrowed/StringBuffer keep its payload valid until the wrapper snapshots it. Success leaves error output zero. */");
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
            writer.line("/* Acquire a complete owning copy in a borrowed record, or release and zero an owned record. */");
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

pub(super) fn emit_ownership_helpers(
    flow: &emela_lir::Module,
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
    for &(id, ty) in &layouts.error_types {
        writer.line(&format!("case UINT32_C({}):", enum_index(id) + 1));
        let payload = format!("emela_error_out->payload.e{}", enum_index(id));
        let variant = if layouts.aggregate_ids.contains_key(&ty) {
            format!("{payload}.tag")
        } else {
            payload
        };
        writer.line(&format!("if ((uint32_t)({variant}) >= UINT32_C({})) emela_board_defect(EMELA_DEFECT_ASSERT, \"invalid external error\", UINT16_C(22));",layouts.core.enums[id].variants.len()));
        writer.line("break;");
    }
    writer.line("default: emela_board_defect(EMELA_DEFECT_ASSERT, \"invalid external error\", UINT16_C(22));");
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

pub(super) fn emit_adapter(
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
        writer.begin_block("if (returned.owner != NULL || returned.length > emela_string_capacity || (returned.length != 0 && (returned.data != emela_string_output || emela_string_output == NULL)))");
        writer.line("emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"invalid external String buffer\", UINT16_C(30));");
        writer.end_block();
    }
    writer.ret(Some("returned"));
    writer.end_block();
    writer.blank();
}
