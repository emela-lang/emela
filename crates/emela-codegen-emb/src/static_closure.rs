use std::collections::{HashMap, HashSet};

use emela_core::Type;
use emela_core::TypeId;
use emela_lir::{
    Case, Closure, Const, FnId, Function, LabelId, Module, OnFail, Operand, Seq, SeqId, Stmt,
    StmtId, Suspend, SwitchOn, Term, Value, VarId,
};

use crate::{Error, ErrorKind};

#[derive(Default)]
struct BodySlice {
    sequences: HashSet<SeqId>,
    statements: HashSet<StmtId>,
    variables: HashSet<VarId>,
    labels: HashSet<LabelId>,
}

#[derive(Default)]
pub(crate) struct ClosureLowering {
    pub(crate) module: Option<Module>,
    pub(crate) functions: HashMap<FnId, usize>,
    pub(crate) families: HashMap<FnId, (FnId, usize)>,
    pub(crate) environments: Vec<ClosureEnvironment>,
}

#[derive(Clone)]
pub(crate) struct ClosureEnvironment {
    pub(crate) parent: FnId,
    pub(crate) function_type: TypeId,
    pub(crate) captures: Vec<CaptureBinding>,
}

#[derive(Clone)]
pub(crate) struct CaptureBinding {
    pub(crate) source: VarId,
    pub(crate) local: VarId,
    pub(crate) ty: TypeId,
}

pub(crate) fn lower(flow: &Module) -> Result<ClosureLowering, Vec<Error>> {
    let mut lowered = flow.clone();
    let mut result = ClosureLowering::default();
    let mut next_name = 0usize;
    let mut parent_ordinals: HashMap<FnId, usize> = HashMap::new();
    loop {
        let reachable = crate::emit::runtime_reachable(&lowered);
        let next = lowered.functions.iter().find_map(|(id, function)| {
            if !reachable.contains(&id) {
                return None;
            }
            let (positions, _) = crate::emit::function_positions(function);
            function.stmts.iter().find_map(|(statement, stmt)| {
                if !positions.contains_key(&statement) {
                    return None;
                }
                let Stmt::Let {
                    dst: Some(dst),
                    value: Value::Closure(closure),
                    ..
                } = stmt
                else {
                    return None;
                };
                Some((id, statement, *dst, (**closure).clone()))
            })
        });
        let Some((parent, statement, dst, closure)) = next else {
            break;
        };
        let parent_function = lowered.functions[parent].clone();
        let name = loop {
            let name = format!("{}$static_closure_{}", parent_function.name, next_name);
            next_name += 1;
            if lowered
                .functions
                .iter()
                .all(|(_, function)| function.name != name)
            {
                break name;
            }
        };
        let Type::Fn(function_type) = lowered.core.types[parent_function.vars[dst].ty].clone()
        else {
            return Err(invalid_flow_ir(
                &parent_function.name,
                "Closure destination is not a function value",
            ));
        };
        if parent_function.constant {
            return Err(unsupported(
                &parent_function.name,
                "closure in a const function",
            ));
        }

        let static_captures = static_capture_constants(&lowered, &parent_function, &closure);
        let (mut lifted, capture_locals) = lift_body(
            &parent_function,
            &closure,
            static_captures.as_deref().unwrap_or_default(),
            &name,
        )?;
        lifted.ret = function_type.ret;
        lifted.fails = function_type.fails;
        lifted.suspends = closure.suspends;
        let target = lowered.add(lifted);
        let ordinal = parent_ordinals.entry(parent).or_default();
        result.families.insert(target, (parent, *ordinal));
        *ordinal += 1;
        if static_captures.is_none() {
            let mut bindings = Vec::with_capacity(closure.captures.len());
            for &source in &closure.captures {
                let local = *capture_locals.get(&source).ok_or_else(|| {
                    invalid_flow_ir(
                        &parent_function.name,
                        "closure capture is absent from the lifted body",
                    )
                })?;
                let ty = parent_function.vars[source].ty;
                bindings.push(CaptureBinding { source, local, ty });
            }
            let environment = result.environments.len();
            result.functions.insert(target, environment);
            result.environments.push(ClosureEnvironment {
                parent,
                function_type: parent_function.vars[dst].ty,
                captures: bindings,
            });
        }
        if let Stmt::Let { value, .. } = &mut lowered.functions[parent].stmts[statement] {
            *value = Value::FnRef(target);
        }
    }

    result.module = Some(lowered);
    Ok(result)
}

