//! Escape analysis: which values of a function may leave it, or reach a place that the function
//! does not control.
//!
//! The scalar replacement of aggregates (SRA) may decompose a tuple, record or closure record
//! only when the value stays inside the function: a value that escapes can be observed after the
//! function's frame is gone (or at a point whose order against the frame is observable), so its
//! allocation cannot be dropped.
//!
//! A value escapes when one of these holds, directly or through an aggregate it is put into:
//!
//! - it is a parameter (it comes from the caller), or its type has a finalizer or an untracked
//!   representation: `Any`, `FiberScope`, a handler value, an evidence table, a cell;
//! - it is stored into a container (`List`, `Array` / `Map` through a builtin, a `Fiber`
//!   operation), a handler or an evidence table, or it is captured by a closure;
//! - it is boxed or unboxed ([`Value::Coerce`]), which may cross `Any`;
//! - it is returned ([`Term::Return`]), failed with ([`Term::Fail`]), or becomes a defect message;
//! - it is passed to a call (direct, indirect or through a handler) or to a builtin, whose body
//!   the analysis does not see; a direct call is included because SRA does not yet inline;
//! - it is live across a suspension point of a suspending function: the state machine moves it
//!   into the heap frame ([`Suspend::saved`]).
//!
//! Everything else does not escape: a tuple or record built and consumed in the same function and
//! read only through its fields, tags or a `switch`, and a closure record that is only called.
//!
//! The analysis is deliberately conservative: it never says that a value does not escape when it
//! might. SRA uses it as a deopt condition, so a wrong `true` loses an optimisation while a wrong
//! `false` changes behaviour.

#![allow(dead_code)] // Read by the SRA pass (T13).

use emela_core::{Type, TypeId};
use rustc_hash::FxHashSet;

use crate::heap::live;
use crate::ir::{Function, Module, Operand, SeqId, Stmt, Term, Value, VarId};

/// The variables of `f` whose value may escape it.
pub(crate) type Set = FxHashSet<VarId>;

/// The variables of `f` that may escape it (see the module documentation).
///
/// `m` is the module `f` belongs to: the Core types of the variables decide which values have a
/// finalizer or an untracked representation. The result is a side table, so it does not change the
/// function.
pub(crate) fn escaping(m: &Module, f: &Function) -> Set {
    let mut walk = Walk {
        escaping: Set::default(),
        aggregates: Vec::new(),
    };
    walk.escaping.extend(f.params.iter().copied());
    for (id, v) in f.vars.iter() {
        if has_finalizer_or_untracked(m, v.ty) {
            walk.escaping.insert(id);
        }
    }
    if let Some(body) = f.body {
        walk.seq(f, body);
    }
    walk.suspension_points(f);
    walk.aggregates_escaping_with_their_fields();
    walk.escaping
}

/// Whether `ty` names a value that SRA must not decompose: a runtime object with a finalizer
/// (`fin`), an untracked `Any`, or a value whose lifetime the language does not keep in a variable.
fn has_finalizer_or_untracked(m: &Module, ty: TypeId) -> bool {
    matches!(
        m.core.ty(ty),
        Type::Any | Type::FiberScope | Type::Handler(_) | Type::Evidence | Type::Cell(_)
    )
}

struct Walk {
    escaping: Set,
    /// `dst = tuple(..)` / `dst = make(..)`: the fields stay with the aggregate, so they escape
    /// when it does.
    aggregates: Vec<(VarId, Vec<VarId>)>,
}

impl Walk {
    fn operands(&mut self, os: &[Operand]) {
        for o in os {
            if let Operand::Var(v) = o {
                self.escaping.insert(*v);
            }
        }
    }

    fn seq(&mut self, f: &Function, id: SeqId) {
        let seq = &f.seqs[id];
        for &s in &seq.stmts {
            self.stmt(f, s);
        }
        self.term(&seq.term);
    }

