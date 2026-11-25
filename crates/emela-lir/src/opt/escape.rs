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
//! - it is boxed or unboxed ([`Value::Coerce`](crate::ir::Value::Coerce)), which may cross `Any`;
//! - it is returned ([`Term::Return`](crate::ir::Term::Return)), failed with
//!   ([`Term::Fail`](crate::ir::Term::Fail)), or becomes a defect message;
//! - it is passed to a call (direct, indirect or through a handler) or to a builtin, whose body
//!   the analysis does not see; a direct call is included because SRA does not yet inline;
//! - it is live across a suspension point of a suspending function: the state machine moves it
//!   into the heap frame ([`Suspend::saved`](crate::ir::Suspend::saved)).
//!
//! Everything else does not escape: a tuple or record built and consumed in the same function and
//! read only through its fields, tags or a `switch`, and a closure record that is only called.
//!
//! The analysis is deliberately conservative: it never says that a value does not escape when it
//! might. SRA uses it as a deopt condition, so a wrong `true` loses an optimisation while a wrong
//! `false` changes behaviour.
//!
//! # Implementation
//!
//! The implementation is [`crate::opt::analysis::escape`], behind the shared analysis API. This
//! module re-exports the intraprocedural entry point [`escaping`] that SRA and its tests call, so
//! there is one implementation and the answer SRA sees is exactly the one
//! [`crate::opt::analysis::escape::analyze`] returns.
//!
//! The analysis also computes the interprocedural parameter summary
//! ([`crate::opt::analysis::escape::parameter_summary`]): which parameters a function may keep.
//! This pull request does not feed it into SRA; C5/PR-043 (the records and closures of SRA) and
//! C2/PR-026 (borrow inference) consume it.

pub(crate) use super::analysis::escape::{Set, escaping};

#[cfg(test)]
#[path = "../../../emela-core/tests/support/mod.rs"]
mod support;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::FnBuilder;
    use crate::ir::{Callee, Const, FnId, Module, Operand, Stmt, Term, Value};
    use crate::opt::analysis::escape;
    use crate::{convert_closures, dump_function, from_core};
    use emela_core::{EffectDef, EffectId, FiberOp, FnType, Roots, TypeId};
    use emela_core::{monomorphize_from, verify_monomorphic};

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

    /// Lowers a program to the stage SRA runs at: monomorphic, in the Flow IR, closures lifted.
    fn mid(source: &str) -> Module {
        let m = support::try_lower_with_opt(
            &[("main.emel", source)],
            Roots::Exported,
            emela_core::core_sources(),
            emela_core::bundled_files(),
            false,
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));
        let roots: Vec<_> = m
            .functions
            .iter()
            .filter(|(_, f)| f.exported)
            .map(|(id, _)| id)
            .collect();
        let (mono, _) = monomorphize_from(&m, &roots);
        if let Err(errors) = verify_monomorphic(&mono) {
            let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
            panic!("not monomorphic:\n{}", lines.join("\n"));
        }
        let mut lir = from_core(mono).unwrap_or_else(|errors| {
            let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
            panic!("conversion errors:\n{}", lines.join("\n"));
        });
        convert_closures(&mut lir);
        lir
    }

    fn function(m: &Module, name: &str) -> FnId {
        m.functions
            .iter()
            .find(|(_, f)| {
                f.name == name
                    || f.name
                        .strip_prefix(name)
                        .is_some_and(|rest| rest.starts_with('['))
            })
            .map(|(id, _)| id)
            .unwrap_or_else(|| panic!("no function `{name}`"))
    }

    fn variable(m: &Module, id: FnId, hint: &str) -> crate::ir::VarId {
        m.functions[id]
            .vars
            .iter()
            .find(|(_, v)| v.hint == hint)
            .map(|(v, _)| v)
            .unwrap_or_else(|| panic!("no variable `{hint}` in `{}`", m.functions[id].name))
    }

    /// A program with a local tuple, a returned tuple, a captured value, a value stored in a list
    /// and a value passed to a call.
    const FIXTURE: &str = "\
fn id(x: (Int, Int)) -> (Int, Int) { x }

fn local(a: Int) -> Int {
  t = (a, a + 1)
  (x, y) = t
  x + y
}

fn ret(a: Int) -> (Int, Int) {
  (a, a + 1)
}

fn capture(a: Int) -> Int {
  t = (a, a + 1)
  f = |x| {
    (p, q) = t
    p + q + x
  }
  f(1)
}

fn store(a: Int) -> List[(Int, Int)] {
  t = (a, a + 1)
  [t]
}

fn pass(t: (Int, Int)) {
  u = id(t)
  ()
}

fn main() {
  t = (1, 2)
  pass(t)
  dbg(local(1))
  dbg(ret(2))
  dbg(capture(5))
  dbg(store(6))
}
";

    /// The analysis behind the shared API answers exactly the set SRA reads, and that set has the
    /// shapes the module documentation promises.
    #[test]
    fn the_analysis_api_returns_the_sets_sra_reads() {
        let m = mid(FIXTURE);
        let sets = escape::analyze(&m)
            .known()
            .expect("the escape analysis answers for a well-formed module");
        for (id, f) in m.functions.iter() {
            assert_eq!(
                &sets[&id],
                &escaping(&m, f),
                "function `{}` has a different set behind the API",
                f.name
            );
        }

        // A tuple built and read only through its fields stays.
        let local = function(&m, "local");
        let t = variable(&m, local, "t");
        assert!(
            !sets[&local].contains(&t),
            "{}",
            dump_function(&m, &m.functions[local])
        );

        // A tuple returned from the function leaves it.
        let ret = function(&m, "ret");
        assert!(
            m.functions[ret]
                .vars
                .iter()
                .any(|(v, d)| d.repr == crate::repr::Repr::Ref && sets[&ret].contains(&v)),
            "a returned tuple does not escape"
        );

        // A captured tuple leaves the function that built it.
        let capture = function(&m, "capture");
        let t = variable(&m, capture, "t");
        assert!(sets[&capture].contains(&t));

        // A tuple put into a list is stored in the container.
        let store = function(&m, "store");
        let t = variable(&m, store, "t");
        assert!(sets[&store].contains(&t));

        // A local passed to a call is paid for by the caller.
        let main = function(&m, "main");
        let t = variable(&m, main, "t");
        assert!(sets[&main].contains(&t));
    }
}