fn static_capture_constants(
    module: &Module,
    parent: &Function,
    closure: &Closure,
) -> Option<Vec<(VarId, Const)>> {
    closure
        .captures
        .iter()
        .map(|&capture| {
            let value = parent
                .stmts
                .iter()
                .find_map(|(_, statement)| match statement {
                    Stmt::Let {
                        dst: Some(dst),
                        value: Value::Const(value),
                        ..
                    } if *dst == capture => Some(value.clone()),
                    _ => None,
                })?;
            let ty = parent.vars[capture].ty;
            let valid = match (&module.core.types[ty], &value) {
                (Type::Unit, Const::Unit)
                | (Type::Int, Const::Int(_))
                | (Type::Bool, Const::Bool(_))
                | (Type::Int64, Const::Int64(_))
                | (Type::Float, Const::Float(_))
                | (Type::String, Const::String(_)) => true,
                (Type::Enum(enum_id, _), Const::Int(_)) => {
                    let definition = &module.core.enums[*enum_id];
                    !definition.external
                        && definition
                            .variants
                            .iter()
                            .all(|variant| variant.tys.is_empty())
                }
                _ => false,
            };
            valid.then_some((capture, value))
        })
        .collect()
}

fn lift_body(
    parent: &Function,
    closure: &Closure,
    captures: &[(VarId, Const)],
    name: &str,
) -> Result<(Function, HashMap<VarId, VarId>), Vec<Error>> {
    let mut slice = BodySlice::default();
    visit_sequence(parent, closure.body, &mut slice, name)?;
    for &parameter in &closure.params {
        slice.variables.insert(parameter);
    }
    slice.variables.extend(closure.captures.iter().copied());

    let mut lifted = Function::new(name);
    let mut variable_map = HashMap::new();
    for (variable, data) in parent.vars.iter() {
        if slice.variables.contains(&variable) {
            let mapped = lifted.vars.alloc(data.clone());
            variable_map.insert(variable, mapped);
        }
    }
    lifted.params = closure
        .params
        .iter()
        .map(|parameter| mapped_id(&variable_map, *parameter, name))
        .collect::<Result<Vec<_>, _>>()?;

    let mut label_map = HashMap::new();
    for (label, data) in parent.labels.iter() {
        if slice.labels.contains(&label) {
            let mapped = lifted.labels.alloc(data.clone());
            label_map.insert(label, mapped);
        }
    }

    let mut sequence_map = HashMap::new();
    for (sequence, _) in parent.seqs.iter() {
        if slice.sequences.contains(&sequence) {
            let mapped = lifted.seqs.alloc(Seq {
                stmts: Vec::new(),
                term: Term::Unreachable,
            });
            sequence_map.insert(sequence, mapped);
        }
    }
    let mut statement_map = HashMap::new();
    for (statement, _) in parent.stmts.iter() {
        if slice.statements.contains(&statement) {
            let mapped = lifted.stmts.alloc(Stmt::Let {
                dst: None,
                value: Value::Const(Const::Unit),
                on_defect: None,
                on_suspend: None,
            });
            statement_map.insert(statement, mapped);
        }
    }

    for (old, new) in &label_map {
        lifted.labels[*new] = parent.labels[*old].clone();
    }
    for (old, new) in &statement_map {
        lifted.stmts[*new] = remap_statement(
            parent.stmts[*old].clone(),
            &variable_map,
            &label_map,
            &sequence_map,
            name,
        )?;
    }
    for (old, new) in &sequence_map {
        let sequence = &parent.seqs[*old];
        let statements = sequence
            .stmts
            .iter()
            .map(|statement| mapped_id(&statement_map, *statement, name))
            .collect::<Result<Vec<_>, _>>()?;
        lifted.seqs[*new] = Seq {
            stmts: statements,
            term: remap_term(sequence.term.clone(), &variable_map, &label_map, name)?,
        };
    }
    lifted.body = Some(mapped_id(&sequence_map, closure.body, name)?);
    let body = lifted.body.expect("lifted closure body was assigned");
    let mut initializers = Vec::new();
    for (capture, value) in captures {
        if matches!(value, Const::Unit) {
            continue;
        }
        let Some(&dst) = variable_map.get(capture) else {
            continue;
        };
        initializers.push(lifted.stmts.alloc(Stmt::Let {
            dst: Some(dst),
            value: Value::Const(value.clone()),
            on_defect: None,
            on_suspend: None,
        }));
    }
    initializers.append(&mut lifted.seqs[body].stmts);
    lifted.seqs[body].stmts = initializers;
    Ok((lifted, variable_map))
}

