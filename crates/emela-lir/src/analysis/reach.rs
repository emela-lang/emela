//! What the entry can reach.
//!
//! Reachability starts at the roots and follows direct calls and function references. A closure
//! built in reachable code is reachable wherever it is called from (passed to another function,
//! captured or aliased), except an operation of a handler value: its body is reachable only once
//! some reachable code performs that operation of that effect. So the unused `sleep` of
//! `SystemClock`, or of a handler that delegates `sleep` to an outer `Clock`, is not reached by a
//! program that only reads the time.
//!
//! The analysis works on both LIR stages:
//!
//! - The Flow IR keeps a closure body nested in the function that builds it ([`Value::Closure`]),
//!   so a body is a `(FnId, SeqId)` pair.
//! - The Heap IR lifts a closure body into its own function ([`Value::MakeClosure`]), so the same
//!   value names a [`FnId`] directly.
//!
//! Two sets are kept apart, because a reachable handler constructor builds a closure for every
//! operation, whether the operation is performed or not:
//!
//! - [`Reachability::is_called`]: the function's body can run. A backend emits its body.
//! - [`Reachability::is_referenced`]: a reachable value names the function (a `MakeClosure`, a
//!   `FnRef`, or a handler operation that was built but never performed). A backend keeps the
//!   function's index and emits a stub body.
//!
//! A handler operation's closure value need not be the direct `MakeClosure` destination: it can
//! pass through a copy or a temporary (a [`Value::Coerce`], an `if` arm). The analysis follows the
//! variable to the value that produces the closure, so an operation that is built but never
//! performed is `referenced` and not `called` whichever shape the value has. This is what the
//! WASM backend previously got wrong when it walked the Heap IR with a map of operation variables
//! to `MakeClosure` destinations: an unperformed operation body was emitted while the Flow-IR
//! walk used for imports pruned it. Both decisions now read this one analysis.
//!
//! The entry's reachable functions decide which calls the target does not support yet (the
//! suspending-call check) and which host interfaces the emitted component imports. The
//! target-specific policy stays in the backend: [`Reachability::externals`] is every reachable
//! external function in first-met order, and a backend filters it by its bindings.

use emela_core::EffectId;
use la_arena::Arena;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::ir::{Callee, FnId, Function, Module, Operand, SeqId, Stmt, Term, Value, VarId};

/// A sequence of statements in a function: a function body or a closure body nested in it.
type Body = (FnId, SeqId);

/// An operation of an effect: the operation index into `EffectDef::ops`.
type Op = (EffectId, usize);

/// A closure body deferred until some reachable code performs its handler operation.
enum Waiting {
    /// A body nested in `f`: the Flow IR keeps a closure body in the arena of the function that
    /// builds it.
    Body(Body),
    /// The function of a lifted closure: the Heap IR makes a closure body its own function.
    Function(FnId),
}

/// What the entry reaches.
#[derive(Debug, Default)]
pub struct Reachability {
    /// The functions whose body can run.
    called: FxHashSet<FnId>,
    /// The functions whose index or address a reachable value names, including the functions of
    /// handler operations that are built but never performed.
    referenced: FxHashSet<FnId>,
    /// The operations of handlers some reachable code performs.
    performed: FxHashSet<Op>,
    /// The reachable external functions, in the order first met (the backend filters by target).
    externals: Vec<FnId>,
}

impl Reachability {
    /// The functions, handler operations and external calls reachable from `roots`.
    ///
    /// The error `show` functions of [`Module::error_shows`] and every const function of the
    /// module are roots too: a backend evaluates a const eagerly and names an error's `show`
    /// function from its descriptor, so neither is called from the entry.
    pub fn walk(m: &Module, roots: &[FnId]) -> Self {
        let mut walk = Walk {
            m,
            reach: Reachability::default(),
            handler_ops: FxHashMap::default(),
            seen: FxHashSet::default(),
            work: Vec::new(),
            waiting: FxHashMap::default(),
        };
        for &root in roots {
            walk.reach_fn(root);
        }
        for &(_, show) in &m.error_shows {
            walk.reach_fn(show);
        }
        for (id, f) in m.functions.iter() {
            if f.constant {
                walk.reach_fn(id);
            }
        }
        while let Some((fid, seq)) = walk.work.pop() {
            walk.seq(fid, seq);
        }
        walk.reach
    }

