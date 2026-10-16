//! Copying statements from one function into another, renaming variables and labels.

use rustc_hash::FxHashMap;

use crate::ir::{
    Callee, Case, Closure, Function, LabelId, OnFail, Operand, Seq, SeqId, Stmt, StmtId, Suspend,
    SwitchOn, Term, Value, VarId,
};

/// Copies sequences of `src` into another function. A variable or label is given a new ID the
/// first time it is seen; pre-seed [`Copier::vars`] to substitute variables.
pub(crate) struct Copier<'a> {
    pub src: &'a Function,
    pub vars: FxHashMap<VarId, VarId>,
    pub labels: FxHashMap<LabelId, LabelId>,
}

impl<'a> Copier<'a> {
    pub fn new(src: &'a Function) -> Self {
        Copier {
            src,
            vars: FxHashMap::default(),
            labels: FxHashMap::default(),
        }
    }

    pub fn var(&mut self, out: &mut Function, v: VarId) -> VarId {
        if let Some(&n) = self.vars.get(&v) {
            return n;
        }
        self.fresh(out, v)
    }

    /// A new variable in `out` like `v`, recorded as its copy (for a definition).
    pub fn fresh(&mut self, out: &mut Function, v: VarId) -> VarId {
        let n = out.vars.alloc(self.src.vars[v].clone());
        self.vars.insert(v, n);
        n
    }

    pub fn label(&mut self, out: &mut Function, l: LabelId) -> LabelId {
        if let Some(&n) = self.labels.get(&l) {
            return n;
        }
        let n = out.labels.alloc(self.src.labels[l].clone());
        self.labels.insert(l, n);
        n
    }

    pub fn operand(&mut self, out: &mut Function, o: &Operand) -> Operand {
        match o {
            Operand::Var(v) => Operand::Var(self.var(out, *v)),
            Operand::Const(c) => Operand::Const(c.clone()),
        }
    }

    pub fn operands(&mut self, out: &mut Function, os: &[Operand]) -> Vec<Operand> {
        os.iter().map(|o| self.operand(out, o)).collect()
    }

    pub fn vars_of(&mut self, out: &mut Function, vs: &[VarId]) -> Vec<VarId> {
        vs.iter().map(|&v| self.var(out, v)).collect()
    }

    /// A deep copy of the sequence.
    pub fn seq(&mut self, out: &mut Function, id: SeqId) -> SeqId {
        let src = self.src;
        let seq = &src.seqs[id];
        let stmts = seq.stmts.iter().map(|&s| self.stmt(out, s)).collect();
        let term = self.term(out, &seq.term);
        out.seqs.alloc(Seq { stmts, term })
    }

    pub fn term(&mut self, out: &mut Function, t: &Term) -> Term {
        match t {
            Term::Yield(v) => Term::Yield(v.as_ref().map(|v| self.operand(out, v))),
            Term::Return(v) => Term::Return(v.as_ref().map(|v| self.operand(out, v))),
            Term::Fail(v) => Term::Fail(self.operand(out, v)),
            Term::Break { label, value } => Term::Break {
                label: self.label(out, *label),
                value: value.as_ref().map(|v| self.operand(out, v)),
            },
            Term::Recur(args) => Term::Recur(self.operands(out, args)),
            Term::Defect(msg) => Term::Defect(self.operand(out, msg)),
            Term::Unwind => Term::Unwind,
            Term::Unreachable => Term::Unreachable,
        }
    }