fn visit_sequence(
    function: &Function,
    sequence: SeqId,
    slice: &mut BodySlice,
    _owner: &str,
) -> Result<(), Vec<Error>> {
    if !slice.sequences.insert(sequence) {
        return Ok(());
    }
    let sequence_data = &function.seqs[sequence];
    for &statement_id in &sequence_data.stmts {
        if !slice.statements.insert(statement_id) {
            continue;
        }
        let statement = &function.stmts[statement_id];
        if let Some(dst) = statement.dst() {
            slice.variables.insert(dst);
        }
        match statement {
            Stmt::Let {
                value,
                on_defect,
                on_suspend,
                ..
            } => {
                if let Value::Closure(closure) = value {
                    slice.variables.extend(closure.params.iter().copied());
                    visit_sequence(function, closure.body, slice, _owner)?;
                }
                value.for_each_var(|variable| {
                    slice.variables.insert(variable);
                });
                if let Some(sequence) = on_defect {
                    visit_sequence(function, *sequence, slice, _owner)?;
                }
                if let Some(Suspend { saved, .. }) = on_suspend {
                    slice.variables.extend(saved.iter().copied());
                }
                if let Value::Call {
                    on_fail: Some(edge),
                    ..
                } = value
                {
                    match edge {
                        OnFail::Break(label) => {
                            slice.labels.insert(*label);
                        }
                        OnFail::Handle { error, body } => {
                            slice.variables.insert(*error);
                            visit_sequence(function, *body, slice, _owner)?;
                        }
                        OnFail::Propagate => {}
                    }
                }
            }
            Stmt::If {
                cond, then, else_, ..
            } => {
                add_operand(slice, cond);
                visit_sequence(function, *then, slice, _owner)?;
                visit_sequence(function, *else_, slice, _owner)?;
            }
            Stmt::Switch {
                on, cases, default, ..
            } => {
                slice.variables.insert(match on {
                    SwitchOn::Tag(variable) | SwitchOn::Lit(variable) => *variable,
                });
                for case in cases {
                    visit_sequence(function, case.body, slice, _owner)?;
                }
                if let Some(default) = default {
                    visit_sequence(function, *default, slice, _owner)?;
                }
            }
            Stmt::Loop {
                vars, inits, body, ..
            } => {
                slice.variables.extend(vars.iter().copied());
                for operand in inits {
                    add_operand(slice, operand);
                }
                visit_sequence(function, *body, slice, _owner)?;
            }
            Stmt::Block { label, body, .. } => {
                slice.labels.insert(*label);
                visit_sequence(function, *body, slice, _owner)?;
            }
            Stmt::Protect { body, cleanup, .. } => {
                visit_sequence(function, *body, slice, _owner)?;
                visit_sequence(function, *cleanup, slice, _owner)?;
            }
            Stmt::Dup(variable) | Stmt::Drop(variable) => {
                slice.variables.insert(*variable);
            }
        }
    }
    match &sequence_data.term {
        Term::Yield(value) | Term::Return(value) => {
            if let Some(value) = value {
                add_operand(slice, value);
            }
        }
        Term::Fail(value) | Term::Defect(value, _) => add_operand(slice, value),
        Term::Break { label, value } => {
            slice.labels.insert(*label);
            if let Some(value) = value {
                add_operand(slice, value);
            }
        }
        Term::Recur(values) => {
            for value in values {
                add_operand(slice, value);
            }
        }
        Term::Unwind | Term::Unreachable => {}
    }
    Ok(())
}

fn add_operand(slice: &mut BodySlice, operand: &Operand) {
    if let Operand::Var(variable) = operand {
        slice.variables.insert(*variable);
    }
}

