//! Synchronous statement, control-flow, cleanup, and failure emission.
use super::functions::{emit_owned_cleanup, emit_release_if_owned};
use super::{
    StringBufferPlan, TypeLayouts,
    analysis::ResultOwnership,
    c::{enum_index, owned_variable_name, variable_name},
    diagnostics::invalid_flow_ir,
    features,
};
use crate::contract::ir::Type;
use crate::contract::ir::{
    FnId, Function, LabelId, OnFail, Operand, Repr, Stmt, Term, Value, VarId,
};
use crate::semantics::abi::LayoutRead;
use crate::{Error, writer::Writer};
use std::collections::HashMap;

#[allow(clippy::too_many_arguments)]
pub(in crate::compiler::emit) fn emit_statement(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    statement: &Stmt,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    let managed_dst = statement
        .dst()
        .filter(|&dst| layouts.is_managed_value(function.vars[dst].ty));
    if let Some(dst) = managed_dst {
        emit_release_if_owned(layouts, function, dst, writer);
    }
    let retain_result = matches!(statement, Stmt::Let { dst: Some(dst), .. }
        if string_buffers.result_ownership.get(dst) == Some(&ResultOwnership::Borrowed));
    emit_statement_inner(
        layouts,
        function,
        statement,
        writer,
        names,
        string_buffers,
        loops,
        blocks,
        protects,
        next_temp,
        next_label,
    )?;
    if let Some(dst) = managed_dst {
        if retain_result {
            layouts.emit_retain_value_expression(
                function.vars[dst].ty,
                &variable_name(dst),
                writer,
            );
        }
        writer.assign(&owned_variable_name(dst), "INT32_C(1)");
        if string_buffers.discarded_results.contains(&dst) {
            emit_release_if_owned(layouts, function, dst, writer);
        }
    }
    if layouts.runtime.has_cause_raise
        && layouts.runtime.has_unwind
        && !function.suspends
        && matches!(
            statement,
            Stmt::Let {
                value: Value::Call { .. }
                    | Value::Fiber {
                        op: crate::contract::ir::FiberOp::Attempt { .. },
                        ..
                    },
                ..
            }
        )
    {
        emit_sync_interrupt_exit(
            layouts,
            function,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(in crate::compiler::emit) fn emit_statement_inner(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    statement: &Stmt,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    features::statements::emit(
        &mut features::Context {
            layouts,
            function,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        },
        statement,
    )
}

#[allow(clippy::too_many_arguments)]
pub(in crate::compiler::emit) fn emit_compound_sequence(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    sequence: crate::contract::ir::SeqId,
    dst: Option<VarId>,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    let sequence = &function.seqs[sequence];
    for &statement in &sequence.stmts {
        emit_statement(
            layouts,
            function,
            &function.stmts[statement],
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        )?;
    }
    features::terms::emit(
        &mut features::Context {
            layouts,
            function,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        },
        &sequence.term,
        dst,
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(in crate::compiler::emit) fn emit_call_failure_edge(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    edge: OnFail,
    error_name: &str,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    features::failures::emit(
        &mut features::Context {
            layouts,
            function,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        },
        edge,
        error_name,
    )
}

pub(in crate::compiler::emit) fn emit_error_assignment(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    error: &Operand,
    target: &str,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let Operand::Var(variable) = error else {
        return Err(invalid_flow_ir(
            &function.name,
            "Fail operand is not an error variable",
        ));
    };
    let ty = function.vars[*variable].ty;
    if matches!(layouts.shapes.types[ty], Type::Any) {
        writer.assign(target, &variable_name(*variable));
        writer.line(&format!("emela_error_retain({target});"));
        return Ok(());
    }
    let Some(enum_id) = layouts.error_enum(ty) else {
        return Err(invalid_flow_ir(
            &function.name,
            "Fail operand is not an error enum or propagated EmelaError",
        ));
    };
    if layouts.error_type(enum_id).is_none() {
        return Err(invalid_flow_ir(
            &function.name,
            "Fail operand error enum is missing from EmelaError layout",
        ));
    }
    let object = format!("({target})");
    writer.assign(target, "(EmelaError){0}");
    writer.assign(
        &format!("{object}.tag"),
        &format!("UINT32_C({})", enum_index(enum_id) + 1),
    );
    layouts.emit_owned_value_store(
        ty,
        &variable_name(*variable),
        &format!("{object}.payload.e{}", enum_index(enum_id)),
        writer,
    );
    Ok(())
}

pub(in crate::compiler::emit) fn emit_failure_return(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    writer: &mut Writer,
) {
    emit_owned_cleanup(layouts, function, None, writer);
    if layouts.type_repr(function.ret) == Repr::Void {
        writer.ret(None);
    } else {
        writer.ret(Some(&format!("({}){{0}}", layouts.c_type(function.ret))));
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::compiler::emit) fn emit_pending_cleanups(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    keep_depth: usize,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    if keep_depth > protects.len() {
        return Err(invalid_flow_ir(
            &function.name,
            "control transfer escapes a block outside the active protect stack",
        ));
    }
    let saved = protects.clone();
    for index in (keep_depth..saved.len()).rev() {
        protects.truncate(index);
        let result = emit_compound_sequence(
            layouts,
            function,
            saved[index].cleanup,
            None,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        );
        *protects = saved.clone();
        result?;
    }
    Ok(())
}

pub(in crate::compiler::emit) fn emit_return(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    term: &Term,
    string_buffers: &StringBufferPlan,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    features::terms::emit_function(layouts, function, term, string_buffers, writer)
}

#[allow(clippy::too_many_arguments)]
pub(in crate::compiler::emit) fn emit_sync_interrupt_exit(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    strings: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    writer.begin_block("if (emela_current_fiber != NULL && emela_current_fiber->forced)");
    writer.line("emela_current_fiber->forced = 0;");
    writer.line("emela_interrupt_hold(emela_current_fiber);");
    emit_pending_cleanups(
        layouts, function, 0, writer, names, strings, loops, blocks, protects, next_temp,
        next_label,
    )?;
    writer.line("emela_current_fiber->forced = 1;");
    if function.fails {
        writer.line("emela_error_clear(emela_error_out);");
    }
    emit_failure_return(layouts, function, writer);
    writer.end_block();
    Ok(())
}

pub(in crate::compiler::emit) struct BlockTarget {
    pub(in crate::compiler::emit) label: LabelId,
    pub(in crate::compiler::emit) dst: Option<VarId>,
    pub(in crate::compiler::emit) exit: String,
    pub(in crate::compiler::emit) used: bool,
    pub(in crate::compiler::emit) protect_depth: usize,
}

#[derive(Clone, Copy)]
pub(in crate::compiler::emit) struct ProtectTarget {
    pub(in crate::compiler::emit) cleanup: crate::contract::ir::SeqId,
}

use super::layouts::LayoutEmission;
