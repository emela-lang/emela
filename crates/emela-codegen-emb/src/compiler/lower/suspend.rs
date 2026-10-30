//! Prepare structured suspension inputs without changing the caller's Flow module.
use super::closure::ClosureLowering;
use emela_lir::{FnId, Function, LabelId, Module, Operand, Stmt, SwitchOn, Term, Value, VarId};
use std::collections::{HashMap, HashSet};

pub(super) fn prepare(
    program: &Module,
    closures: &ClosureLowering,
) -> HashMap<FnId, (Function, VarId, LabelId)> {
    program
        .functions
        .iter()
        .filter(|(_, function)| function.suspends && function.external.is_none())
        .map(|(id, function)| {
            let mut function = function.clone();
            discard_unused_child_handles(program, closures, &mut function);
            let error = function.vars.alloc(emela_lir::VarData {
                hint: "call_error".into(),
                ty: emela_core::TypeId::ANY,
                repr: emela_lir::Repr::Ref,
            });
            let label = function.labels.alloc(emela_lir::LabelData {
                hint: "call_error".into(),
            });
            (id, (function, error, label))
        })
        .collect()
}
fn for_each_plan_var(closures: &ClosureLowering, value: &Value, mut visit: impl FnMut(VarId)) {
    value.for_each_var(&mut visit);
    if let Value::FnRef(target) = value
        && let Some(&environment) = closures.functions.get(target)
    {
        for capture in &closures.environments[environment].captures {
            visit(capture.source);
        }
    }
}
fn count_operand(operand: &Operand, count: &mut impl FnMut(VarId)) {
    if let Operand::Var(variable) = operand {
        count(*variable);
    }
}
fn discard_unused_child_handles(
    program: &Module,
    closures: &ClosureLowering,
    function: &mut Function,
) {
    let mut used = HashSet::new();
    for (_, statement) in function.stmts.iter() {
        match statement {
            Stmt::Let { value, .. } => for_each_plan_var(closures, value, |id| {
                used.insert(id);
            }),
            Stmt::If { cond, .. } => count_operand(cond, &mut |id| {
                used.insert(id);
            }),
            Stmt::Switch {
                on: SwitchOn::Tag(id) | SwitchOn::Lit(id),
                ..
            }
            | Stmt::Dup(id)
            | Stmt::Drop(id) => {
                used.insert(*id);
            }
            Stmt::Loop { inits, .. } => {
                for value in inits {
                    count_operand(value, &mut |id| {
                        used.insert(id);
                    });
                }
            }
            Stmt::Block { .. } | Stmt::Protect { .. } => {}
        }
    }
    for (_, sequence) in function.seqs.iter() {
        let mut add = |id| {
            used.insert(id);
        };
        match &sequence.term {
            Term::Return(value) | Term::Yield(value) | Term::Break { value, .. } => {
                if let Some(value) = value {
                    count_operand(value, &mut add);
                }
            }
            Term::Fail(value) | Term::Defect(value, _) => count_operand(value, &mut add),
            Term::Recur(values) => {
                for value in values {
                    count_operand(value, &mut add);
                }
            }
            Term::Unwind | Term::Unreachable => {}
        }
    }
    for (_, statement) in function.stmts.iter_mut() {
        if let Stmt::Let {
            dst: destination @ Some(_),
            value:
                Value::Call { .. }
                | Value::Fiber {
                    op: emela_core::FiberOp::Fork,
                    ..
                },
            ..
        } = statement
        {
            let id = destination.unwrap();
            if !used.contains(&id)
                && matches!(program.core.types[function.vars[id].ty], emela_core::Type::Enum(enum_id, _) if program.core.enums[enum_id].external && program.core.enums[enum_id].name == "Std.Fiber.Fiber")
            {
                // A source-level discarded handle must not become a persistent frame owner.
                *destination = None;
            }
        }
    }
}
