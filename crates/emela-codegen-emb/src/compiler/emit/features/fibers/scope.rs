//! crate::contract::ir::FiberOp::Scope
use super::super::*;
pub(in crate::compiler::emit) fn emit(
    context: &mut Context<'_, '_>,
    _dst: &Option<VarId>,
    _value: &Value,
) -> Result<(), Vec<Error>> {
    let function = context.function;
    Err(unsupported(
        &function.name,
        "Fiber::Scope (phase 7 follow-up)",
    ))
}

pub(in crate::compiler::emit) fn emit_suspend(
    context: &mut SuspendContext<'_, '_>,
    statement: &Stmt,
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    let next = &context.next;
    if let Stmt::Let {
        dst,
        value:
            Value::Fiber {
                op: crate::contract::ir::FiberOp::Scope { .. },
                args,
                on_fail,
            },
        ..
    } = statement
    {
        let [Operand::Var(body)] = args.as_slice() else {
            return Err(invalid_flow_ir(
                &function.name,
                "Scope body is not a function value",
            ));
        };
        let Type::Fn(ft) = &layouts.shapes.types[function.vars[*body].ty] else {
            return Err(invalid_flow_ir(&function.name, "Scope body type"));
        };
        if ft.ev || !ft.suspends || ft.params != [TypeId::FIBER_SCOPE] {
            return Err(unsupported(&function.name, "Scope body ABI"));
        }
        prepare_result(layouts, function, *dst, writer);
        writer.line(&format!("frame->base.pc = {next};"));
        writer.line(&format!(
            "emela_scope *scope = emela_scope_begin(fiber{});",
            if layouts.runtime.has_error_abi {
                if on_fail.is_some() {
                    ", &frame->call_error"
                } else {
                    ", NULL"
                }
            } else {
                ""
            }
        ));
        let body = variable_name(*body);
        writer.line(&format!(
            "if ({body}.call == NULL) emela_scheduler_invalid();"
        ));
        let mut call_args = vec![
            "fiber".to_owned(),
            format!("{body}.env"),
            "scope".to_owned(),
        ];
        if ft.fails {
            call_args.push("&scope->frame->body_error".into());
        }
        if layouts.type_repr(ft.ret) != Repr::Void {
            call_args.push(
                dst.map(|v| format!("&{}", variable_name(v)))
                    .unwrap_or_else(|| "NULL".into()),
            );
        }
        writer.line(&format!("{body}.call({}); return;", call_args.join(", ")));
    } else {
        unreachable!("suspend operation entry")
    };
    Ok(())
}