    /// Whether reachable code can run the body of `f`.
    pub fn is_called(&self, f: FnId) -> bool {
        self.called.contains(&f)
    }

    /// Whether a reachable value names `f` (even if its body cannot run).
    pub fn is_referenced(&self, f: FnId) -> bool {
        self.referenced.contains(&f)
    }

    /// The functions whose body can run.
    pub fn called(&self) -> impl Iterator<Item = FnId> + '_ {
        self.called.iter().copied()
    }

    /// The operations of handlers some reachable code performs.
    pub fn performed(&self) -> &FxHashSet<Op> {
        &self.performed
    }

    /// The reachable external functions, in the order first met.
    pub fn externals(&self) -> &[FnId] {
        &self.externals
    }
}

struct Walk<'m> {
    m: &'m Module,
    reach: Reachability,
    /// Per function, the variables that produce the closure of an operation of a handler value it
    /// builds. The key is the variable of the `let` whose value makes the closure, after following
    /// a copy or temporary back to it, not necessarily the operation slot itself.
    handler_ops: FxHashMap<FnId, FxHashMap<VarId, Vec<Op>>>,
    seen: FxHashSet<Body>,
    work: Vec<Body>,
    /// The bodies of handler operations not performed yet.
    waiting: FxHashMap<Op, Vec<Waiting>>,
}

impl Walk<'_> {
    fn reach_body(&mut self, body: Body) {
        if self.seen.insert(body) {
            self.work.push(body);
        }
    }

    /// A function whose body can run: a root, a direct callee, or the function of a closure that
    /// some reachable code may call. Its index is also referenced.
    fn reach_fn(&mut self, fid: FnId) {
        self.reach.referenced.insert(fid);
        if !self.reach.called.insert(fid) {
            return;
        }
        let f = &self.m.functions[fid];
        if f.external.is_some() {
            self.reach.externals.push(fid);
        }
        if let Some(body) = f.body {
            self.reach_body((fid, body));
        }
    }

    /// A function whose index a reachable value names but whose body cannot run (yet).
    fn reference_fn(&mut self, fid: FnId) {
        self.reach.referenced.insert(fid);
    }

    fn perform(&mut self, op: Op) {
        if self.reach.performed.insert(op) {
            for waiting in self.waiting.remove(&op).unwrap_or_default() {
                match waiting {
                    Waiting::Body(body) => self.reach_body(body),
                    Waiting::Function(fid) => self.reach_fn(fid),
                }
            }
        }
    }

    /// The operations whose closure `var` produces, if `var` is an operation slot of a handler
    /// value of `f`.
    fn handler_op(&mut self, fid: FnId, var: VarId) -> Option<Vec<Op>> {
        let f = &self.m.functions[fid];
        self.handler_ops
            .entry(fid)
            .or_insert_with(|| handler_ops_of(f))
            .get(&var)
            .cloned()
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
        match value {
            Value::Closure(closure) => {
                let body = (fid, closure.body);
                match dst.and_then(|dst| self.handler_op(fid, dst)) {
                    Some(ops) => self.defer_or_reach_body(body, &ops),
                    None => self.reach_body(body),
                }
            }
            // The Heap IR lifts a closure body into its own function (`heap::convert_closures`),
            // so a walk over that stage reaches the function rather than a nested body.
            Value::MakeClosure { function, .. } => {
                let function = *function;
                match dst.and_then(|dst| self.handler_op(fid, dst)) {
                    Some(ops) => self.defer_or_reach_fn(function, &ops),
                    None => self.reach_fn(function),
                }
            }
            Value::FnRef(callee) => {
                let callee = *callee;
                match dst.and_then(|dst| self.handler_op(fid, dst)) {
                    Some(ops) => self.defer_or_reach_fn(callee, &ops),
                    None => self.reach_fn(callee),
                }
            }
            Value::Call { callee, .. } => match callee {
                Callee::Direct(callee) => self.reach_fn(*callee),
                Callee::Op { effect, op, .. } => self.perform((*effect, *op)),
                Callee::Indirect(_) => {}
            },
            // A handler value itself is not visited: only its operations are, each when some
            // reachable code performs it. Evidence tables identify handler values, not calls.
            _ => {}
        }
    }

    /// A nested closure body that is one of `ops`: reached if some operation is already performed,
    /// deferred until one is otherwise.
    fn defer_or_reach_body(&mut self, body: Body, ops: &[Op]) {
        let mut pending = Vec::new();
        for &op in ops {
            if self.reach.performed.contains(&op) {
                self.reach_body(body);
                return;
            }
            pending.push(op);
        }
        for op in pending {
            self.waiting
                .entry(op)
                .or_default()
                .push(Waiting::Body(body));
        }
    }

    /// A lifted closure function that is one of `ops`, as [`Self::defer_or_reach_body`].
    fn defer_or_reach_fn(&mut self, fid: FnId, ops: &[Op]) {
        let mut pending = Vec::new();
        for &op in ops {
            if self.reach.performed.contains(&op) {
                self.reach_fn(fid);
                return;
            }
            pending.push(op);
        }
        // The function index is named by the closure record even though its body cannot run.
        self.reference_fn(fid);
        for op in pending {
            self.waiting
                .entry(op)
                .or_default()
                .push(Waiting::Function(fid));
        }
    }
}

