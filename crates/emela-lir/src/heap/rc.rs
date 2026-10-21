//! Reference counting (Perceus): `dup` and `drop` so that every owned reference is consumed
//! exactly once on every path.
//!
//! Who owns what (the ownership rules, shared with the verifier's balance check):
//!
//! - Only `ref` variables are counted. Constants (string literals) are static data the runtime
//!   marks as not counted, so they need nothing here.
//! - Owned when defined: parameters (owned-by-default calling convention), loop variables, the
//!   destinations of compound statements, the error of a failure landing, and the destinations of
//!   values that make a new reference (calls, `make`, `list`, `tuple`, `make_closure`, …).
//! - Borrowed: the environment parameter of a lifted function (the caller keeps the closure record
//!   alive for the call). It is never dropped; using it where a reference is consumed needs a
//!   `dup`. When a suspension edge saves it, it is owned after a `dup` at the start: a frame may
//!   keep it after the call returned suspended and the caller let the record go.
//! - Views: `field`, `tuple_get`, `list_head`, `list_tail`, `env_get`, `ev_lookup`, `as_error`
//!   and `coerce` of a `ref` read a reference out of a value they borrow. Their destination owns
//!   nothing until a `dup` right after the statement, which the pass emits when the destination is
//!   used later.
//! - Consumed (ownership moves): the arguments of calls of functions, closure records and
//!   operations (their parameters are owned), the fields of `make`, the elements of `tuple` and
//!   `list`, the captures of `make_closure`, the table and handlers of `ev_extend`, the operations
//!   of `handler`, the argument of `runtime defect_resume`, the initial values of loops, the
//!   values of `yield`, `break`, `return`, `fail` and `recur`, and the variables a suspension edge
//!   saves (moved into the frame). A call consumes its arguments however it comes back (value,
//!   failure, defect or suspended).
//! - Borrowed (the owner keeps it): the callee of `call_indirect` and the handler of `call_op`,
//!   the arguments of external functions, builtins, `runtime` operations and concurrency
//!   operations (the runtime dups what it keeps), primitive operations, the base of views, and
//!   what `tag`, `list_is_empty`, `is_error` and `switch` look at. Builtins borrow so that the
//!   runtime written in C need not care about ownership.
//!
//! Where the pass puts `dup` and `drop`, from liveness (a backward analysis over the structured
//! control, with a fixpoint at loops):
//!
//! - Before a statement or terminator, a `dup` for each consumption of a variable that is still
//!   needed afterwards (live after it on any path, including its failure and defect landings, or
//!   borrowed by the same statement), so that only the last use takes the reference.
//! - After a statement, a `drop` for each owned variable that is no longer live (as early as
//!   possible: right after its last use, or right after its definition if it is unused).
//! - At the start of a branch, a landing or a loop body, a `drop` for each owned variable that the
//!   branch does not use.
//! - Before a terminator, a `drop` for each owned variable not needed where it goes: after the
//!   statement it yields to or breaks out of, at the loop head for `recur`, nothing for `return`,
//!   `fail` and `unwind`. Every edge into a join point thus brings the same owned variables.
//!
//! - On a suspension edge, the saved variables are consumed. They are exactly the owned variables
//!   at that point (the state machine pass saves what is live on and what the value borrows), so
//!   nothing is dropped there, and the resumed function continues after the value owning what the
//!   uninterrupted one owns.
//!
//! No drop specialization or reuse yet. Runs last.

use rustc_hash::{FxHashMap, FxHashSet};

use crate::ir::{
    Callee, FnId, Function, LabelId, Module, OnFail, Operand, RuntimeOp, SeqId, Stage, Stmt,
    StmtId, SwitchOn, Term, Value, VarId,
};
use crate::repr::Repr;

/// How a statement uses a variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Takes one reference.
    Consume,
    /// Reads it; the reference stays with the owner.
    Borrow,
}

/// Whether the variable is counted (`ref`).
pub(crate) fn counted(f: &Function, v: VarId) -> bool {
    f.vars[v].repr == Repr::Ref
}

/// Whether the variable is a borrowed parameter (the environment of a lifted function).
pub(crate) fn borrowed_param(f: &Function, v: VarId) -> bool {
    f.env.is_some() && f.params.first() == Some(&v)
}

