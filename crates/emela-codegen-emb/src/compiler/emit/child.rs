use super::*;

pub(super) use crate::semantics::child::*;
pub(super) fn emit_runtime(
    layouts: &TypeLayouts<'_>,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    for &ty in &layouts.runtime.child_join_types {
        if layouts.type_repr(ty) == Repr::Void {
            continue;
        }
        writer.begin_block(&format!(
            "static void emela_child_join_{}(emela_child *child, void *result)",
            ty.index()
        ));
        writer.line("if (child->status != 1) emela_scheduler_invalid();");
        writer.line("EmelaAny emela_v_0 = child->result;");
        writer.line(&format!("{} emela_v_1 = {{0}};", layouts.c_type(ty)));
        let f = deferred::coerce_function(layouts, TypeId::ANY, ty);
        let vars: Vec<_> = f.vars.iter().map(|(id, _)| id).collect();
        emit_coerce(layouts, &f, Some(vars[1]), vars[0], writer)?;
        writer.line(&format!("*({} *)result = emela_v_1;", layouts.c_type(ty)));
        writer.end_block();
    }
    for &exit in &layouts.runtime.child_await_types {
        let ty = success_type(layouts, exit).expect("validated Await Success payload");
        let represented = layouts.type_repr(ty) != Repr::Void;
        writer.begin_block(&format!(
            "static void emela_child_await_{}(emela_child *child, void *result)",
            exit.index()
        ));
        let failure = layouts
            .enum_fields(exit)
            .and_then(|variants| variants.get(crate::contract::ir::EXIT_FAILURE));
        if !failure.is_some_and(|fields| fields.len() == 1 && layouts.is_cause(fields[0])) {
            return Err(invalid_flow_ir(
                "Fiber.await",
                "Exit Failure payload is not Cause",
            ));
        }
        writer.begin_block("if (child->status == 2)");
        writer . line (& format ! ("*({cty} *)result = ({cty}){{.tag = INT32_C({tag}), .payload.v{tag}.f0 = emela_cause_interrupt()}};" , cty = layouts . c_type (exit) , tag = crate :: contract :: ir :: EXIT_FAILURE)) ;
        writer.line("return;");
        writer.end_block();
        if layouts.runtime.has_error_abi {
            writer.begin_block("if (child->status == 3)");
            writer . line (& format ! ("*({cty} *)result = ({cty}){{.tag = INT32_C({tag}), .payload.v{tag}.f0 = emela_cause_fail(child->error)}};" , cty = layouts . c_type (exit) , tag = crate :: contract :: ir :: EXIT_FAILURE)) ;
            writer.line("return;");
            writer.end_block();
        }
        writer.line("if (child->status != 1) emela_scheduler_invalid();");
        if represented {
            writer.line("EmelaAny emela_v_0 = child->result;");
            writer.line(&format!("{} emela_v_1 = {{0}};", layouts.c_type(ty)));
            let f = deferred::coerce_function(layouts, TypeId::ANY, ty);
            let vars: Vec<_> = f.vars.iter().map(|(id, _)| id).collect();
            emit_coerce(layouts, &f, Some(vars[1]), vars[0], writer)?;
        }
        let field = if represented {
            "f0 = emela_v_1"
        } else {
            "_unit = UINT8_C(0)"
        };
        writer.line(&format!(
            "*({cty} *)result = ({cty}){{.tag = INT32_C({tag}), .payload.v{tag}.{field}}};",
            cty = layouts.c_type(exit),
            tag = crate::contract::ir::EXIT_SUCCESS
        ));
        writer.end_block();
    }
    Ok(())
}

pub(super) fn emit_wait(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    op: crate::contract::ir::FiberOp,
    args: &[Operand],
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let [handle] = args else {
        return Err(invalid_flow_ir(&function.name, "Join arity"));
    };
    let ty = value_type(layouts, function, handle)?;
    if op == crate::contract::ir::FiberOp::Interrupt {
        let handle = operand_expression(handle).map_err(|why| unsupported(&function.name, why))?;
        writer.line(&format!("emela_child_interrupt(fiber, {handle});"));
        return Ok(());
    }
    let result = dst
        .map(|v| format!("&{}", variable_name(v)))
        .unwrap_or_else(|| "NULL".into());
    let (awaiting, deliver) = match op {
        crate::contract::ir::FiberOp::Join => (
            0,
            if layouts.type_repr(ty) == Repr::Void {
                "NULL".into()
            } else {
                format!("emela_child_join_{}", ty.index())
            },
        ),
        crate::contract::ir::FiberOp::Await { exit } => {
            let result_ty = layouts
                .runtime
                .child_await_types
                .iter()
                .copied()
                .find(|&result| {
                    matches ! (layouts.shapes.types [result] , Type :: Enum (id , _) if id == exit)
                        && success_type(layouts, result) == Some(ty)
                })
                .ok_or_else(|| invalid_flow_ir(&function.name, "Await result layout"))?;
            (1, format!("emela_child_await_{}", result_ty.index()))
        }
        _ => return Err(invalid_flow_ir(&function.name, "child wait operation")),
    };
    let handle = operand_expression(handle).map_err(|why| unsupported(&function.name, why))?;
    writer.line(&format!(
        "emela_child_wait(fiber, {handle}, {awaiting}, {result}, {deliver});"
    ));
    Ok(())
}
