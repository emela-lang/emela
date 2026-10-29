use std::collections::{HashMap, HashSet};

use emela_core::Type;
use emela_lir::{Callee, Function, Module, Operand, Seq, Stmt, Term, Value, VarData};

pub(crate) fn lower(module: &mut Module, closure_functions: &HashSet<emela_lir::FnId>) {
    normalize_suspend_handlers(module);
    normalize_call_arguments(module);
    let reachable = crate::emit::runtime_reachable(module);
    let mut adapters = HashMap::new();
    let mut pending = Vec::new();
    for function_id in &reachable {
        let function = &module.functions[*function_id];
        for (statement, value) in function.stmts.iter() {
            let Stmt::Let {
                dst: Some(dst),
                value: Value::Coerce(source),
                ..
            } = value
            else {
                continue;
            };
            let (Type::Fn(from), Type::Fn(to)) = (
                module.core.types[function.vars[*source].ty].clone(),
                module.core.types[function.vars[*dst].ty].clone(),
            ) else {
                continue;
            };
            if from.fails
                || !to.fails
                || from.suspends
                || to.suspends
                || from.ev
                || to.ev
                || from.sets.is_some()
                || to.sets.is_some()
                || from.params != to.params
                || from.ret != to.ret
            {
                continue;
            }
            let target = function
                .stmts
                .iter()
                .find_map(|(_, statement)| match statement {
                    Stmt::Let {
                        dst: Some(variable),
                        value: Value::FnRef(target),
                        ..
                    } if *variable == *source => Some(*target),
                    _ => None,
                });
            if let Some(target) = target {
                if closure_functions.contains(&target) {
                    continue;
                }
                pending.push((*function_id, statement, target, function.vars[*dst].ty));
            }
        }
    }

    for (caller, statement, target, function_type) in pending {
        let adapter = if let Some(adapter) = adapters.get(&(target, function_type)) {
            *adapter
        } else {
            let target_function = module.functions[target].clone();
            if target_function.fails
                || target_function.suspends
                || target_function.constant
                || target_function.env.is_some()
            {
                continue;
            }
            let name = unique_name(module, &target_function.name, adapters.len());
            let adapter_function = make_adapter(&module.core, target, &target_function, &name);
            let adapter = module.add(adapter_function);
            adapters.insert((target, function_type), adapter);
            adapter
        };
        if let Stmt::Let { value, .. } = &mut module.functions[caller].stmts[statement] {
            *value = Value::FnRef(adapter);
        }
    }
}

fn normalize_call_arguments(module: &mut Module) {
    for id in crate::emit::runtime_reachable(module) {
        let mut inserted = HashMap::new();
        for (statement, stmt) in module.functions[id].stmts.clone().iter() {
            let Stmt::Let {
                value: Value::Call { callee, args, .. },
                ..
            } = stmt
            else {
                continue;
            };
            let expected = match callee {
                Callee::Direct(target) => module.functions[*target]
                    .params
                    .iter()
                    .map(|&p| module.functions[*target].vars[p].ty)
                    .collect::<Vec<_>>(),
                Callee::Indirect(target) => {
                    let Type::Fn(ft) = &module.core.types[module.functions[id].vars[*target].ty]
                    else {
                        continue;
                    };
                    ft.params
                        .iter()
                        .copied()
                        .chain(ft.ev.then_some(emela_core::TypeId::EVIDENCE))
                        .filter(|&ty| emela_lir::repr_of(&module.core, ty) != emela_lir::Repr::Void)
                        .collect()
                }
                _ => continue,
            };
            let f = &mut module.functions[id];
            let mut adapted = args.clone();
            for (index, (arg, expected)) in args.iter().zip(expected).enumerate() {
                let Operand::Var(source) = arg else {
                    continue;
                };
                let actual = f.vars[*source].ty;
                if actual == expected {
                    continue;
                }
                let (Type::Fn(from), Type::Fn(to)) =
                    (&module.core.types[actual], &module.core.types[expected])
                else {
                    continue;
                };
                // Flow permits function subtyping at a call boundary without a Coerce node.
                // Our distinct C records need the same adapter used by explicit coercions.
                if from.params != to.params || from.ret != to.ret {
                    continue;
                }
                let dst = f.vars.alloc(VarData {
                    hint: "call_argument".into(),
                    repr: emela_lir::Repr::Ref,
                    ty: expected,
                });
                let coercion = f.stmts.alloc(Stmt::Let {
                    dst: Some(dst),
                    value: Value::Coerce(*source),
                    on_defect: None,
                    on_suspend: None,
                });
                inserted
                    .entry(statement)
                    .or_insert_with(Vec::new)
                    .push(coercion);
                adapted[index] = dst.into();
            }
            if let Stmt::Let {
                value: Value::Call { args, .. },
                ..
            } = &mut f.stmts[statement]
            {
                *args = adapted;
            }
        }
        for (_, seq) in module.functions[id].seqs.iter_mut() {
            let mut statements = Vec::new();
            for statement in &seq.stmts {
                if let Some(before) = inserted.get(statement) {
                    statements.extend(before);
                }
                statements.push(*statement);
            }
            seq.stmts = statements;
        }
    }
}

