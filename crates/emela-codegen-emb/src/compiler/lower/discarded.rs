//! Materialize discarded runtime results without retaining them until function return.
use emela_core::{Builtin, Sig, Type, TypeId};
use emela_lir::{Const, FnId, Function, Module, Operand, RuntimeOp, Stmt, Value, VarData, VarId};
use std::collections::{HashMap, HashSet};

// Function names need not be unique in a caller-supplied verified Flow module.
pub(crate) type Results = HashMap<FnId, HashSet<VarId>>;

pub(crate) fn normalize(module: &mut Module) -> Results {
    let mut discarded = Results::new();
    for (function_id, function) in module.functions.iter_mut() {
        let (positions, _) = super::function_positions(function);
        let statements: Vec<_> = function
            .stmts
            .iter()
            .filter_map(|(id, _)| positions.contains_key(&id).then_some(id))
            .collect();
        for statement_id in statements {
            let Stmt::Let {
                dst: None, value, ..
            } = &function.stmts[statement_id]
            else {
                continue;
            };
            let Some(ty) = result_type(&mut module.core, function, value) else {
                continue;
            };
            let repr = match module.core.types[ty] {
                Type::Int | Type::Bool => emela_lir::Repr::I32,
                Type::Int64 => emela_lir::Repr::I64,
                Type::Float => emela_lir::Repr::F64,
                Type::Unit | Type::Never => emela_lir::Repr::Void,
                _ => emela_lir::Repr::Ref,
            };
            if repr == emela_lir::Repr::Void {
                continue;
            }
            let variable = function.vars.alloc(VarData {
                hint: "discarded_result".into(),
                ty,
                repr,
            });
            let Stmt::Let { dst, .. } = &mut function.stmts[statement_id] else {
                unreachable!()
            };
            *dst = Some(variable);
            discarded.entry(function_id).or_default().insert(variable);
        }
    }
    discarded
}

fn result_type(
    core: &mut emela_core::Module,
    function: &Function,
    value: &Value,
) -> Option<TypeId> {
    match value {
        Value::EvLookup { effect, .. } => Some(core.handler(*effect)),
        Value::EvExtend { .. } => Some(TypeId::EVIDENCE),
        Value::Handler { effect, .. } => Some(core.handler(*effect)),
        Value::List { elems, tail } => {
            if let Some(tail) = tail {
                let ty = operand_type(function, tail);
                if matches!(core.types[ty], Type::List(_)) {
                    return Some(ty);
                }
                return None;
            }
            let element = elems
                .first()
                .map_or(TypeId::UNIT, |value| operand_type(function, value));
            Some(core.list(element))
        }
        Value::Builtin {
            op:
                Builtin::CellNew
                | Builtin::CellGet
                | Builtin::CellSet
                | Builtin::Panic
                | Builtin::Todo
                | Builtin::AssertFail
                | Builtin::Dbg,
            ..
        } => None,
        Value::Builtin { op, .. } => signature_type(core, op.info().ret),
        Value::Runtime {
            op: RuntimeOp::ShowString,
            ..
        } => Some(TypeId::STRING),
        _ => None,
    }
}

fn signature_type(core: &mut emela_core::Module, sig: Sig) -> Option<TypeId> {
    Some(match sig {
        Sig::Int => TypeId::INT,
        Sig::Int64 => TypeId::INT64,
        Sig::Float => TypeId::FLOAT,
        Sig::Bool => TypeId::BOOL,
        Sig::String => TypeId::STRING,
        Sig::Bytes => TypeId::BYTES,
        Sig::Unit => TypeId::UNIT,
        Sig::Never => TypeId::NEVER,
        Sig::List(element) => {
            let element = signature_type(core, *element)?;
            core.types.intern(Type::List(element))
        }
        Sig::Option(payload) => {
            let payload = signature_type(core, *payload)?;
            let option = core.option_enum();
            core.enum_type(option, vec![payload])
        }
        Sig::Param(_) => return None,
    })
}

fn operand_type(function: &Function, operand: &Operand) -> TypeId {
    match operand {
        Operand::Var(value) => function.vars[*value].ty,
        Operand::Const(value) => match value {
            Const::Int(_) => TypeId::INT,
            Const::Int64(_) => TypeId::INT64,
            Const::Float(_) => TypeId::FLOAT,
            Const::Bool(_) => TypeId::BOOL,
            Const::String(_) => TypeId::STRING,
            Const::Unit => TypeId::UNIT,
        },
    }
}
