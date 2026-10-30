//! crate::contract::ir::Term::Return
use super::super::*;
pub(in crate::compiler::emit) fn emit(
    context: &mut Context<'_, '_>,
    term: &Term,
    _dst: Option<VarId>,
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    let names = context.names;
    let string_buffers = context.string_buffers;
    let loops = &mut *context.loops;
    let blocks = &mut *context.blocks;
    let protects = &mut *context.protects;
    let next_temp = &mut *context.next_temp;
    let next_label = &mut *context.next_label;
    let Term::Return(_) = term else {
        unreachable!("terminal dispatch")
    };
    {
        emit_pending_cleanups(
            layouts,
            function,
            0,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        )?;
        emit_return(layouts, function, term, string_buffers, writer)?;
    };
    Ok(())
}

pub(in crate::compiler::emit) fn emit_function(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    term: &Term,
    string_buffers: &StringBufferPlan,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    match term {
        Term::Return(None) => {
            emit_owned_cleanup(layouts, function, None, writer);
            writer.ret(None);
        }
        Term::Return(Some(operand)) => {
            let mut expression = operand_expression(operand)
                .map_err(|variant| unsupported(&function.name, variant))?;
            if function.ret == TypeId::ANY
                && matches ! (operand , Operand :: Const (Const :: String (s)) if s . is_empty ())
            {
                expression = "(EmelaAny){0}".into();
            }
            if layouts
                .functions
                .owned_string_returns
                .contains(&function.name)
            {
                writer.line(&format!(
                    "EmelaString emela_owned_return = emela_heap_own_string({expression});"
                ));
                emit_owned_cleanup(layouts, function, None, writer);
                writer.ret(Some("emela_owned_return"));
                return Ok(());
            }
            if layouts
                .functions
                .owned_aggregate_returns
                .contains(&function.name)
            {
                writer.decl(
                    &layouts.c_type(function.ret),
                    "emela_owned_return",
                    Some(&expression),
                );
                layouts.emit_own_value_in_place(function.ret, "emela_owned_return", writer);
                emit_owned_cleanup(layouts, function, None, writer);
                writer.ret(Some("emela_owned_return"));
                return Ok(());
            }
            let mut moved = None;
            let copies_string_to_caller = string_buffers.returns_to_caller
                && matches!(layouts.shapes.types[function.ret], Type::String);
            if copies_string_to_caller {
                writer.decl(
                    "EmelaString",
                    "emela_string_return_source",
                    Some(&expression),
                );
                writer . decl ("EmelaString" , "emela_string_return_value" , Some (& format ! ("emela_emb_string_copy_to_output(emela_string_return_source, emela_string_output, emela_emb_string_output_capacity, UINT16_C({}))" , string_buffers . capacity)) ,) ;
                expression = "emela_string_return_value".to_owned();
            }
            if !copies_string_to_caller
                && layouts.is_managed_value(function.ret)
                && let Operand::Var(variable) = operand
            {
                if function.params.contains(variable) {
                    emit_retain_operand(layouts, function, operand, writer);
                } else {
                    writer.assign(&owned_variable_name(*variable), "INT32_C(0)");
                    moved = Some(*variable);
                }
            }
            emit_owned_cleanup(layouts, function, moved, writer);
            writer.ret(Some(&expression));
        }
        _ => unreachable!("function terminal"),
    };
    let return_repr = layouts.type_repr(function.ret);
    if return_repr == Repr::Void && matches!(term, Term::Return(Some(_))) {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: void return has a value",
                function.name
            ),
        }]);
    }
    Ok(())
}

pub(in crate::compiler::emit) fn emit_suspend(
    context: &mut SuspendContext<'_, '_>,
    node: &Node,
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    let Node::Return(value) = node else {
        unreachable!("frame state dispatch")
    };
    {
        if let Some(value) = value {
            writer.begin_block("if (frame->result != NULL)");
            let mut expression =
                operand_expression(value).map_err(|why| unsupported(&function.name, why))?;
            if function.ret == TypeId::ANY
                && matches ! (value , Operand :: Const (Const :: String (s)) if s . is_empty ())
            {
                expression = "(EmelaAny){0}".into();
            }
            writer.decl(&layouts.c_type(function.ret), "returned", Some(&expression));
            retain_frame_value(layouts, function.ret, "returned", writer);
            writer.line("*frame->result = returned;");
            writer.end_block();
        }
        writer.line("emela_frame_return(fiber); return;");
    };
    Ok(())
}