    pub fn stmt(&mut self, out: &mut Function, id: StmtId) -> StmtId {
        let src = self.src;
        let stmt = match &src.stmts[id] {
            Stmt::Let {
                dst,
                value,
                on_defect,
                on_suspend,
            } => {
                let value = self.value(out, value);
                let on_defect = on_defect.map(|l| self.seq(out, l));
                let on_suspend = on_suspend.as_ref().map(|s| Suspend {
                    state: s.state,
                    saved: self.vars_of(out, &s.saved),
                });
                Stmt::Let {
                    dst: dst.map(|d| self.fresh(out, d)),
                    value,
                    on_defect,
                    on_suspend,
                }
            }
            Stmt::If {
                dst,
                cond,
                then,
                else_,
            } => {
                let cond = self.operand(out, cond);
                let then = self.seq(out, *then);
                let else_ = self.seq(out, *else_);
                Stmt::If {
                    dst: dst.map(|d| self.fresh(out, d)),
                    cond,
                    then,
                    else_,
                }
            }
            Stmt::Switch {
                dst,
                on,
                cases,
                default,
            } => {
                let on = match on {
                    SwitchOn::Tag(v) => SwitchOn::Tag(self.var(out, *v)),
                    SwitchOn::Lit(v) => SwitchOn::Lit(self.var(out, *v)),
                };
                let cases = cases
                    .iter()
                    .map(|c| Case {
                        key: c.key.clone(),
                        body: self.seq(out, c.body),
                    })
                    .collect();
                let default = default.map(|d| self.seq(out, d));
                Stmt::Switch {
                    dst: dst.map(|d| self.fresh(out, d)),
                    on,
                    cases,
                    default,
                }
            }
            Stmt::Loop {
                dst,
                vars,
                inits,
                body,
            } => {
                let inits = self.operands(out, inits);
                let vars = vars.iter().map(|&v| self.fresh(out, v)).collect();
                let body = self.seq(out, *body);
                Stmt::Loop {
                    dst: dst.map(|d| self.fresh(out, d)),
                    vars,
                    inits,
                    body,
                }
            }
            Stmt::Block { dst, label, body } => {
                // A new label, so that a sequence can be copied twice into one function.
                let label = {
                    let n = out.labels.alloc(src.labels[*label].clone());
                    self.labels.insert(*label, n);
                    n
                };
                let body = self.seq(out, *body);
                Stmt::Block {
                    dst: dst.map(|d| self.fresh(out, d)),
                    label,
                    body,
                }
            }
            Stmt::Dup(v) => Stmt::Dup(self.var(out, *v)),
            Stmt::Drop(v) => Stmt::Drop(self.var(out, *v)),
            Stmt::Protect { dst, body, cleanup } => {
                let body = self.seq(out, *body);
                let cleanup = self.seq(out, *cleanup);
                Stmt::Protect {
                    dst: dst.map(|d| self.fresh(out, d)),
                    body,
                    cleanup,
                }
            }
        };
        out.stmts.alloc(stmt)
    }

    pub fn on_fail(&mut self, out: &mut Function, on_fail: &Option<OnFail>) -> Option<OnFail> {
        on_fail.map(|o| match o {
            OnFail::Break(l) => OnFail::Break(self.label(out, l)),
            OnFail::Propagate => OnFail::Propagate,
            OnFail::Handle { error, body } => OnFail::Handle {
                error: self.fresh(out, error),
                body: self.seq(out, body),
            },
        })
    }

    pub fn callee(&mut self, out: &mut Function, callee: &Callee) -> Callee {
        match callee {
            Callee::Direct(f) => Callee::Direct(*f),
            Callee::Indirect(c) => Callee::Indirect(self.var(out, *c)),
            Callee::Op {
                handler,
                effect,
                op,
            } => Callee::Op {
                handler: self.var(out, *handler),
                effect: *effect,
                op: *op,
            },
        }
    }

