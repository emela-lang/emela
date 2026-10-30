//! crate::contract::ir::Stmt::Switch
use super::super::*;
pub(in crate::compiler::emit) fn emit(
    context: &mut Context<'_, '_>,
    statement: &Stmt,
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
    match statement {
        Stmt::Switch {
            dst,
            on,
            cases,
            default,
        } => emit_switch(
            layouts,
            function,
            *dst,
            *on,
            cases,
            *default,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        ),
        other => Err(unsupported(&function.name, statement_name(other))),
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::compiler::emit) fn emit_switch(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    on: SwitchOn,
    cases: &[crate::contract::ir::Case],
    default: Option<crate::contract::ir::SeqId>,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    let (scrutinee, tag_switch) = features::switches::scrutinee(layouts, function, on)?;

    if !tag_switch && default.is_none() {
        return Err(invalid_flow_ir(
            &function.name,
            "literal switch has no default sequence",
        ));
    }

    writer.begin_block(&format!("switch ({scrutinee})"));
    for case in cases {
        let label = features::case_keys::label(layouts, function, &case.key, on)?;
        writer.begin_block(&format!("case {label}:"));
        emit_compound_sequence(
            layouts,
            function,
            case.body,
            dst,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        )?;
        writer.line("break;");
        writer.end_block();
    }

    if let Some(default) = default {
        writer.begin_block("default:");
        emit_compound_sequence(
            layouts,
            function,
            default,
            dst,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        )?;
        writer.line("break;");
        writer.end_block();
    } else {
        writer.begin_block("default:");
        writer.line("emela_board_defect(EMELA_DEFECT_NO_MATCH, \"no match\", UINT16_C(8));");
        writer.end_block();
    }
    writer.end_block();
    Ok(())
}

pub(in crate::compiler::emit) fn emit_suspend(
    context: &mut SuspendContext<'_, '_>,
    node: &Node,
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    let Node::Switch(on, cases, default) = node else {
        unreachable!("frame state dispatch")
    };
    {
        let (scrutinee, tag_switch) = match *on {
            SwitchOn::Lit(var) => (variable_name(var), false),
            SwitchOn::Tag(var) => {
                let ty = function.vars[var].ty;
                let Type::Enum(id, _) = layouts.shapes.types[ty] else {
                    return Err(invalid_flow_ir(&function.name, "tag switch on a non-enum"));
                };
                if layouts.enum_is_i32(id) {
                    (variable_name(var), true)
                } else if layouts.enum_fields(ty).is_some() {
                    (format!("{}.tag", variable_name(var)), true)
                } else {
                    return Err(unsupported(
                        &function.name,
                        "suspend switch enum representation",
                    ));
                }
            }
        };
        writer.begin_block(&format!("switch ({scrutinee})"));
        for (key, target) in cases {
            let label =
                match (key, on) {
                    (CaseKey::Lit(value), SwitchOn::Lit(_)) => constant_expression(value)
                        .map_err(|why| unsupported(&function.name, why))?
                        .ok_or_else(|| invalid_flow_ir(&function.name, "void switch case"))?,
                    (CaseKey::Variant(variant), SwitchOn::Tag(var)) => {
                        let Type::Enum(id, _) = layouts.shapes.types[function.vars[*var].ty] else {
                            unreachable!()
                        };
                        if *variant >= layouts.shapes.core.enums[id].variants.len() {
                            return Err(invalid_flow_ir(
                                &function.name,
                                "switch variant out of range",
                            ));
                        }
                        int32_literal(i32::try_from(*variant).map_err(|_| {
                            invalid_flow_ir(&function.name, "switch variant too large")
                        })?)
                    }
                    _ => return Err(invalid_flow_ir(&function.name, "switch key type")),
                };
            writer.line(&format!("case {label}: frame->base.pc = {target}; break;"));
        }
        if let Some(target) = default {
            writer.line(&format!("default: frame->base.pc = {target}; break;"));
        } else if tag_switch {
            writer.line(
                "default: emela_board_defect(EMELA_DEFECT_NO_MATCH, \"no match\", UINT16_C(8));",
            );
        } else {
            writer.line("default: emela_scheduler_invalid();");
        }
        writer.end_block();
        writer.line("continue;");
    };
    Ok(())
}