/// Whether the value reads a reference out of a value it borrows, so that its destination owns
/// nothing until a `dup`.
pub(crate) fn is_view(f: &Function, v: &Value) -> bool {
    match v {
        Value::Field { .. }
        | Value::TupleGet { .. }
        | Value::ListHead(_)
        | Value::ListTail(_)
        | Value::EnvGet { .. }
        | Value::EvLookup { .. }
        | Value::AsError { .. } => true,
        Value::Coerce(x) => f.vars[*x].repr == Repr::Ref,
        _ => false,
    }
}

/// Applies `use_` to every counted variable the value uses, with how it uses it.
pub(crate) fn value_uses(m: &Module, f: &Function, v: &Value, use_: &mut dyn FnMut(VarId, Mode)) {
    let each = |os: &[Operand], mode: Mode, use_: &mut dyn FnMut(VarId, Mode)| {
        for o in os {
            if let Operand::Var(x) = o
                && counted(f, *x)
            {
                use_(*x, mode);
            }
        }
    };
    let one = |x: VarId, mode: Mode, use_: &mut dyn FnMut(VarId, Mode)| {
        if counted(f, x) {
            use_(x, mode);
        }
    };
    match v {
        Value::Const(_) | Value::FnRef(_) => {}
        Value::Prim { args, .. } | Value::Builtin { args, .. } | Value::Fiber { args, .. } => {
            each(args, Mode::Borrow, use_)
        }
        Value::Call { callee, args, .. } => {
            let mode = match callee {
                Callee::Direct(g) if m.functions[*g].external.is_some() => Mode::Borrow,
                Callee::Direct(_) => Mode::Consume,
                Callee::Indirect(c) | Callee::Op { handler: c, .. } => {
                    one(*c, Mode::Borrow, use_);
                    Mode::Consume
                }
            };
            each(args, mode, use_);
        }
        Value::Make { args, .. } | Value::Tuple(args) => each(args, Mode::Consume, use_),
        Value::List { elems, tail } => {
            each(elems, Mode::Consume, use_);
            each(tail.as_slice(), Mode::Consume, use_);
        }
        Value::Field { base: x, .. }
        | Value::TupleGet { base: x, .. }
        | Value::ListHead(x)
        | Value::ListTail(x)
        | Value::EnvGet { env: x, .. }
        | Value::EvLookup { ev: x, .. }
        | Value::AsError { error: x, .. }
        | Value::Tag(x)
        | Value::ListIsEmpty(x)
        | Value::IsError { error: x, .. }
        | Value::Coerce(x) => one(*x, Mode::Borrow, use_),
        Value::Closure(c) => c.captures.iter().for_each(|&x| one(x, Mode::Consume, use_)),
        Value::MakeClosure { captures, .. } => {
            captures.iter().for_each(|&x| one(x, Mode::Consume, use_))
        }
        Value::EvExtend { base, bindings } => {
            base.iter().for_each(|&x| one(x, Mode::Consume, use_));
            bindings
                .iter()
                .for_each(|&(_, x)| one(x, Mode::Consume, use_));
        }
        Value::Handler { ops, .. } => ops.iter().for_each(|&x| one(x, Mode::Consume, use_)),
        Value::Runtime { op, args } => {
            let mode = match op {
                RuntimeOp::DefectResume => Mode::Consume,
                _ => Mode::Borrow,
            };
            each(args, mode, use_);
        }
    }
}

/// The counted variables among the operands, in order (each consumed once per occurrence).
pub(crate) fn operand_vars(f: &Function, os: &[&Operand]) -> Vec<VarId> {
    os.iter()
        .filter_map(|o| match o {
            Operand::Var(v) if counted(f, *v) => Some(*v),
            _ => None,
        })
        .collect()
}

/// The values a terminator consumes.
pub(crate) fn term_consumes(f: &Function, t: &Term) -> Vec<VarId> {
    match t {
        Term::Yield(v) | Term::Return(v) | Term::Break { value: v, .. } => {
            operand_vars(f, &v.iter().collect::<Vec<_>>())
        }
        Term::Fail(v) | Term::Defect(v) => operand_vars(f, &[v]),
        Term::Recur(args) => operand_vars(f, &args.iter().collect::<Vec<_>>()),
        Term::Unwind | Term::Unreachable => Vec::new(),
    }
}

