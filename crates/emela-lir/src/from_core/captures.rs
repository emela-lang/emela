//! Capture lists of closures, computed after a function is built.
//!
//! A closure captures the variables its body uses but does not define, in first-use order. A
//! nested closure is done first, and its captures count as uses in the enclosing body, so a
//! closure between a variable's definition and its use captures it too (as the verifier wants).

use rustc_hash::FxHashSet;

use crate::ir::{Function, Operand, SeqId, Stmt, StmtId, Term, Value, VarId};

/// Fills in `Closure::captures` of every closure of the function.
pub(super) fn fill(f: &mut Function) {
    let Some(body) = f.body else {
        return;
    };
    let mut found = Vec::new();
    let mut body_uses = Uses::default();
    seq(f, body, &mut body_uses, &mut found);
    for (stmt, captures) in found {
        if let Stmt::Let {
            value: Value::Closure(c),
            ..
        } = &mut f.stmts[stmt]
        {
            c.captures = captures;
        }
    }
}

/// The variables a body defines and uses (each once, in first-use order).
#[derive(Default)]
struct Uses {
    defined: FxHashSet<VarId>,
    seen: FxHashSet<VarId>,
    used: Vec<VarId>,
}

impl Uses {
    fn use_(&mut self, v: VarId) {
        if self.seen.insert(v) {
            self.used.push(v);
        }
    }

    fn operand(&mut self, o: &Operand) {
        if let Operand::Var(v) = o {
            self.use_(*v);
        }
    }
}

fn seq(f: &Function, id: SeqId, uses: &mut Uses, found: &mut Vec<(StmtId, Vec<VarId>)>) {
    let s = &f.seqs[id];
    for &stmt in &s.stmts {
        match &f.stmts[stmt] {
            Stmt::Let { dst, value, .. } => {
                if let Value::Closure(c) = value {
                    let mut inner = Uses::default();
                    inner.defined.extend(c.params.iter().copied());
                    seq(f, c.body, &mut inner, found);
                    let captures: Vec<VarId> = inner
                        .used
                        .into_iter()
                        .filter(|v| !inner.defined.contains(v))
                        .collect();
                    for &v in &captures {
                        uses.use_(v);
                    }
                    found.push((stmt, captures));
                } else {
                    value.for_each_var(|v| uses.use_(v));
                }
                uses.defined.extend(*dst);
            }
            Stmt::If {
                dst,
                cond,
                then,
                else_,
            } => {
                uses.operand(cond);
                seq(f, *then, uses, found);
                seq(f, *else_, uses, found);
                uses.defined.extend(*dst);
            }
            Stmt::Switch {
                dst,
                on,
                cases,
                default,
            } => {
                let (crate::SwitchOn::Tag(v) | crate::SwitchOn::Lit(v)) = on;
                uses.use_(*v);
                for c in cases {
                    seq(f, c.body, uses, found);
                }
                if let Some(d) = default {
                    seq(f, *d, uses, found);
                }
                uses.defined.extend(*dst);
            }
            Stmt::Loop {
                dst,
                vars,
                inits,
                body,
            } => {
                inits.iter().for_each(|o| uses.operand(o));
                uses.defined.extend(vars.iter().copied());
                seq(f, *body, uses, found);
                uses.defined.extend(*dst);
            }
            Stmt::Block { dst, body, .. } => {
                seq(f, *body, uses, found);
                uses.defined.extend(*dst);
            }
            Stmt::Protect { dst, body, cleanup } => {
                seq(f, *body, uses, found);
                seq(f, *cleanup, uses, found);
                uses.defined.extend(*dst);
            }
        }
    }
    match &s.term {
        Term::Yield(v) | Term::Return(v) | Term::Break { value: v, .. } => {
            v.iter().for_each(|o| uses.operand(o));
        }
        Term::Fail(o) | Term::Defect(o) => uses.operand(o),
        Term::Recur(args) => args.iter().for_each(|o| uses.operand(o)),
        Term::Unwind | Term::Unreachable => {}
    }
}
