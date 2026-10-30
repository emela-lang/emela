//! Shared validation of a declared builtin signature. Operation behavior stays in its entry.
use super::*;

pub(super) enum ResultType {
    Exact(Type),
    Option(TypeId),
}

pub(super) fn prepare(
    context: &BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    op: Builtin,
    args: &[Operand],
    arity: usize,
    result: ResultType,
    arguments_match: fn(&TypeLayouts<'_>, &Function, &[Operand]) -> bool,
) -> Result<(VarId, Vec<String>), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    if args.len() != arity {
        return Err(invalid_flow_ir(
            &function.name,
            &format!(
                "builtin {op:?} has {} arguments, expected {arity}",
                args.len()
            ),
        ));
    }
    if !arguments_match(layouts, function, args) {
        return Err(invalid_flow_ir(
            &function.name,
            &format!("builtin {op:?} has an argument with the wrong type"),
        ));
    }
    let Some(dst) = dst else {
        return Err(invalid_flow_ir(
            &function.name,
            &format!("builtin {op:?} has no result destination"),
        ));
    };
    let matches = match result {
        ResultType::Exact(ty) => layouts.shapes.types[function.vars[dst].ty] == ty,
        ResultType::Option(ty) => option_payload_type(layouts, function.vars[dst].ty)
            .is_some_and(|payload| layouts.shapes.types[payload] == layouts.shapes.types[ty]),
    };
    if !matches {
        return Err(invalid_flow_ir(
            &function.name,
            &format!("builtin {op:?} has a result with the wrong type"),
        ));
    }
    let expressions = args
        .iter()
        .map(operand_expression)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|variant| unsupported(&function.name, variant))?;
    Ok((dst, expressions))
}

pub(super) fn cell_arity(
    context: &BuiltinContext<'_, '_>,
    args: &[Operand],
    arity: usize,
) -> Emission {
    if args.len() == arity {
        Ok(())
    } else {
        Err(invalid_flow_ir(
            &context.function.name,
            "Cell builtin has an invalid argument count",
        ))
    }
}

pub(super) struct CellStorage {
    pub(super) content: TypeId,
    pub(super) c_type: String,
    pub(super) expression: String,
}

pub(super) fn cell_storage(
    context: &BuiltinContext<'_, '_>,
    operand: &Operand,
) -> Result<CellStorage, Vec<Error>> {
    let cell_ty = operand_type(context.function, operand).expect("Cell operand has a type");
    let Type::Cell(content) = context.layouts.shapes.types[cell_ty] else {
        return Err(invalid_flow_ir(
            &context.function.name,
            "Cell builtin first argument is not a Cell",
        ));
    };
    let cell = operand_expression(operand).map_err(|v| unsupported(&context.function.name, v))?;
    let c_type = context.layouts.cell_content_c_type(content);
    Ok(CellStorage {
        content,
        expression: format!("(*(({c_type} *){cell}))"),
        c_type,
    })
}