    fn term(&mut self, t: &Term) {
        match t {
            // Only the ways that leave the function (or become observable) move a value out.
            // `yield`, `break` and `recur` give values to a compound statement or a loop inside
            // the same function, so those values stay local.
            Term::Return(v) => {
                if let Some(v) = v {
                    self.operands(std::slice::from_ref(v));
                }
            }
            Term::Fail(v) | Term::Defect(v, _) => {
                self.operands(std::slice::from_ref(v));
            }
            Term::Yield(_)
            | Term::Break { .. }
            | Term::Recur(_)
            | Term::Unwind
            | Term::Unreachable => {}
        }
    }

    fn stmt(&mut self, f: &Function, id: crate::ir::StmtId) {
        match &f.stmts[id] {
            Stmt::Let {
                dst,
                value,
                on_defect,
                on_suspend,
            } => {
                self.value(*dst, value);
                if let Some(suspend) = on_suspend {
                    self.escaping.extend(suspend.saved.iter().copied());
                }
                if let Some(landing) = on_defect {
                    self.seq(f, *landing);
                }
            }
            Stmt::If { then, else_, .. } => {
                // The condition is a scalar (an `i32`); reading it does not move a value.
                self.seq(f, *then);
                self.seq(f, *else_);
            }
            Stmt::Switch { cases, default, .. } => {
                // The scrutinee is a tag or a scalar, read in place.
                for case in cases {
                    self.seq(f, case.body);
                }
                if let Some(default) = default {
                    self.seq(f, *default);
                }
            }
            Stmt::Loop { body, .. } => {
                // The loop variables are rebound by `recur`; their values stay local.
                self.seq(f, *body);
            }
            Stmt::Block { body, .. } => self.seq(f, *body),
            Stmt::Protect { body, cleanup, .. } => {
                self.seq(f, *body);
                self.seq(f, *cleanup);
            }
            Stmt::Dup(_) | Stmt::Drop(_) => {}
        }
    }

    fn value(&mut self, dst: Option<VarId>, value: &Value) {
        match value {
            Value::Const(_) | Value::FnRef(_) => {}
            // A primitive operation reads its operands; it neither stores nor returns them.
            Value::Prim { .. } => {}
            Value::Call { args, .. } => {
                // The callee is called, not stored; the arguments may be kept by the callee.
                self.operands(args);
            }
            // A builtin may store its arguments (`Array.set`, `Cell.set`, `Map.insert`, …). SRA
            // does not know the shape of each one, so every argument is treated as escaping.
            Value::Builtin { args, .. } => self.operands(args),
            Value::Make { args, .. } | Value::Tuple(args) => {
                if let Some(dst) = dst {
                    let fields = var_operands(args);
                    self.aggregates.push((dst, fields));
                }
            }
            // Reads: the base stays in its variable and does not escape by being read.
            Value::Field { .. }
            | Value::Tag(_)
            | Value::TupleGet { .. }
            | Value::ListIsEmpty(_)
            | Value::ListHead(_)
            | Value::ListTail(_)
            | Value::EvLookup { .. }
            | Value::IsError { .. }
            | Value::AsError { .. }
            | Value::EnvGet { .. } => {}
            // A list is a container: its elements (and tail) are stored in cons cells.
            Value::List { elems, tail } => {
                self.operands(elems);
                if let Some(tail) = tail {
                    self.operands(std::slice::from_ref(tail));
                }
            }
            // A closure body is a function of its own; only its captures leave this function.
            Value::Closure(closure) => {
                self.escaping.extend(closure.captures.iter().copied());
            }
            Value::MakeClosure { captures, .. } => {
                self.escaping.extend(captures.iter().copied());
            }
            // The base of an extension is read; the bindings are stored in the new table.
            Value::EvExtend { base, bindings } => {
                if let Some(base) = base {
                    self.escaping.insert(*base);
                }
                for (_, v) in bindings {
                    self.escaping.insert(*v);
                }
            }
            // The handler keeps its operations.
            Value::Handler { ops, .. } => self.escaping.extend(ops.iter().copied()),
            // A concurrency operation stores its arguments in runtime objects, and its value is a
            // runtime object with a finalizer (`fin`).
            Value::Fiber { args, .. } => {
                self.operands(args);
                if let Some(dst) = dst {
                    self.escaping.insert(dst);
                }
            }
            // A runtime operation takes and returns references the runtime owns (a pending defect,
            // a held value). They are not plain locals.
            Value::Runtime { args, .. } => {
                self.operands(args);
                if let Some(dst) = dst {
                    self.escaping.insert(dst);
                }
            }
            // Boxing or unboxing: the representation changes, so the value is not a plain local.
            Value::Coerce(v) => {
                self.escaping.insert(*v);
            }
        }
    }

