//! Lazy const preparation and heap-independent diagnostic storage.

use std::collections::HashSet;

use emela_lir::{FnId, Module};

use crate::{Error, ErrorKind, Options, const_eval, writer::Writer};

pub(crate) fn prepare(
    module: &mut Module,
    reachable: fn(&Module) -> HashSet<FnId>,
) -> HashSet<FnId> {
    let mut runtime = HashSet::new();
    loop {
        let newly_runtime: Vec<_> = reachable(module)
            .into_iter()
            .filter(|&id| {
                module.functions[id].constant && const_eval::evaluate(module, id).is_err()
            })
            .collect();
        if newly_runtime.is_empty() {
            return runtime;
        }
        for id in newly_runtime {
            module.functions[id].constant = false;
            runtime.insert(id);
        }
    }
}

pub(crate) fn emit_runtime(
    module: &Module,
    runtime: &HashSet<FnId>,
    options: &Options,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let prefix_bytes: usize = runtime
        .iter()
        .map(|&id| "while evaluating const : ".len() + module.functions[id].name.len())
        .sum();
    let emergency = prefix_bytes + 64;
    if emergency > usize::from(u16::MAX) || options.const_diagnostic_bytes == 0 {
        return Err(vec![Error {
            kind: ErrorKind::InvalidOptions,
            message: "runtime const diagnostics require a nonzero diagnostic capacity and const context names fitting uint16_t".into(),
        }]);
    }
    writer.line(&format!(
        "#define EMELA_CONST_DIAGNOSTIC_BYTES UINT32_C({})",
        options.const_diagnostic_bytes
    ));
    writer.line(&format!(
        "#define EMELA_CONST_EMERGENCY_BYTES {}",
        emergency.max(1)
    ));
    writer.line(include_str!("runtime/const.c"));
    Ok(())
}

pub(crate) fn empty_list_functions(module: &Module) -> HashSet<FnId> {
    use emela_core::Type;
    use emela_lir::{Callee, Operand, Stmt, Term, Value};
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