/// The landings of a let: the failure landing (with its error variable) and the defect landing.
pub(crate) fn landings(stmt: &Stmt) -> (Option<(VarId, SeqId)>, Option<SeqId>) {
    match stmt {
        Stmt::Let {
            value, on_defect, ..
        } => {
            let fail = match value {
                Value::Call {
                    on_fail: Some(OnFail::Handle { error, body }),
                    ..
                }
                | Value::Fiber {
                    on_fail: Some(OnFail::Handle { error, body }),
                    ..
                } => Some((*error, *body)),
                _ => None,
            };
            (fail, *on_defect)
        }
        _ => (None, None),
    }
}

/// The counted variables a let's suspension edge saves.
pub(crate) fn suspend_saved(f: &Function, stmt: &Stmt) -> Vec<VarId> {
    match stmt {
        Stmt::Let {
            on_suspend: Some(s),
            ..
        } => s.saved.iter().copied().filter(|&v| counted(f, v)).collect(),
        _ => Vec::new(),
    }
}

type Set = FxHashSet<VarId>;

/// Inserts `dup` and `drop` into every function. The module must be at
/// [`Stage::StateMachines`]; it is at [`Stage::RefCounted`] afterwards.
pub fn insert_rc(m: &mut Module) {
    assert_eq!(
        m.stage,
        Stage::StateMachines,
        "reference counting runs after the state machines are made"
    );
    let ids: Vec<FnId> = m.functions.iter().map(|(id, _)| id).collect();
    for id in ids {
        let f = &m.functions[id];
        if f.body.is_none() {
            continue;
        }
        let mut f = f.clone();
        Counter::run(m, &mut f);
        m.functions[id] = f;
    }
    m.stage = Stage::RefCounted;
}

fn sorted(set: &Set) -> Vec<VarId> {
    let mut vs: Vec<VarId> = set.iter().copied().collect();
    vs.sort_by_key(|v| v.into_raw());
    vs
}

struct Counter<'a> {
    m: &'a Module,
    /// The live variables at the start of each sequence (by index).
    live: Vec<Set>,
    /// The live outer variables at the head of each loop (by statement index).
    heads: FxHashMap<StmtId, Set>,
    /// The live variables after each block (by label), during the liveness analysis.
    after_label: FxHashMap<LabelId, Set>,
    /// The owned variables wanted after each block, during the rewrite.
    label_target: FxHashMap<LabelId, Set>,
}

fn ix<T>(id: la_arena::Idx<T>) -> usize {
    u32::from(id.into_raw()) as usize
}

impl<'a> Counter<'a> {
    fn run(m: &'a Module, f: &mut Function) {
        let mut c = Counter {
            m,
            live: vec![Set::default(); f.seqs.len()],
            heads: FxHashMap::default(),
            after_label: FxHashMap::default(),
            label_target: FxHashMap::default(),
        };
        let body = f.body.expect("a function with a body");
        let live = c.live_seq(f, body, &Set::default(), None);
        let mut owned: Set = f
            .params
            .iter()
            .copied()
            .filter(|&p| counted(f, p) && !borrowed_param(f, p))
            .collect();
        // A suspending function owns its environment when a frame may keep it (see the module
        // documentation).
        let env = f.params.first().copied().filter(|&p| {
            borrowed_param(f, p)
                && live.contains(&p)
                && f.stmts.iter().any(|(_, s)| {
                    matches!(s, Stmt::Let { on_suspend: Some(e), .. } if e.saved.contains(&p))
                })
        });
        owned.extend(env);
        c.seq(f, body, owned, &Set::default(), None);
        if let Some(env) = env {
            let dup = f.stmts.alloc(Stmt::Dup(env));
            f.seqs[body].stmts.insert(0, dup);
        }
    }

    // Liveness.

    /// The counted variables a value uses.
    fn uses(&self, f: &Function, v: &Value) -> Vec<(VarId, Mode)> {
        let mut out = Vec::new();
        value_uses(self.m, f, v, &mut |x, mode| out.push((x, mode)));
        out
    }

    /// Computes and records the live variables at the start of the sequence. `yield_live`: live
    /// after the statement a `yield` goes to; `head`: live outer variables at the innermost loop
    /// head.
    fn live_seq(&mut self, f: &Function, id: SeqId, yield_live: &Set, head: Option<&Set>) -> Set {
        let seq = &f.seqs[id];
        let mut live: Set = match &seq.term {
            Term::Yield(_) => yield_live.clone(),
            Term::Break { label, .. } => self.after_label.get(label).cloned().unwrap_or_default(),
            Term::Recur(_) => head.cloned().unwrap_or_default(),
            _ => Set::default(),
        };
        live.extend(term_consumes(f, &seq.term));
        for &s in seq.stmts.iter().rev() {
            live = self.live_stmt(f, s, live, yield_live, head);
        }
        self.live[ix(id)] = live.clone();
        live
    }