/// The variables of `f` that produce the closure of a handler operation, each with the operations
/// it is the implementation of.
///
/// The operation slot of a [`Value::Handler`] is not always the destination of the [`Value::Closure`]
/// or [`Value::MakeClosure`] that builds it: the value can pass through a copy or a temporary
/// ([`Value::Coerce`], an `if` arm, a `block`, a loop's result). The scan follows the slot back to
/// the variables that actually make the closure, so the walk can defer the body under the right
/// operation whichever shape the value has.
fn handler_ops_of(f: &Function) -> FxHashMap<VarId, Vec<Op>> {
    // The statement defining each variable of the function (parameters and loop variables have
    // none, so the walk cannot follow them; a handler operation is always a `let` in practice).
    let mut defs: FxHashMap<VarId, &Stmt> = FxHashMap::default();
    for (_, stmt) in f.stmts.iter() {
        if let Some(dst) = stmt.dst() {
            defs.insert(dst, stmt);
        }
    }
    let mut ops: FxHashMap<VarId, Vec<Op>> = FxHashMap::default();
    for (_, stmt) in f.stmts.iter() {
        let Stmt::Let {
            value: Value::Handler {
                effect, ops: slots, ..
            },
            ..
        } = stmt
        else {
            continue;
        };
        for (index, &slot) in slots.iter().enumerate() {
            let op = (*effect, index);
            let mut seen = FxHashSet::default();
            closure_vars(f, &defs, slot, op, &mut ops, &mut seen);
        }
    }
    ops
}

/// Adds `op` to every variable whose defining value makes the closure `var` denotes, following
/// copies and temporaries.
fn closure_vars(
    f: &Function,
    defs: &FxHashMap<VarId, &Stmt>,
    var: VarId,
    op: Op,
    ops: &mut FxHashMap<VarId, Vec<Op>>,
    seen: &mut FxHashSet<VarId>,
) {
    if !seen.insert(var) {
        return;
    }
    let Some(stmt) = defs.get(&var) else {
        return;
    };
    match stmt {
        Stmt::Let { value, .. } => match value {
            Value::Closure(_) | Value::MakeClosure { .. } | Value::FnRef(_) => {
                let entry = ops.entry(var).or_default();
                if !entry.contains(&op) {
                    entry.push(op);
                }
            }
            Value::Coerce(base) => closure_vars(f, defs, *base, op, ops, seen),
            _ => {}
        },
        Stmt::If { then, else_, .. } => {
            for seq in [*then, *else_] {
                closure_vars_at_yield(f, defs, seq, op, ops, seen);
            }
        }
        Stmt::Switch { cases, default, .. } => {
            for case in cases {
                closure_vars_at_yield(f, defs, case.body, op, ops, seen);
            }
            if let Some(default) = default {
                closure_vars_at_yield(f, defs, *default, op, ops, seen);
            }
        }
        // The value of a block, a loop or a protected body leaves through its end (`yield`).
        Stmt::Block { body, .. } | Stmt::Loop { body, .. } | Stmt::Protect { body, .. } => {
            closure_vars_at_yield(f, defs, *body, op, ops, seen);
        }
        Stmt::Dup(_) | Stmt::Drop(_) => {}
    }
}

/// [`closure_vars`] for the value a sequence yields.
fn closure_vars_at_yield(
    f: &Function,
    defs: &FxHashMap<VarId, &Stmt>,
    seq: SeqId,
    op: Op,
    ops: &mut FxHashMap<VarId, Vec<Op>>,
    seen: &mut FxHashSet<VarId>,
) {
    if let Term::Yield(Some(Operand::Var(var))) = &f.seqs[seq].term {
        closure_vars(f, defs, *var, op, ops, seen);
    }
}

