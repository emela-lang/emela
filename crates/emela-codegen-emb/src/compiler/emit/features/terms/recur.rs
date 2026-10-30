//! crate::contract::ir::Term::Recur
use super::super::*;
pub(in crate::compiler::emit) fn emit(
    context: &mut Context<'_, '_>,
    term: &Term,
    _dst: Option<VarId>,
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    let loops = &mut *context.loops;
    let next_temp = &mut *context.next_temp;
    let Term::Recur(args) = term else {
        unreachable!("terminal dispatch")
    };
    emit_recur(layouts, function, args, loops, writer, next_temp)?;
    Ok(())
}

pub(in crate::compiler::emit) fn emit_recur(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    args: &[Operand],
    loops: &[Vec<VarId>],
    writer: &mut Writer,
    next_temp: &mut usize,
) -> Result<(), Vec<Error>> {
    let Some(vars) = loops.last() else {
        return Err(invalid_flow_ir(
            &function.name,
            "Recur appears outside an emitted loop",
        ));
    };
    if vars.len() != args.len() {
        return Err(invalid_flow_ir(
            &function.name,
            "Recur argument and loop variable counts differ",
        ));
    }

    let mut temporaries = Vec::with_capacity(args.len());
    for (&variable, argument) in vars.iter().zip(args) {
        let expression =
            operand_expression(argument).map_err(|variant| unsupported(&function.name, variant))?;
        let temporary = format!("emela_recur_tmp_{}", *next_temp);
        *next_temp += 1;
        writer.decl(
            &layouts.c_type(function.vars[variable].ty),
            &temporary,
            Some(&expression),
        );
        temporaries.push(temporary);
    }
    for (&variable, temporary) in vars.iter().zip(&temporaries) {
        if layouts.is_managed_value(function.vars[variable].ty) {
            layouts.emit_retain_value_expression(function.vars[variable].ty, temporary, writer);
        }
    }
    for &variable in vars {
        emit_release_if_owned(layouts, function, variable, writer);
    }
    for (&variable, temporary) in vars.iter().zip(temporaries) {
        writer.assign(&variable_name(variable), &temporary);
        if layouts.is_managed_value(function.vars[variable].ty) {
            writer.assign(&owned_variable_name(variable), "INT32_C(1)");
        }
    }
    writer.line("continue;");
    Ok(())
}

pub(in crate::compiler::emit) fn emit_function(
    _layouts: &TypeLayouts<'_>,
    function: &Function,
    term: &Term,
    _string_buffers: &StringBufferPlan,
    _writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    Err(unsupported(&function.name, term_name(term)))
}