    /// Marks the values live across a suspension point: the state machine saves them into the
    /// heap frame.
    fn suspension_points(&mut self, f: &Function) {
        if !f.suspends {
            return;
        }
        let live = live::live_on_lets(f);
        for (id, after) in live {
            if may_suspend(&f.stmts[id]) {
                self.escaping.extend(after);
            }
        }
    }

    /// A tuple or record that escapes keeps its fields alive, so the aggregates it holds escape
    /// too. Iterate to a fixpoint for nested aggregates.
    fn aggregates_escaping_with_their_fields(&mut self) {
        loop {
            let mut changed = false;
            for (dst, fields) in &self.aggregates {
                if self.escaping.contains(dst) {
                    for field in fields {
                        changed |= self.escaping.insert(*field);
                    }
                }
            }
            if !changed {
                break;
            }
        }
    }
}

fn var_operands(os: &[Operand]) -> Vec<VarId> {
    os.iter()
        .filter_map(|o| match o {
            Operand::Var(v) => Some(*v),
            Operand::Const(_) => None,
        })
        .collect()
}

/// Whether the value of a `let` may come back suspended (a suspension point of its function).
fn may_suspend(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Let { value, .. } => match value {
            Value::Call { suspends, .. } => *suspends,
            Value::Fiber { op, .. } => op.suspends(),
            _ => false,
        },
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::FnBuilder;
    use crate::ir::{Callee, Const, FnId, Stmt};
    use emela_core::{EffectDef, EffectId, FiberOp, FnType, TypeId};

    fn int(n: i32) -> Operand {
        Operand::Const(Const::Int(n))
    }

    /// A Core module with the definitions the tests use, plus a builder that adds a function.
    struct Fixture {
        m: Module,
        pair: TypeId,
        list_pair: TypeId,
        nested: TypeId,
        thunk: TypeId,
        effect: EffectId,
        handler: TypeId,
    }

    impl Fixture {
        fn new() -> Self {
            let mut core = emela_core::Module::new();
            let pair = core.tuple(vec![TypeId::INT, TypeId::INT]);
            let list_pair = core.list(pair);
            let nested = core.tuple(vec![pair, TypeId::INT]);
            let thunk = core.func(FnType {
                params: vec![],
                ret: TypeId::INT,
                fails: false,
                suspends: false,
                ev: false,
                sets: None,
            });
            let effect = core.add_effect(EffectDef::new("TestEffect", Vec::new()));
            let handler = core.handler(effect);
            Fixture {
                m: Module::new(core),
                pair,
                list_pair,
                nested,
                thunk,
                effect,
                handler,
            }
        }

        fn build<R>(&mut self, name: &str, f: impl FnOnce(&mut FnBuilder<'_>) -> R) -> (FnId, R) {
            let mut b = FnBuilder::new(&self.m.core, name);
            let r = f(&mut b);
            let function = b.finish();
            (self.m.add(function), r)
        }

        fn escaping(&self, id: FnId) -> Set {
            escaping(&self.m, &self.m.functions[id])
        }
    }

    #[test]
    fn a_tuple_read_through_a_field_does_not_escape() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let (id, t) = fx.build("read", |b| {
            b.set_ret(TypeId::INT);
            let t = b.assign("t", pair, Value::Tuple(vec![int(1), int(2)]));
            let x = b.assign("x", TypeId::INT, Value::TupleGet { base: t, index: 0 });
            b.term(Term::Return(Some(x.into())));
            t
        });
        assert!(!fx.escaping(id).contains(&t));
    }

    #[test]
    fn a_returned_tuple_escapes() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let (id, t) = fx.build("ret", |b| {
            b.set_ret(pair);
            let t = b.assign("t", pair, Value::Tuple(vec![int(1), int(2)]));
            b.term(Term::Return(Some(t.into())));
            t
        });
        assert!(fx.escaping(id).contains(&t));
    }

    #[test]
    fn a_tuple_stored_in_a_list_escapes() {
        let mut fx = Fixture::new();
        let (pair, list) = (fx.pair, fx.list_pair);
        let (id, t) = fx.build("list", |b| {
            let t = b.assign("t", pair, Value::Tuple(vec![int(1), int(2)]));
            b.assign(
                "xs",
                list,
                Value::List {
                    elems: vec![t.into()],
                    tail: None,
                },
            );
            b.term(Term::Return(None));
            t
        });
        assert!(fx.escaping(id).contains(&t));
    }

    #[test]
    fn a_tuple_captured_by_a_closure_escapes() {
        let mut fx = Fixture::new();
        let thunk = fx.thunk;
        let (g, ()) = fx.build("g", |b| {
            b.set_ret(TypeId::INT);
            b.term(Term::Return(Some(int(0))));
        });
        let pair = fx.pair;
        let (id, t) = fx.build("capture", |b| {
            let t = b.assign("t", pair, Value::Tuple(vec![int(1), int(2)]));
            b.assign(
                "closure",
                thunk,
                Value::MakeClosure {
                    function: g,
                    captures: vec![t],
                },
            );
            b.term(Term::Return(None));
            t
        });
        assert!(fx.escaping(id).contains(&t));
    }

    #[test]
    fn a_tuple_passed_to_an_indirect_call_escapes() {
        let mut fx = Fixture::new();
        let (pair, thunk) = (fx.pair, fx.thunk);
        let (id, t) = fx.build("indirect", |b| {
            let callee = b.param("callee", thunk);
            let t = b.assign("t", pair, Value::Tuple(vec![int(1), int(2)]));
            b.let_(
                None,
                Value::Call {
                    callee: Callee::Indirect(callee),
                    args: vec![t.into()],
                    suspends: false,
                    on_fail: None,
                },
            );
            b.term(Term::Return(None));
            t
        });
        assert!(fx.escaping(id).contains(&t));
    }

    #[test]
    fn a_boxed_tuple_escapes() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let (id, t) = fx.build("box", |b| {
            let t = b.assign("t", pair, Value::Tuple(vec![int(1), int(2)]));
            b.assign("boxed", TypeId::ANY, Value::Coerce(t));
            b.term(Term::Return(None));
            t
        });
        assert!(fx.escaping(id).contains(&t));
    }

    #[test]
    fn a_parameter_escapes() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let (id, p) = fx.build("param", |b| {
            let p = b.param("p", pair);
            b.term(Term::Return(None));
            p
        });
        assert!(fx.escaping(id).contains(&p));
    }

    #[test]
    fn a_field_of_an_escaping_aggregate_escapes() {
        let mut fx = Fixture::new();
        let (pair, nested) = (fx.pair, fx.nested);
        let (id, inner) = fx.build("nested", |b| {
            b.set_ret(nested);
            let inner = b.assign("inner", pair, Value::Tuple(vec![int(1), int(2)]));
            let outer = b.assign("outer", nested, Value::Tuple(vec![inner.into(), int(3)]));
            b.term(Term::Return(Some(outer.into())));
            inner
        });
        assert!(fx.escaping(id).contains(&inner));
    }

    #[test]
    fn a_value_live_across_a_suspension_escapes() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let (g, ()) = fx.build("suspend_once", |b| {
            b.set_ret(TypeId::INT);
            b.term(Term::Return(Some(int(0))));
        });
        let (id, t) = fx.build("live", |b| {
            b.set_suspends();
            let t = b.assign("t", pair, Value::Tuple(vec![int(1), int(2)]));
            b.let_(
                None,
                Value::Call {
                    callee: Callee::Direct(g),
                    args: vec![],
                    suspends: true,
                    on_fail: None,
                },
            );
            let x = b.assign("x", TypeId::INT, Value::TupleGet { base: t, index: 0 });
            b.term(Term::Return(Some(x.into())));
            t
        });
        assert!(fx.escaping(id).contains(&t));
    }

    #[test]
    fn a_value_dead_at_a_suspension_does_not_escape() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let (g, ()) = fx.build("suspend_again", |b| {
            b.set_ret(TypeId::INT);
            b.term(Term::Return(Some(int(0))));
        });
        let (id, t) = fx.build("dead", |b| {
            b.set_suspends();
            let t = b.assign("t", pair, Value::Tuple(vec![int(1), int(2)]));
            let x = b.assign("x", TypeId::INT, Value::TupleGet { base: t, index: 0 });
            b.let_(
                None,
                Value::Call {
                    callee: Callee::Direct(g),
                    args: vec![],
                    suspends: true,
                    on_fail: None,
                },
            );
            b.term(Term::Return(Some(x.into())));
            t
        });
        assert!(!fx.escaping(id).contains(&t));
    }

    #[test]
    fn a_value_stored_in_an_evidence_table_escapes() {
        let mut fx = Fixture::new();
        let (handler, effect) = (fx.handler, fx.effect);
        let (id, h) = fx.build("evidence", |b| {
            let h = b.param("h", handler);
            b.assign(
                "ev",
                TypeId::EVIDENCE,
                Value::EvExtend {
                    base: None,
                    bindings: vec![(effect, h)],
                },
            );
            b.term(Term::Return(None));
            h
        });
        assert!(fx.escaping(id).contains(&h));
        // The table itself is an observable value.
        let ev: Vec<_> = fx.escaping(id).into_iter().collect();
        assert!(ev.iter().any(|&v| fx.m.functions[id].vars[v].hint == "ev"));
    }

    #[test]
    fn a_value_stored_in_a_runtime_object_escapes() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let (id, t) = fx.build("deferred", |b| {
            let d = b.var("d", TypeId::ANY);
            let t = b.assign("t", pair, Value::Tuple(vec![int(1), int(2)]));
            b.let_(
                None,
                Value::Fiber {
                    op: FiberOp::Complete,
                    args: vec![d.into(), t.into()],
                    on_fail: None,
                },
            );
            b.term(Term::Return(None));
            t
        });
        assert!(fx.escaping(id).contains(&t));
        // The runtime value itself has a finalizer (`fin`).
        let vars = fx.m.functions[id]
            .vars
            .iter()
            .find(|(_, v)| v.hint == "d")
            .map(|(id, _)| id)
            .unwrap();
        assert!(fx.escaping(id).contains(&vars));
    }

    #[test]
    fn a_dropped_local_does_not_escape_by_being_dropped() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let (id, t) = fx.build("drop", |b| {
            let t = b.assign("t", pair, Value::Tuple(vec![int(1), int(2)]));
            let x = b.assign("x", TypeId::INT, Value::TupleGet { base: t, index: 0 });
            b.stmt(Stmt::Drop(t));
            b.term(Term::Return(Some(x.into())));
            t
        });
        assert!(!fx.escaping(id).contains(&t));
    }
}
