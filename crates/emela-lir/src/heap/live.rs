//! Liveness of every variable (all representations) over the structured control: a backward
//! analysis with a fixpoint at loops, like the one of reference counting (`heap::rc`), which
//! only follows counted variables.
//!
//! The state machine pass uses it to find what a resume point must save, and the verifier to
//! check the saved variables.

use rustc_hash::{FxHashMap, FxHashSet};

use crate::ir::Operand;
use crate::ir::{Function, LabelId, OnFail, SeqId, Stmt, StmtId, SwitchOn, Term, Value, VarId};

pub(crate) type Set = FxHashSet<VarId>;

/// The variables live on after each `let` of a function: live after it (its destination
/// excluded) on any way on, its failure and defect landings included.
pub(crate) fn live_on_lets(f: &Function) -> FxHashMap<StmtId, Set> {
    let mut l = Live {
        after_label: FxHashMap::default(),
        live_on: FxHashMap::default(),
    };
    if let Some(body) = f.body {
        l.seq(f, body, &Set::default(), None);
    }
    l.live_on
}

struct Live {
    after_label: FxHashMap<LabelId, Set>,
    live_on: FxHashMap<StmtId, Set>,
}

fn operands(os: &[Operand], out: &mut Set) {
    for o in os {
        if let Operand::Var(v) = o {
            out.insert(*v);
        }
    }
}

fn term_uses(t: &Term, out: &mut Set) {
    match t {
        Term::Yield(v) | Term::Return(v) | Term::Break { value: v, .. } => {
            operands(v.as_slice(), out)
        }
        Term::Fail(v) | Term::Defect(v) => operands(std::slice::from_ref(v), out),
        Term::Recur(args) => operands(args, out),
        Term::Unwind | Term::Unreachable => {}
    }
}

impl Live {
    /// The live variables at the start of the sequence. `yield_live`: live after the statement a
    /// `yield` goes to; `head`: live outer variables at the innermost loop head.
    fn seq(&mut self, f: &Function, id: SeqId, yield_live: &Set, head: Option<&Set>) -> Set {
        let seq = &f.seqs[id];
        let mut live: Set = match &seq.term {
            Term::Yield(_) => yield_live.clone(),
            Term::Break { label, .. } => self.after_label.get(label).cloned().unwrap_or_default(),
            Term::Recur(_) => head.cloned().unwrap_or_default(),
            _ => Set::default(),
        };
        term_uses(&seq.term, &mut live);
        for &s in seq.stmts.iter().rev() {
            live = self.stmt(f, s, live, yield_live, head);
        }
        live
    }

    fn stmt(
        &mut self,
        f: &Function,
        s: StmtId,
        mut after: Set,
        yield_live: &Set,
        head: Option<&Set>,
    ) -> Set {
        let stmt = &f.stmts[s];
        if let Some(d) = stmt.dst() {
            after.remove(&d);
        }
        match stmt {
            Stmt::Let {
                value, on_defect, ..
            } => {
                if let Value::Call {
                    on_fail: Some(OnFail::Handle { error, body }),
                    ..
                }
                | Value::Fiber {
                    on_fail: Some(OnFail::Handle { error, body }),
                    ..
                } = value
                {
                    let mut l = self.seq(f, *body, yield_live, head);
                    l.remove(error);
                    after.extend(l);
                }
                if let Some(l) = on_defect {
                    after.extend(self.seq(f, *l, yield_live, head));
                }
                self.live_on.insert(s, after.clone());
                value.for_each_var(|v| {
                    after.insert(v);
                });
                after
            }
            Stmt::Dup(v) | Stmt::Drop(v) => {
                after.insert(*v);
                after
            }
            Stmt::If {
                cond, then, else_, ..
            } => {
                let mut l = self.seq(f, *then, &after, head);
                l.extend(self.seq(f, *else_, &after, head));
                operands(std::slice::from_ref(cond), &mut l);
                l
            }
            Stmt::Switch {
                on, cases, default, ..
            } => {
                let mut l = Set::default();
                for c in cases {
                    l.extend(self.seq(f, c.body, &after, head));
                }
                if let Some(d) = default {
                    l.extend(self.seq(f, *d, &after, head));
                }
                let (SwitchOn::Tag(v) | SwitchOn::Lit(v)) = on;
                l.insert(*v);
                l
            }
            Stmt::Block { label, body, .. } => {
                self.after_label.insert(*label, after.clone());
                self.seq(f, *body, &after, head)
            }
            Stmt::Loop {
                vars, inits, body, ..
            } => {
                let mut h = Set::default();
                loop {
                    let mut l = self.seq(f, *body, &after, Some(&h));
                    for v in vars {
                        l.remove(v);
                    }
                    if l == h {
                        break;
                    }
                    h = l;
                }
                let mut l = h;
                operands(inits, &mut l);
                l
            }
            Stmt::Protect { body, cleanup, .. } => {
                // Flow IR only; the cleanup runs after the body however it leaves.
                let after_cleanup = self.seq(f, *cleanup, &after, head);
                let mut l = self.seq(f, *body, &after_cleanup, head);
                l.extend(after_cleanup);
                l
            }
        }
    }
}