    /// The live variables before the statement, given those after it.
    fn live_stmt(
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
            Stmt::Let { value, .. } => {
                let (fail, defect) = landings(stmt);
                if let Some((e, body)) = fail {
                    let mut l = self.live_seq(f, body, yield_live, head);
                    l.remove(&e);
                    after.extend(l);
                }
                if let Some(l) = defect {
                    after.extend(self.live_seq(f, l, yield_live, head));
                }
                after.extend(self.uses(f, value).into_iter().map(|(v, _)| v));
                after.extend(suspend_saved(f, stmt));
                after
            }
            Stmt::Dup(v) | Stmt::Drop(v) => {
                after.insert(*v);
                after
            }
            Stmt::If { then, else_, .. } => {
                let mut l = self.live_seq(f, *then, &after, head);
                l.extend(self.live_seq(f, *else_, &after, head));
                l
            }
            Stmt::Switch {
                on, cases, default, ..
            } => {
                let mut l = Set::default();
                for c in cases {
                    l.extend(self.live_seq(f, c.body, &after, head));
                }
                if let Some(d) = default {
                    l.extend(self.live_seq(f, *d, &after, head));
                }
                if let SwitchOn::Tag(v) | SwitchOn::Lit(v) = on
                    && counted(f, *v)
                {
                    l.insert(*v);
                }
                l
            }
            Stmt::Block { label, body, .. } => {
                self.after_label.insert(*label, after.clone());
                self.live_seq(f, *body, &after, head)
            }
            Stmt::Loop {
                vars, inits, body, ..
            } => {
                let mut h = Set::default();
                loop {
                    let mut l = self.live_seq(f, *body, &after, Some(&h));
                    for v in vars {
                        l.remove(v);
                    }
                    if l == h {
                        break;
                    }
                    h = l;
                }
                self.heads.insert(s, h.clone());
                let mut l = h;
                l.extend(operand_vars(f, &inits.iter().collect::<Vec<_>>()));
                l
            }
            Stmt::Protect { .. } => unreachable!("no protect after the defect paths are explicit"),
        }
    }

    /// The live variables after each statement of the sequence (using the recorded liveness of
    /// nested sequences).
    fn afters(&self, f: &Function, id: SeqId, yield_live: &Set, head: Option<&Set>) -> Vec<Set> {
        let seq = &f.seqs[id];
        let mut live: Set = match &seq.term {
            Term::Yield(_) => yield_live.clone(),
            Term::Break { label, .. } => self.after_label.get(label).cloned().unwrap_or_default(),
            Term::Recur(_) => head.cloned().unwrap_or_default(),
            _ => Set::default(),
        };
        live.extend(term_consumes(f, &seq.term));
        let mut out = vec![Set::default(); seq.stmts.len()];
        for (i, &s) in seq.stmts.iter().enumerate().rev() {
            out[i] = live.clone();
            live = self.live_before(f, s, live);
        }
        out
    }

    /// [`Self::live_stmt`] from the recorded liveness of nested sequences.
    fn live_before(&self, f: &Function, s: StmtId, mut after: Set) -> Set {
        let stmt = &f.stmts[s];
        if let Some(d) = stmt.dst() {
            after.remove(&d);
        }
        match stmt {
            Stmt::Let { value, .. } => {
                let (fail, defect) = landings(stmt);
                if let Some((e, body)) = fail {
                    let mut l = self.live[ix(body)].clone();
                    l.remove(&e);
                    after.extend(l);
                }
                if let Some(l) = defect {
                    after.extend(self.live[ix(l)].iter().copied());
                }
                after.extend(self.uses(f, value).into_iter().map(|(v, _)| v));
                after.extend(suspend_saved(f, stmt));
                after
            }
            Stmt::Dup(v) | Stmt::Drop(v) => {
                after.insert(*v);
                after
            }
            Stmt::If { then, else_, .. } => {
                let mut l = self.live[ix(*then)].clone();
                l.extend(self.live[ix(*else_)].iter().copied());
                l
            }
            Stmt::Switch {
                on, cases, default, ..
            } => {
                let mut l = Set::default();
                for c in cases {
                    l.extend(self.live[ix(c.body)].iter().copied());
                }
                if let Some(d) = default {
                    l.extend(self.live[ix(*d)].iter().copied());
                }
                if let SwitchOn::Tag(v) | SwitchOn::Lit(v) = on
                    && counted(f, *v)
                {
                    l.insert(*v);
                }
                l
            }
            Stmt::Block { body, .. } => self.live[ix(*body)].clone(),
            Stmt::Loop { inits, .. } => {
                let mut l = self.heads[&s].clone();
                l.extend(operand_vars(f, &inits.iter().collect::<Vec<_>>()));
                l
            }
            Stmt::Protect { .. } => unreachable!("no protect after the defect paths are explicit"),
        }
    }

    // Rewriting.

    fn new_stmt(f: &mut Function, s: Stmt, out: &mut Vec<StmtId>) {
        out.push(f.stmts.alloc(s));
    }

    /// Drops the owned variables not in `keep`.
    fn drop_except(f: &mut Function, owned: &mut Set, keep: &Set, out: &mut Vec<StmtId>) {
        let dead: Set = owned
            .iter()
            .filter(|v| !keep.contains(v))
            .copied()
            .collect();
        for v in sorted(&dead) {
            owned.remove(&v);
            Self::new_stmt(f, Stmt::Drop(v), out);
        }
    }

    /// The `dup`s for consuming `consumed` (one reference per occurrence) when the variables in
    /// `keep` must stay owned afterwards; takes the consumed references out of `owned`.
    fn consume(
        f: &mut Function,
        owned: &mut Set,
        consumed: &[VarId],
        keep: &dyn Fn(VarId) -> bool,
        out: &mut Vec<StmtId>,
    ) {
        let mut counts: Vec<(VarId, usize)> = Vec::new();
        for &v in consumed {
            match counts.iter_mut().find(|(x, _)| *x == v) {
                Some((_, k)) => *k += 1,
                None => counts.push((v, 1)),
            }
        }
        for (v, k) in counts {
            let give_up = owned.contains(&v) && !keep(v);
            let dups = if give_up { k - 1 } else { k };
            for _ in 0..dups {
                Self::new_stmt(f, Stmt::Dup(v), out);
            }
            if give_up {
                owned.remove(&v);
            }
        }
    }

    /// Rewrites the sequence entered with the owned variables `owned`. `yield_target`: the owned
    /// variables wanted after the statement a `yield` goes to (and its liveness for `afters`);
    /// `loop_target`: those wanted at the loop head.
    fn seq(
        &mut self,
        f: &mut Function,
        id: SeqId,
        mut owned: Set,
        yield_target: &Set,
        loop_target: Option<&Set>,
    ) {
        let mut out = Vec::new();
        let live_in = self.live[ix(id)].clone();
        Self::drop_except(f, &mut owned, &live_in, &mut out);
        // Liveness after a yield is the yield target's liveness: the target set is exactly the
        // owned variables live there, so it serves as both.
        let afters = self.afters(f, id, yield_target, loop_target);
        let stmts = f.seqs[id].stmts.clone();
        for (i, &s) in stmts.iter().enumerate() {
            self.stmt(f, s, &mut owned, &afters[i], loop_target, &mut out);
        }
        let term = f.seqs[id].term.clone();
        let target: Option<Set> = match &term {
            Term::Yield(_) => Some(yield_target.clone()),
            Term::Break { label, .. } => Some(self.label_target[label].clone()),
            Term::Recur(_) => Some(loop_target.cloned().unwrap_or_default()),
            Term::Return(_) | Term::Fail(_) | Term::Unwind | Term::Defect(_) => {
                Some(Set::default())
            }
            Term::Unreachable => None,
        };
        if let Some(target) = target {
            let consumed = term_consumes(f, &term);
            Self::consume(f, &mut owned, &consumed, &|v| target.contains(&v), &mut out);
            Self::drop_except(f, &mut owned, &target, &mut out);
        }
        f.seqs[id].stmts = out;
    }

    fn stmt(
        &mut self,
        f: &mut Function,
        s: StmtId,
        owned: &mut Set,
        after: &Set,
        loop_target: Option<&Set>,
        out: &mut Vec<StmtId>,
    ) {
        let stmt = f.stmts[s].clone();
        let dst = stmt.dst().filter(|&d| counted(f, d));
        match &stmt {
            Stmt::Let { value, .. } => {
                let uses = self.uses(f, value);
                let (fail, defect) = landings(&stmt);
                // Live after the uses, on any way on.
                let mut live_on: Set = after.clone();
                if let Some(d) = dst {
                    live_on.remove(&d);
                }
                if let Some((e, body)) = fail {
                    live_on.extend(self.live[ix(body)].iter().copied().filter(|&v| v != e));
                }
                if let Some(l) = defect {
                    live_on.extend(self.live[ix(l)].iter().copied());
                }
                let borrowed: Set = uses
                    .iter()
                    .filter(|(_, m)| *m == Mode::Borrow)
                    .map(|(v, _)| *v)
                    .collect();
                let consumed: Vec<VarId> = uses
                    .iter()
                    .filter(|(_, m)| *m == Mode::Consume)
                    .map(|(v, _)| *v)
                    .collect();
                Self::consume(
                    f,
                    owned,
                    &consumed,
                    &|v| live_on.contains(&v) || borrowed.contains(&v),
                    out,
                );
                out.push(s);
                // The suspension edge moves exactly the owned variables into the frame.
                debug_assert!(
                    !matches!(
                        &stmt,
                        Stmt::Let {
                            on_suspend: Some(_),
                            ..
                        }
                    ) || suspend_saved(f, &stmt).into_iter().collect::<Set>() == *owned,
                    "a suspension edge of {} saves other variables than the owned ones",
                    f.name
                );
                if let Some((e, body)) = fail {
                    let mut entry = owned.clone();
                    entry.insert(e);
                    self.seq(f, body, entry, &Set::default(), loop_target);
                }
                if let Some(l) = defect {
                    self.seq(f, l, owned.clone(), &Set::default(), loop_target);
                }
                if let Some(d) = dst {
                    if !is_view(f, value) {
                        owned.insert(d);
                    } else if after.contains(&d) {
                        Self::new_stmt(f, Stmt::Dup(d), out);
                        owned.insert(d);
                    }
                }
            }
            Stmt::Dup(_) | Stmt::Drop(_) => out.push(s),
            Stmt::If { then, else_, .. } => {
                let target = self.target(owned, after, dst);
                self.seq(f, *then, owned.clone(), &target, loop_target);
                self.seq(f, *else_, owned.clone(), &target, loop_target);
                out.push(s);
                *owned = target;
                owned.extend(dst);
            }
            Stmt::Switch { cases, default, .. } => {
                let target = self.target(owned, after, dst);
                for c in cases {
                    self.seq(f, c.body, owned.clone(), &target, loop_target);
                }
                if let Some(d) = default {
                    self.seq(f, *d, owned.clone(), &target, loop_target);
                }
                out.push(s);
                *owned = target;
                owned.extend(dst);
            }
            Stmt::Block { label, body, .. } => {
                let target = self.target(owned, after, dst);
                self.label_target.insert(*label, target.clone());
                self.seq(f, *body, owned.clone(), &target, loop_target);
                out.push(s);
                *owned = target;
                owned.extend(dst);
            }
            Stmt::Loop {
                vars, inits, body, ..
            } => {
                let head = self.heads[&s].clone();
                let consumed = operand_vars(f, &inits.iter().collect::<Vec<_>>());
                Self::consume(f, owned, &consumed, &|v| head.contains(&v), out);
                Self::drop_except(f, owned, &head, out);
                let at_head = owned.clone();
                let target = self.target(&at_head, after, dst);
                let mut entry = at_head.clone();
                entry.extend(vars.iter().copied().filter(|&v| counted(f, v)));
                self.seq(f, *body, entry, &target, Some(&at_head));
                out.push(s);
                *owned = target;
                owned.extend(dst);
            }
            Stmt::Protect { .. } => unreachable!("no protect after the defect paths are explicit"),
        }
        Self::drop_except(f, owned, after, out);
    }

    /// The owned variables wanted after a compound statement: those owned now that are live
    /// after it.
    fn target(&self, owned: &Set, after: &Set, dst: Option<VarId>) -> Set {
        owned
            .iter()
            .filter(|v| after.contains(v) && Some(**v) != dst)
            .copied()
            .collect()
    }
}