fn remap_statement(
    statement: Stmt,
    variables: &HashMap<VarId, VarId>,
    labels: &HashMap<LabelId, LabelId>,
    sequences: &HashMap<SeqId, SeqId>,
    owner: &str,
) -> Result<Stmt, Vec<Error>> {
    Ok(match statement {
        Stmt::Let {
            dst,
            mut value,
            on_defect,
            on_suspend,
        } => {
            let mut missing = false;
            if let Value::Closure(closure) = &mut value {
                closure.params = closure
                    .params
                    .iter()
                    .map(|&parameter| mapped_id(variables, parameter, owner))
                    .collect::<Result<Vec<_>, _>>()?;
                closure.body = mapped_id(sequences, closure.body, owner)?;
            }
            value.for_each_var_mut(|variable| {
                if let Some(mapped) = variables.get(variable) {
                    *variable = *mapped;
                } else {
                    missing = true;
                }
            });
            if missing {
                return Err(invalid_flow_ir(
                    owner,
                    "closure body contains an unmapped variable",
                ));
            }
            if let Value::Call {
                on_fail: Some(edge),
                ..
            } = &mut value
            {
                match edge {
                    OnFail::Break(label) => *label = mapped_id(labels, *label, owner)?,
                    OnFail::Handle { error, body } => {
                        *error = mapped_id(variables, *error, owner)?;
                        *body = mapped_id(sequences, *body, owner)?;
                    }
                    OnFail::Propagate => {}
                }
            }
            Stmt::Let {
                dst: dst
                    .map(|dst| mapped_id(variables, dst, owner))
                    .transpose()?,
                value,
                on_defect: on_defect
                    .map(|sequence| mapped_id(sequences, sequence, owner))
                    .transpose()?,
                on_suspend: on_suspend
                    .map(|suspend| remap_suspend(suspend, variables, owner))
                    .transpose()?,
            }
        }
        Stmt::If {
            dst,
            cond,
            then,
            else_,
        } => Stmt::If {
            dst: dst
                .map(|dst| mapped_id(variables, dst, owner))
                .transpose()?,
            cond: remap_operand(cond, variables, owner)?,
            then: mapped_id(sequences, then, owner)?,
            else_: mapped_id(sequences, else_, owner)?,
        },
        Stmt::Switch {
            dst,
            on,
            cases,
            default,
        } => Stmt::Switch {
            dst: dst
                .map(|dst| mapped_id(variables, dst, owner))
                .transpose()?,
            on: match on {
                SwitchOn::Tag(variable) => SwitchOn::Tag(mapped_id(variables, variable, owner)?),
                SwitchOn::Lit(variable) => SwitchOn::Lit(mapped_id(variables, variable, owner)?),
            },
            cases: cases
                .into_iter()
                .map(|Case { key, body }| {
                    Ok(Case {
                        key,
                        body: mapped_id(sequences, body, owner)?,
                    })
                })
                .collect::<Result<Vec<_>, Vec<Error>>>()?,
            default: default
                .map(|sequence| mapped_id(sequences, sequence, owner))
                .transpose()?,
        },
        Stmt::Loop {
            dst,
            vars,
            inits,
            body,
        } => Stmt::Loop {
            dst: dst
                .map(|dst| mapped_id(variables, dst, owner))
                .transpose()?,
            vars: vars
                .into_iter()
                .map(|variable| mapped_id(variables, variable, owner))
                .collect::<Result<Vec<_>, _>>()?,
            inits: inits
                .into_iter()
                .map(|operand| remap_operand(operand, variables, owner))
                .collect::<Result<Vec<_>, _>>()?,
            body: mapped_id(sequences, body, owner)?,
        },
        Stmt::Block { dst, label, body } => Stmt::Block {
            dst: dst
                .map(|dst| mapped_id(variables, dst, owner))
                .transpose()?,
            label: mapped_id(labels, label, owner)?,
            body: mapped_id(sequences, body, owner)?,
        },
        Stmt::Protect { dst, body, cleanup } => Stmt::Protect {
            dst: dst
                .map(|dst| mapped_id(variables, dst, owner))
                .transpose()?,
            body: mapped_id(sequences, body, owner)?,
            cleanup: mapped_id(sequences, cleanup, owner)?,
        },
        Stmt::Dup(variable) => Stmt::Dup(mapped_id(variables, variable, owner)?),
        Stmt::Drop(variable) => Stmt::Drop(mapped_id(variables, variable, owner)?),
    })
}