/// What [`prune_functions`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prune {
    /// The new roots, in the same order as the roots passed in.
    pub roots: Vec<FnId>,
    /// How many functions were removed.
    pub removed: usize,
}

/// Removes every function that `reach` neither calls nor references, and remaps the [`FnId`]s of
/// the module and of `roots`.
///
/// A [`FnId`] is an arena index, so removing a function renumbers the ones after it. Every place
/// that names a function is remapped: [`Value::FnRef`], [`Value::MakeClosure::function`],
/// [`Callee::Direct`] and [`Module::error_shows`]. The error `show` functions and the const
/// functions are kept by [`Reachability::walk`] (they are roots there), so they are never removed.
///
/// A handler operation that is referenced but not called stays: a reachable closure record names
/// its function, so the backend must keep its index and emit a stub body. Its body is replaced by
/// [`Term::Unreachable`] here, so that the names it used (the host a never-performed operation
/// would call) can be removed and the two backends emit the same stub. That is sound for the same
/// reason the WASM backend has always stubbed the body: no reachable code performs the operation,
/// so the function cannot be entered.
pub fn prune_functions(m: &mut Module, roots: &[FnId], reach: &Reachability) -> Prune {
    let keep = |id: FnId| reach.is_called(id) || reach.is_referenced(id);
    let old_len = m.functions.len();
    let mut kept: Arena<Function> = Arena::new();
    let mut map: FxHashMap<FnId, FnId> = FxHashMap::default();
    for (id, f) in m.functions.iter() {
        if keep(id) {
            let mut f = f.clone();
            if !reach.is_called(id) && f.body.is_some() {
                // Referenced but never called (a handler operation no reachable code performs):
                // keep the index and the signature, drop the body's names.
                f.stmts.clear();
                f.seqs.clear();
                f.labels.clear();
                f.drop_kinds.clear();
                f.reuse.clear();
                let body = f.seqs.alloc(crate::ir::Seq {
                    stmts: Vec::new(),
                    term: Term::Unreachable,
                });
                f.body = Some(body);
            }
            let new = kept.alloc(f);
            map.insert(id, new);
        }
    }
    let remap = |id: &mut FnId| {
        *id = *map
            .get(id)
            .expect("a reachable name refers only to a kept function");
    };
    for (_, f) in kept.iter_mut() {
        for (_, stmt) in f.stmts.iter_mut() {
            match stmt {
                Stmt::Let { value, .. } => remap_value(value, &remap),
                Stmt::Dup(_) | Stmt::Drop(_) => {}
                Stmt::If { .. }
                | Stmt::Switch { .. }
                | Stmt::Loop { .. }
                | Stmt::Block { .. }
                | Stmt::Protect { .. } => {}
            }
        }
    }
    let roots = roots
        .iter()
        .map(|&id| *map.get(&id).expect("a root is called, so it is kept"))
        .collect();
    for (_, show) in &mut m.error_shows {
        *show = *map
            .get(show)
            .expect("an error show function is a root, so it is kept");
    }
    let removed = old_len - kept.len();
    m.functions = kept;
    Prune { roots, removed }
}

/// Remaps the function names of a value.
fn remap_value(value: &mut Value, remap: &impl Fn(&mut FnId)) {
    match value {
        Value::FnRef(id) => remap(id),
        Value::MakeClosure { function, .. } => remap(function),
        Value::Call {
            callee: Callee::Direct(id),
            ..
        } => remap(id),
        Value::Call { .. }
        | Value::Const(_)
        | Value::Prim { .. }
        | Value::Builtin { .. }
        | Value::Make { .. }
        | Value::Field { .. }
        | Value::Tag(_)
        | Value::Tuple(_)
        | Value::TupleGet { .. }
        | Value::List { .. }
        | Value::ListIsEmpty(_)
        | Value::ListHead(_)
        | Value::ListTail(_)
        | Value::Closure(_)
        | Value::EnvGet { .. }
        | Value::EvLookup { .. }
        | Value::EvExtend { .. }
        | Value::Handler { .. }
        | Value::Fiber { .. }
        | Value::IsError { .. }
        | Value::AsError { .. }
        | Value::Runtime { .. }
        | Value::Coerce(_) => {}
    }
}
