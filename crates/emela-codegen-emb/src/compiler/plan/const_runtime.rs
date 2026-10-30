use crate::contract::ir::{FnId, Program as Module};
use std::collections::HashSet;
pub(crate) fn empty_list_functions(module: &Module) -> HashSet<FnId> {
    use crate::contract::ir::Type;
    use crate::contract::ir::{Callee, Operand, Stmt, Term, Value};
    let mut result = HashSet::new();
    loop {
        let mut changed = false;
        for (id, function) in module.functions.iter() {
            if result.contains(&id)
                || function.external.is_some()
                || !matches!(module.core.types[function.ret], Type::List(_))
            {
                continue;
            }
            let mut empty = HashSet::new();
            loop {
                let mut local_changed = false;
                for (_, stmt) in function.stmts.iter() {
                    let Stmt::Let {
                        dst: Some(dst),
                        value,
                        ..
                    } = stmt
                    else {
                        continue;
                    };
                    let known = match value {
                        Value::List { elems, tail: None } => elems.is_empty(),
                        Value::List {
                            elems,
                            tail: Some(Operand::Var(tail)),
                        } => elems.is_empty() && empty.contains(tail),
                        Value::Call {
                            callee: Callee::Direct(target),
                            ..
                        } => result.contains(target),
                        Value::Coerce(source) => empty.contains(source),
                        _ => false,
                    };
                    if known {
                        local_changed |= empty.insert(*dst);
                    }
                }
                if !local_changed {
                    break;
                }
            }
            let returns: Vec<_> = function
                .seqs
                .iter()
                .filter_map(|(_, seq)| match &seq.term {
                    Term::Return(value) => Some(value),
                    _ => None,
                })
                .collect();
            if !returns.is_empty()
                && returns
                    .iter()
                    .all(|value| matches!(value,Some(Operand::Var(var)) if empty.contains(var)))
            {
                changed |= result.insert(id);
            }
        }
        if !changed {
            return result;
        }
    }
}
