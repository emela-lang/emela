//! What the entry can reach.
//!
//! Reachability starts at the roots and follows direct calls and function references. A closure
//! built in reachable code is reachable wherever it is called from (passed to another function,
//! captured or aliased), except an operation of a handler value: its body is reachable only once
//! some reachable code performs that operation of that effect. So the unused `sleep` of
//! `SystemClock`, or of a handler that delegates `sleep` to an outer `Clock`, is not reached by a
//! program that only reads the time.
//!
//! The entry's reachable functions decide which calls the target does not support yet (the
//! suspending-call check) and which host interfaces the emitted component imports. For the std
//! hosts the target supports, this is the implementation of `[target.wasm.output]`: an interface
//! is imported only when the entry reaches a host function that uses it.

use emela_core::EffectId;
use emela_lir::{Callee, FnId, Function, Module, SeqId, Stmt, Value, VarId};
use rustc_hash::{FxHashMap, FxHashSet};

/// A sequence of statements in a function: a function body or a closure body nested in it.
type Body = (FnId, SeqId);

/// What the entry reaches.
#[derive(Debug, Default)]
pub(crate) struct Reach {
    /// The functions whose body (or, for an external function, call site) is reachable.
    functions: FxHashSet<FnId>,
    /// The operations of handlers some reachable code performs.
    performed: FxHashSet<(EffectId, usize)>,
    /// `suspend fn` external functions with no `wasm` binding that reachable code calls, in the
    /// order met.
    suspending: Vec<FnId>,
}

impl Reach {
    /// Whether reachable code can call `f`.
    pub(crate) fn contains(&self, f: FnId) -> bool {
        self.functions.contains(&f)
    }

    /// The `suspend fn` external functions with no `wasm` binding that reachable code calls.
    pub(crate) fn suspending(&self) -> &[FnId] {
        &self.suspending
    }
}

/// The functions, handler operations and unsupported suspending calls reachable from `roots`.
pub(crate) fn walk(m: &Module, roots: &[FnId]) -> Reach {
    let mut walk = Walk {
        m,
        reach: Reach::default(),
        handler_ops: FxHashMap::default(),
        seen: FxHashSet::default(),
        work: Vec::new(),
        waiting: FxHashMap::default(),
    };
    for &root in roots {
        walk.reach_fn(root);
    }
    while let Some((fid, seq)) = walk.work.pop() {
        walk.seq(fid, seq);
    }
    walk.reach
}

struct Walk<'m> {
    m: &'m Module,
    reach: Reach,
    /// Per function, the variables that hold an operation of a handler value it builds.
    handler_ops: FxHashMap<FnId, FxHashMap<VarId, (EffectId, usize)>>,
    seen: FxHashSet<Body>,
    work: Vec<Body>,
    /// The bodies of handler operations not performed yet.
    waiting: FxHashMap<(EffectId, usize), Vec<Body>>,
}

impl Walk<'_> {
    fn reach(&mut self, body: Body) {
        if self.seen.insert(body) {
            self.work.push(body);
        }
    }

    fn reach_fn(&mut self, fid: FnId) {
        self.reach.functions.insert(fid);
        if let Some(body) = self.m.functions[fid].body {
            self.reach((fid, body));
        }
    }

    fn perform(&mut self, op: (EffectId, usize)) {
        if self.reach.performed.insert(op) {
            for body in self.waiting.remove(&op).unwrap_or_default() {
                self.reach(body);
            }
        }
    }

    fn handler_op(&mut self, fid: FnId, var: VarId) -> Option<(EffectId, usize)> {
        let f = &self.m.functions[fid];
        self.handler_ops
            .entry(fid)
            .or_insert_with(|| handler_ops_of(f))
            .get(&var)
            .copied()
    }

    fn seq(&mut self, fid: FnId, seq: SeqId) {
        let f = &self.m.functions[fid];
        for &stmt in &f.seqs[seq].stmts {
            match &f.stmts[stmt] {
                Stmt::Let { dst, value, .. } => self.value(fid, *dst, value),
                Stmt::If { then, else_, .. } => {
                    self.seq(fid, *then);
                    self.seq(fid, *else_);
                }
                Stmt::Switch { cases, default, .. } => {
                    for case in cases {
                        self.seq(fid, case.body);
                    }
                    if let Some(default) = default {
                        self.seq(fid, *default);
                    }
                }
                Stmt::Loop { body, .. } | Stmt::Block { body, .. } => self.seq(fid, *body),
                Stmt::Protect { body, cleanup, .. } => {
                    self.seq(fid, *body);
                    self.seq(fid, *cleanup);
                }
                Stmt::Dup(_) | Stmt::Drop(_) => {}
            }
        }
    }

    fn value(&mut self, fid: FnId, dst: Option<VarId>, value: &Value) {
        let m = self.m;
        match value {
            Value::Closure(closure) => {
                let body = (fid, closure.body);
                match dst.and_then(|dst| self.handler_op(fid, dst)) {
                    Some(op) if !self.reach.performed.contains(&op) => {
                        self.waiting.entry(op).or_default().push(body);
                    }
                    _ => self.reach(body),
                }
            }
            Value::FnRef(callee) => self.reach_fn(*callee),
            Value::Call { callee, .. } => match callee {
                Callee::Direct(callee) => {
                    let g = &m.functions[*callee];
                    // An external with a `wasm` binding is already reported once by
                    // `check/external.rs`, so it is not named here again.
                    if g.suspends
                        && g.external
                            .as_ref()
                            .is_some_and(|e| e.binding("wasm").is_none())
                        && !self.reach.suspending.contains(callee)
                    {
                        self.reach.suspending.push(*callee);
                    }
                    self.reach_fn(*callee);
                }
                Callee::Op { effect, op, .. } => self.perform((*effect, *op)),
                Callee::Indirect(_) => {}
            },
            _ => {}
        }
    }
}

/// The variables of `f` that a handler value takes as one of its operations.
fn handler_ops_of(f: &Function) -> FxHashMap<VarId, (EffectId, usize)> {
    let mut ops = FxHashMap::default();
    for (_, stmt) in f.stmts.iter() {
        if let Stmt::Let {
            value: Value::Handler {
                effect, ops: vars, ..
            },
            ..
        } = stmt
        {
            for (op, &var) in vars.iter().enumerate() {
                ops.insert(var, (*effect, op));
            }
        }
    }
    ops
}
