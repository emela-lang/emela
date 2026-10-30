//! One fork-operation ABI for Scope instances with different error sets.
use std::collections::HashMap;

use emela_core::{Type, TypeId, TypeTable};
use emela_lir::{Callee, Function, Module, OnFail, SeqId, Stmt, Term, Value};

use crate::{Error, ErrorKind};

pub(crate) fn normalize(module: &mut Module) -> Result<(), Vec<Error>> {
    let mut reachable: Vec<_> = super::runtime_reachable(module).into_iter().collect();
    reachable.sort_by_key(|id| id.into_raw());
    let mut operations = HashMap::new();
    for &id in &reachable {
        let f = &module.functions[id];
        for (_, stmt) in f.stmts.iter() {
            let Stmt::Let {
                value: Value::Handler { effect, ops, .. },
                ..
            } = stmt
            else {
                continue;
            };
            if !module.core.effects[*effect]
                .name
                .starts_with("Std.Fiber.Scope[")
            {
                continue;
            }
            let mut types: Vec<_> = ops.iter().map(|v| f.vars[*v].ty).collect();
            if let Some(previous) = operations.get(effect) {
                let previous: &Vec<TypeId> = previous;
                for (ty, &other) in types.iter_mut().zip(previous) {
                    *ty = merge_operation(&mut module.core.types, *ty, other).ok_or_else(|| {
                        unsupported(&f.name, "Scope operations with different callback layouts")
                    })?;
                }
            }
            operations.insert(*effect, types);
        }
    }
    for id in reachable {
        let f = &mut module.functions[id];
        let mut pending = Vec::new();
        for (_, stmt) in f.stmts.iter() {
            let Stmt::Let {
                value: Value::Handler { effect, ops, .. },
                ..
            } = stmt
            else {
                continue;
            };
            let Some(expected) = operations.get(effect) else {
                continue;
            };
            pending.extend(
                ops.iter()
                    .zip(expected)
                    .filter_map(|(&v, &ty)| (f.vars[v].ty != ty).then_some((v, ty))),
            );
        }
        for (operation, ty) in pending {
            let closure = f
                .stmts
                .iter()
                .find_map(|(_, stmt)| match stmt {
                    Stmt::Let {
                        dst: Some(dst),
                        value: Value::Closure(c),
                        ..
                    } if *dst == operation => Some(c.clone()),
                    _ => None,
                })
                .ok_or_else(|| {
                    unsupported(&f.name, "Scope operation without its generated closure")
                })?;
            let Type::Fn(ft) = module.core.types[ty].clone() else {
                unreachable!()
            };
            for (&parameter, &expected) in closure.params.iter().zip(&ft.params) {
                f.vars[parameter].ty = expected;
            }
            f.vars[operation].ty = ty;
            if adapt_body(&mut module.core.types, f, closure.body)? && !ft.fails {
                return Err(unsupported(&f.name, "failing Scope operation body"));
            }
        }
    }
    Ok(())
}

fn merge_operation(types: &mut TypeTable, left: TypeId, right: TypeId) -> Option<TypeId> {
    if left == right {
        return Some(left);
    }
    let (Type::Fn(mut a), Type::Fn(b)) = (types[left].clone(), types[right].clone()) else {
        return None;
    };
    if a.params.len() != b.params.len() {
        return None;
    }
    for (arg, &other) in a.params.iter_mut().zip(&b.params) {
        if *arg == other {
            continue;
        }
        let (Type::Fn(mut x), Type::Fn(mut y)) = (types[*arg].clone(), types[other].clone()) else {
            return None;
        };
        let fails = x.fails || y.fails;
        x.fails = false;
        y.fails = false;
        if x != y {
            return None;
        }
        x.fails = fails;
        *arg = types.intern(Type::Fn(x));
    }
    let mut check = a.clone();
    check.params = b.params.clone();
    if check != b {
        return None;
    }
    Some(types.intern(Type::Fn(a)))
}

fn adapt_body(types: &mut TypeTable, f: &mut Function, seq: SeqId) -> Result<bool, Vec<Error>> {
    let mut fails = matches!(f.seqs[seq].term, Term::Fail(_));
    for id in f.seqs[seq].stmts.clone() {
        match f.stmts[id].clone() {
            Stmt::Let {
                dst: Some(dst),
                value: Value::Closure(c),
                ..
            } => {
                let nested = adapt_body(types, f, c.body)?;
                if nested {
                    let Type::Fn(mut ft) = types[f.vars[dst].ty].clone() else {
                        unreachable!()
                    };
                    ft.fails = true;
                    f.vars[dst].ty = types.intern(Type::Fn(ft));
                }
            }
            Stmt::Let {
                value:
                    Value::Call {
                        callee: Callee::Indirect(callee),
                        on_fail,
                        ..
                    },
                ..
            } => {
                if matches!(&types[f.vars[callee].ty], Type::Fn(ft) if ft.fails) {
                    if on_fail.is_none() {
                        let Stmt::Let {
                            value: Value::Call { on_fail, .. },
                            ..
                        } = &mut f.stmts[id]
                        else {
                            unreachable!()
                        };
                        *on_fail = Some(OnFail::Propagate);
                        fails = true;
                    } else {
                        fails |= on_fail == Some(OnFail::Propagate);
                    }
                }
            }
            Stmt::Let {
                value: Value::Coerce(source),
                dst: Some(dst),
                ..
            } => {
                // Scope's generated aliases keep the widened callback ABI inside the operation.
                if let (Type::Fn(from), Type::Fn(to)) =
                    (&types[f.vars[source].ty], &types[f.vars[dst].ty])
                {
                    let mut expected = to.clone();
                    expected.fails = from.fails;
                    if &expected == from {
                        f.vars[dst].ty = f.vars[source].ty;
                    }
                }
            }
            Stmt::If { then, else_, .. } => {
                fails |= adapt_body(types, f, then)?;
                fails |= adapt_body(types, f, else_)?;
            }
            Stmt::Switch { cases, default, .. } => {
                for case in cases {
                    fails |= adapt_body(types, f, case.body)?;
                }
                if let Some(body) = default {
                    fails |= adapt_body(types, f, body)?;
                }
            }
            Stmt::Block { body, .. } | Stmt::Loop { body, .. } => {
                fails |= adapt_body(types, f, body)?
            }
            Stmt::Protect { body, cleanup, .. } => {
                fails |= adapt_body(types, f, body)?;
                if adapt_body(types, f, cleanup)? {
                    return Err(unsupported(&f.name, "failing Scope operation cleanup"));
                }
            }
            _ => {}
        }
    }
    Ok(fails)
}

fn unsupported(function: &str, detail: &str) -> Vec<Error> {
    vec![Error {
        kind: ErrorKind::Unsupported,
        message: format!(
            "embedded C codegen does not support {detail} in function `{function}` yet"
        ),
    }]
}
