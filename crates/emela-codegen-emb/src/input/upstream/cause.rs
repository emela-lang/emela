use emela_core::{FiberOp, Type, TypeId, TypeTable};
use emela_lir::{Module, Stmt, Term, Value};

// A CauseRaise in a body that cannot fail carries no failure edge (`fails: false`), or a
// conservative Propagate when the monomorphic error set is empty. The runtime still checks the
// cause's kind, so both get a local landing. Keep compile_flow strict; adapt only Core output
// after Core's own monomorphic verification.
pub(crate) fn normalize_infallible_cause_raise(module: &mut Module) {
    fn visit(
        types: &TypeTable,
        f: &mut emela_lir::Function,
        seq: emela_lir::SeqId,
        fails: bool,
        landings: &mut Vec<(emela_lir::SeqId, emela_lir::VarId)>,
    ) {
        let statements = f.seqs[seq].stmts.clone();
        for (position, id) in statements.into_iter().enumerate() {
            let stmt = f.stmts[id].clone();
            match stmt {
                Stmt::Let {
                    dst,
                    value: Value::Closure(closure),
                    ..
                } => {
                    let Some(dst) = dst else {
                        continue;
                    };
                    let Type::Fn(ft) = &types[f.vars[dst].ty] else {
                        continue;
                    };
                    visit(types, f, closure.body, ft.fails, landings);
                }
                Stmt::Let {
                    dst: None,
                    value:
                        Value::Fiber {
                            op: FiberOp::CauseRaise { .. },
                            on_fail: None | Some(emela_lir::OnFail::Propagate),
                            ..
                        },
                    ..
                } if !fails => {
                    let error = f.vars.alloc(emela_lir::VarData {
                        hint: "empty_cause_failure".into(),
                        ty: TypeId::ANY,
                        repr: emela_lir::Repr::Ref,
                    });
                    let label = f.labels.alloc(emela_lir::LabelData {
                        hint: "empty_cause_failure".into(),
                    });
                    if let Stmt::Let {
                        value: Value::Fiber { op, on_fail, .. },
                        ..
                    } = &mut f.stmts[id]
                    {
                        *op = FiberOp::CauseRaise { fails: true };
                        *on_fail = Some(emela_lir::OnFail::Break(label));
                    }
                    let body = f.seqs.alloc(emela_lir::Seq {
                        stmts: vec![id],
                        term: Term::Unreachable,
                    });
                    let block = f.stmts.alloc(Stmt::Block {
                        dst: Some(error),
                        label,
                        body,
                    });
                    let body = f.seqs.alloc(emela_lir::Seq {
                        stmts: vec![block],
                        term: Term::Unreachable,
                    });
                    landings.push((body, error));
                    let label = f.labels.alloc(emela_lir::LabelData {
                        hint: "raise_empty_cause".into(),
                    });
                    f.seqs[seq].stmts[position] = f.stmts.alloc(Stmt::Block {
                        dst: None,
                        label,
                        body,
                    });
                }
                Stmt::If { then, else_, .. } => {
                    visit(types, f, then, fails, landings);
                    visit(types, f, else_, fails, landings);
                }
                Stmt::Switch { cases, default, .. } => {
                    for case in cases {
                        visit(types, f, case.body, fails, landings);
                    }
                    if let Some(seq) = default {
                        visit(types, f, seq, fails, landings);
                    }
                }
                Stmt::Block { body, .. } | Stmt::Loop { body, .. } => {
                    visit(types, f, body, fails, landings)
                }
                Stmt::Protect { body, cleanup, .. } => {
                    visit(types, f, body, fails, landings);
                    visit(types, f, cleanup, false, landings);
                }
                _ => {}
            }
        }
    }
    let mut pending = Vec::new();
    for (id, f) in module.functions.iter_mut() {
        if let Some(body) = f.body {
            let mut landings = Vec::new();
            visit(&module.core.types, f, body, f.fails, &mut landings);
            pending.extend(landings.into_iter().map(|(seq, error)| (id, seq, error)));
        }
    }
    if pending.is_empty() {
        return;
    }
    // This backend-owned binding is introduced only after Core's monomorphic verification.
    // A local Break keeps normal Flow verification in force for the conservative fails edge.
    let mut helper = emela_lir::FnBuilder::new(&module.core, INFALLIBLE_CAUSE_FAIL);
    helper.param("error", TypeId::ANY);
    helper.set_ret(TypeId::NEVER);
    let helper = module.add(helper.finish_external(emela_core::External {
        bindings: vec![emela_core::ExternalBinding {
            target: INTERNAL_TARGET.into(),
            host: emela_core::HostRef::Global(INFALLIBLE_CAUSE_FAIL.into()),
        }],
    }));
    for (id, seq, error) in pending {
        let f = &mut module.functions[id];
        let call = f.stmts.alloc(Stmt::Let {
            dst: None,
            value: Value::Call {
                callee: emela_lir::Callee::Direct(helper),
                args: vec![error.into()],
                suspends: false,
                on_fail: None,
            },
            on_defect: None,
            on_suspend: None,
        });
        f.seqs[seq].stmts.push(call);
    }
}

const INTERNAL_TARGET: &str = "emela_emb_internal";
pub(crate) const INFALLIBLE_CAUSE_FAIL: &str = "emela_infallible_cause_fail";

pub(crate) fn is_infallible_cause_fail(function: &emela_lir::Function) -> bool {
    function.name == INFALLIBLE_CAUSE_FAIL && function.external.as_ref()
        .and_then(|external| external.binding(INTERNAL_TARGET))
        .is_some_and(|binding| matches!(&binding.host, emela_core::HostRef::Global(name) if name == INFALLIBLE_CAUSE_FAIL))
}