    pub fn value(&mut self, out: &mut Function, v: &Value) -> Value {
        match v {
            Value::Const(c) => Value::Const(c.clone()),
            Value::Prim { op, ty, args } => Value::Prim {
                op: *op,
                ty: *ty,
                args: self.operands(out, args),
            },
            Value::Call {
                callee,
                args,
                suspends,
                on_fail,
            } => Value::Call {
                callee: self.callee(out, callee),
                args: self.operands(out, args),
                suspends: *suspends,
                on_fail: self.on_fail(out, on_fail),
            },
            Value::Builtin { op, args } => Value::Builtin {
                op: *op,
                args: self.operands(out, args),
            },
            Value::Make { ctor, args } => Value::Make {
                ctor: *ctor,
                args: self.operands(out, args),
            },
            Value::Field { base, ctor, index } => Value::Field {
                base: self.var(out, *base),
                ctor: *ctor,
                index: *index,
            },
            Value::Tag(v) => Value::Tag(self.var(out, *v)),
            Value::Tuple(es) => Value::Tuple(self.operands(out, es)),
            Value::TupleGet { base, index } => Value::TupleGet {
                base: self.var(out, *base),
                index: *index,
            },
            Value::List { elems, tail } => Value::List {
                elems: self.operands(out, elems),
                tail: tail.as_ref().map(|t| self.operand(out, t)),
            },
            Value::ListIsEmpty(v) => Value::ListIsEmpty(self.var(out, *v)),
            Value::ListHead(v) => Value::ListHead(self.var(out, *v)),
            Value::ListTail(v) => Value::ListTail(self.var(out, *v)),
            Value::FnRef(f) => Value::FnRef(*f),
            Value::Closure(c) => {
                let captures = self.vars_of(out, &c.captures);
                let params = c.params.iter().map(|&p| self.fresh(out, p)).collect();
                let body = self.seq(out, c.body);
                Value::Closure(Box::new(Closure {
                    params,
                    captures,
                    body,
                    suspends: c.suspends,
                }))
            }
            Value::MakeClosure { function, captures } => Value::MakeClosure {
                function: *function,
                captures: self.vars_of(out, captures),
            },
            Value::EnvGet { env, index } => Value::EnvGet {
                env: self.var(out, *env),
                index: *index,
            },
            Value::EvLookup { ev, effect } => Value::EvLookup {
                ev: self.var(out, *ev),
                effect: *effect,
            },
            Value::EvExtend { base, bindings } => Value::EvExtend {
                base: base.map(|b| self.var(out, b)),
                bindings: bindings
                    .iter()
                    .map(|&(e, h)| (e, self.var(out, h)))
                    .collect(),
            },
            Value::Handler { name, effect, ops } => Value::Handler {
                name: name.clone(),
                effect: *effect,
                ops: self.vars_of(out, ops),
            },
            Value::Fiber { op, args, on_fail } => Value::Fiber {
                op: *op,
                args: self.operands(out, args),
                on_fail: self.on_fail(out, on_fail),
            },
            Value::IsError { error, enum_id } => Value::IsError {
                error: self.var(out, *error),
                enum_id: *enum_id,
            },
            Value::AsError { error, enum_id } => Value::AsError {
                error: self.var(out, *error),
                enum_id: *enum_id,
            },
            Value::Runtime { op, args } => Value::Runtime {
                op: *op,
                args: self.operands(out, args),
            },
            Value::Coerce(v) => Value::Coerce(self.var(out, *v)),
        }
    }
}

/// A function like `f` without arenas, parameters or body.
pub(crate) fn empty_like(f: &Function) -> Function {
    let mut out = Function::new(&f.name);
    out.exported = f.exported;
    out.ret = f.ret;
    out.fails = f.fails;
    out.suspends = f.suspends;
    out.constant = f.constant;
    out.external = f.external.clone();
    out.env = f.env.clone();
    out.defects = f.defects;
    out
}

/// A copy of the function with only the variables, statements, sequences and labels its body
/// reaches, numbered in the order they are met.
pub(crate) fn compact(f: &Function) -> Function {
    let mut out = empty_like(f);
    let mut c = Copier::new(f);
    out.params = f.params.iter().map(|&p| c.fresh(&mut out, p)).collect();
    out.body = f.body.map(|b| c.seq(&mut out, b));
    out
}

/// A deep copy of the sequence inside its own function: the variables and labels it defines get
/// new IDs, and the ones defined outside it are kept (so the copy can stand where the original
/// stands, as a second landing of the same code).
pub(crate) fn clone_seq(f: &mut Function, id: SeqId) -> SeqId {
    let mut c = Cloner {
        vars: FxHashMap::default(),
        labels: FxHashMap::default(),
    };
    c.seq(f, id)
}

struct Cloner {
    vars: FxHashMap<VarId, VarId>,
    labels: FxHashMap<LabelId, LabelId>,
}

impl Cloner {
    fn use_(&self, v: VarId) -> VarId {
        self.vars.get(&v).copied().unwrap_or(v)
    }

    fn def(&mut self, f: &mut Function, v: VarId) -> VarId {
        let n = f.vars.alloc(f.vars[v].clone());
        self.vars.insert(v, n);
        n
    }

    fn operand(&self, o: &Operand) -> Operand {
        match o {
            Operand::Var(v) => Operand::Var(self.use_(*v)),
            Operand::Const(c) => Operand::Const(c.clone()),
        }
    }