fn remap_suspend(
    suspend: Suspend,
    variables: &HashMap<VarId, VarId>,
    owner: &str,
) -> Result<Suspend, Vec<Error>> {
    Ok(Suspend {
        state: suspend.state,
        saved: suspend
            .saved
            .into_iter()
            .map(|variable| mapped_id(variables, variable, owner))
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn remap_term(
    term: Term,
    variables: &HashMap<VarId, VarId>,
    labels: &HashMap<LabelId, LabelId>,
    owner: &str,
) -> Result<Term, Vec<Error>> {
    Ok(match term {
        Term::Yield(value) => Term::Yield(
            value
                .map(|value| remap_operand(value, variables, owner))
                .transpose()?,
        ),
        Term::Return(value) => Term::Return(
            value
                .map(|value| remap_operand(value, variables, owner))
                .transpose()?,
        ),
        Term::Fail(value) => Term::Fail(remap_operand(value, variables, owner)?),
        Term::Break { label, value } => Term::Break {
            label: mapped_id(labels, label, owner)?,
            value: value
                .map(|value| remap_operand(value, variables, owner))
                .transpose()?,
        },
        Term::Recur(values) => Term::Recur(
            values
                .into_iter()
                .map(|value| remap_operand(value, variables, owner))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Term::Defect(value, location) => {
            Term::Defect(remap_operand(value, variables, owner)?, location.clone())
        }
        Term::Unwind => Term::Unwind,
        Term::Unreachable => Term::Unreachable,
    })
}

fn remap_operand(
    operand: Operand,
    variables: &HashMap<VarId, VarId>,
    owner: &str,
) -> Result<Operand, Vec<Error>> {
    Ok(match operand {
        Operand::Var(variable) => Operand::Var(mapped_id(variables, variable, owner)?),
        Operand::Const(value) => Operand::Const(value),
    })
}

fn mapped_id<K: Copy + std::hash::Hash + Eq, V: Copy>(
    map: &HashMap<K, V>,
    id: K,
    owner: &str,
) -> Result<V, Vec<Error>> {
    map.get(&id)
        .copied()
        .ok_or_else(|| invalid_flow_ir(owner, "closure body references an unmapped Flow IR id"))
}

fn unsupported(function: &str, construct: &str) -> Vec<Error> {
    vec![Error {
        kind: ErrorKind::Unsupported,
        message: format!(
            "embedded C codegen does not support {construct} in function `{function}` yet"
        ),
    }]
}

fn invalid_flow_ir(function: &str, message: &str) -> Vec<Error> {
    vec![Error {
        kind: ErrorKind::InvalidFlowIr,
        message: format!("invalid Flow IR in function `{function}`: {message}"),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use emela_core::FnType;
    use emela_lir::FnBuilder;

    #[test]
    fn closure_families_include_static_siblings_and_nested_parents() {
        let mut core = emela_core::Module::new();
        let leaf = core
            .types
            .intern(Type::Fn(FnType::pure(vec![], TypeId::INT)));
        let nested = core.types.intern(Type::Fn(FnType::pure(vec![], leaf)));
        let mut b = FnBuilder::new(&core, "entry");
        b.set_exported().set_ret(TypeId::INT);
        let first = b.var("first", nested);
        b.closure(first, vec![], vec![], false, |b| {
            let child = b.var("child", leaf);
            b.closure(child, vec![], vec![], false, |b| {
                b.term(Term::Return(Some(Const::Int(42).into())))
            });
            b.term(Term::Return(Some(child.into())));
        });
        let second = b.var("second", leaf);
        b.closure(second, vec![], vec![], false, |b| {
            b.term(Term::Return(Some(Const::Int(42).into())))
        });
        b.term(Term::Return(Some(Const::Int(42).into())));
        let function = b.finish();
        let mut flow = Module::new(core);
        let entry = flow.add(function);
        let lowered = lower(&flow).unwrap();
        assert_eq!(lowered.families.len(), 3);
        assert!(
            lowered.functions.is_empty(),
            "static captures still have family metadata"
        );
        let first_target = lowered
            .families
            .iter()
            .find_map(|(&target, &(parent, ordinal))| {
                (parent == entry && ordinal == 0).then_some(target)
            })
            .unwrap();
        assert!(
            lowered
                .families
                .values()
                .any(|&(parent, ordinal)| parent == entry && ordinal == 1)
        );
        assert!(
            lowered
                .families
                .values()
                .any(|&(parent, ordinal)| parent == first_target && ordinal == 0)
        );
    }
}