fn normalize_suspend_handlers(module: &mut Module) {
    let reachable = crate::emit::runtime_reachable(module);
    for id in reachable {
        let mut inserted = HashMap::new();
        let function = &mut module.functions[id];
        for (statement, stmt) in function.stmts.clone().iter() {
            let Stmt::Let {
                value: Value::Handler { effect, ops, .. },
                ..
            } = stmt
            else {
                continue;
            };
            let mut adapted = ops.clone();
            for (index, &source) in ops.iter().enumerate() {
                if !module.core.effects[*effect].ops[index].suspend {
                    continue;
                }
                let Type::Fn(mut ft) = module.core.types[function.vars[source].ty].clone() else {
                    continue;
                };
                if ft.suspends {
                    continue;
                }
                ft.suspends = true;
                let ty = module.core.types.intern(Type::Fn(ft));
                let dst = function.vars.alloc(VarData {
                    hint: "suspend_operation".into(),
                    repr: emela_lir::Repr::Ref,
                    ty,
                });
                let coercion = function.stmts.alloc(Stmt::Let {
                    dst: Some(dst),
                    value: Value::Coerce(source),
                    on_defect: None,
                    on_suspend: None,
                });
                inserted
                    .entry(statement)
                    .or_insert_with(Vec::new)
                    .push(coercion);
                adapted[index] = dst;
            }
            if let Stmt::Let {
                value: Value::Handler { ops, .. },
                ..
            } = &mut function.stmts[statement]
            {
                *ops = adapted;
            }
        }
        for (_, seq) in function.seqs.iter_mut() {
            let mut statements = Vec::new();
            for statement in &seq.stmts {
                if let Some(before) = inserted.get(statement) {
                    statements.extend(before);
                }
                statements.push(*statement);
            }
            seq.stmts = statements;
        }
    }
}

fn make_adapter(
    core: &emela_core::Module,
    target_id: emela_lir::FnId,
    target: &Function,
    name: &str,
) -> Function {
    let mut adapter = Function::new(name);
    adapter.ret = target.ret;
    adapter.fails = true;
    adapter.params = target
        .params
        .iter()
        .map(|&parameter| adapter.vars.alloc(target.vars[parameter].clone()))
        .collect();

    let result = (emela_lir::repr_of(core, target.ret) != emela_lir::Repr::Void).then(|| {
        adapter.vars.alloc(VarData {
            hint: "coerced_result".to_owned(),
            repr: emela_lir::repr_of(core, target.ret),
            ty: target.ret,
        })
    });
    let call = adapter.stmts.alloc(Stmt::Let {
        dst: result,
        value: Value::Call {
            callee: Callee::Direct(target_id),
            args: adapter.params.iter().copied().map(Operand::Var).collect(),
            suspends: false,
            on_fail: None,
        },
        on_defect: None,
        on_suspend: None,
    });
    let body = adapter.seqs.alloc(Seq {
        stmts: vec![call],
        term: Term::Return(result.map(Operand::Var)),
    });
    adapter.body = Some(body);
    adapter
}

fn unique_name(module: &Module, target: &str, mut index: usize) -> String {
    loop {
        let name = format!("{target}$fail_adapter_{index}");
        if module
            .functions
            .iter()
            .all(|(_, function)| function.name != name)
        {
            return name;
        }
        index += 1;
    }
}