    fn seq(&mut self, f: &mut Function, id: SeqId) -> SeqId {
        let seq = f.seqs[id].clone();
        let stmts = seq.stmts.iter().map(|&s| self.stmt(f, s)).collect();
        let term = self.term(&seq.term);
        f.seqs.alloc(Seq { stmts, term })
    }

    fn term(&self, t: &Term) -> Term {
        let opt = |v: &Option<Operand>| v.as_ref().map(|v| self.operand(v));
        match t {
            Term::Yield(v) => Term::Yield(opt(v)),
            Term::Return(v) => Term::Return(opt(v)),
            Term::Fail(v) => Term::Fail(self.operand(v)),
            Term::Break { label, value } => Term::Break {
                label: self.labels.get(label).copied().unwrap_or(*label),
                value: opt(value),
            },
            Term::Recur(args) => Term::Recur(args.iter().map(|a| self.operand(a)).collect()),
            Term::Defect(msg) => Term::Defect(self.operand(msg)),
            Term::Unwind => Term::Unwind,
            Term::Unreachable => Term::Unreachable,
        }
    }

    fn stmt(&mut self, f: &mut Function, id: StmtId) -> StmtId {
        let stmt = match f.stmts[id].clone() {
            Stmt::Let {
                dst,
                mut value,
                on_defect,
                on_suspend,
            } => {
                value.for_each_var_mut(|v| *v = self.use_(*v));
                if let Value::Call { on_fail, .. } | Value::Fiber { on_fail, .. } = &mut value
                    && let Some(OnFail::Handle { error, body }) = on_fail
                {
                    let e = self.def(f, *error);
                    let b = self.seq(f, *body);
                    *on_fail = Some(OnFail::Handle { error: e, body: b });
                }
                let on_suspend = on_suspend.map(|s| Suspend {
                    state: s.state,
                    saved: s.saved.iter().map(|&v| self.use_(v)).collect(),
                });
                let on_defect = on_defect.map(|l| self.seq(f, l));
                Stmt::Let {
                    dst: dst.map(|d| self.def(f, d)),
                    value,
                    on_defect,
                    on_suspend,
                }
            }
            Stmt::If {
                dst,
                cond,
                then,
                else_,
            } => {
                let cond = self.operand(&cond);
                let then = self.seq(f, then);
                let else_ = self.seq(f, else_);
                Stmt::If {
                    dst: dst.map(|d| self.def(f, d)),
                    cond,
                    then,
                    else_,
                }
            }
            Stmt::Switch {
                dst,
                on,
                cases,
                default,
            } => {
                let on = match on {
                    SwitchOn::Tag(v) => SwitchOn::Tag(self.use_(v)),
                    SwitchOn::Lit(v) => SwitchOn::Lit(self.use_(v)),
                };
                let cases = cases
                    .into_iter()
                    .map(|c| Case {
                        key: c.key,
                        body: self.seq(f, c.body),
                    })
                    .collect();
                let default = default.map(|d| self.seq(f, d));
                Stmt::Switch {
                    dst: dst.map(|d| self.def(f, d)),
                    on,
                    cases,
                    default,
                }
            }
            Stmt::Loop {
                dst,
                vars,
                inits,
                body,
            } => {
                let inits = inits.iter().map(|o| self.operand(o)).collect();
                let vars = vars.iter().map(|&v| self.def(f, v)).collect();
                let body = self.seq(f, body);
                Stmt::Loop {
                    dst: dst.map(|d| self.def(f, d)),
                    vars,
                    inits,
                    body,
                }
            }
            Stmt::Block { dst, label, body } => {
                let new = f.labels.alloc(f.labels[label].clone());
                self.labels.insert(label, new);
                let body = self.seq(f, body);
                Stmt::Block {
                    dst: dst.map(|d| self.def(f, d)),
                    label: new,
                    body,
                }
            }
            Stmt::Protect { dst, body, cleanup } => {
                let body = self.seq(f, body);
                let cleanup = self.seq(f, cleanup);
                Stmt::Protect {
                    dst: dst.map(|d| self.def(f, d)),
                    body,
                    cleanup,
                }
            }
            Stmt::Dup(v) => Stmt::Dup(self.use_(v)),
            Stmt::Drop(v) => Stmt::Drop(self.use_(v)),
        };
        f.stmts.alloc(stmt)
    }
}
