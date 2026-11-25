//! Tests for the borrow analysis (`opt::analysis::borrow`): which parameters a function may
//! receive without taking ownership. The functions are built by hand so that a test names the
//! exact use it is about.

use emela_core::{Builtin, External, FnType, Type, TypeId};
use emela_lir::opt::analysis::Fact;
use emela_lir::opt::analysis::borrow::{self, BorrowFacts};
use emela_lir::{Callee, Const, FnBuilder, FnId, Module, Operand, Stmt, Suspend, Term, Value};

fn int(n: i32) -> Operand {
    Operand::Const(Const::Int(n))
}

/// A Core module with the types the tests use, plus a builder that adds a function.
struct Fixture {
    m: Module,
    pair: TypeId,
    list_pair: TypeId,
    thunk: TypeId,
    array_pair: TypeId,
}

impl Fixture {
    fn new() -> Self {
        let mut core = emela_core::Module::new();
        let pair = core.tuple(vec![TypeId::INT, TypeId::INT]);
        let list_pair = core.list(pair);
        let thunk = core.func(FnType {
            params: vec![pair],
            ret: TypeId::INT,
            fails: false,
            suspends: false,
            ev: false,
            sets: None,
        });
        let array_pair = core.intern(Type::Array(pair));
        Fixture {
            m: Module::new(core),
            pair,
            list_pair,
            thunk,
            array_pair,
        }
    }

    fn build<R>(&mut self, name: &str, f: impl FnOnce(&mut FnBuilder<'_>) -> R) -> (FnId, R) {
        let mut b = FnBuilder::new(&self.m.core, name);
        let r = f(&mut b);
        let function = b.finish();
        (self.m.add(function), r)
    }

    fn declare(&mut self, name: &str) -> FnId {
        self.m.declare(name)
    }

    fn define<R>(&mut self, id: FnId, f: impl FnOnce(&mut FnBuilder<'_>) -> R) -> R {
        let name = self.m.functions[id].name.clone();
        let mut b = FnBuilder::new(&self.m.core, &name);
        let r = f(&mut b);
        self.m.define(id, b.finish());
        r
    }

    fn external(&mut self, name: &str, params: &[TypeId]) -> FnId {
        let mut b = FnBuilder::new(&self.m.core, name);
        for (i, &ty) in params.iter().enumerate() {
            b.param(&format!("p{i}"), ty);
        }
        let function = b.finish_external(External {
            bindings: Vec::new(),
        });
        self.m.add(function)
    }

    fn facts(&self) -> BorrowFacts {
        borrow::analyze(&self.m)
            .known()
            .expect("a well-formed module has borrow facts")
    }

    fn params(&self, id: FnId) -> Vec<bool> {
        self.facts().borrowed_params[&id].clone()
    }
}

#[test]
fn a_parameter_only_read_is_borrowable() {
    let mut fx = Fixture::new();
    let pair = fx.pair;
    let (id, _) = fx.build("first", |b| {
        b.set_ret(TypeId::INT);
        let x = b.param("x", pair);
        let a = b.assign("a", TypeId::INT, Value::TupleGet { base: x, index: 0 });
        b.term(Term::Return(Some(a.into())));
    });
    assert_eq!(fx.params(id), vec![true]);
}

#[test]
fn a_returned_parameter_is_not_borrowable() {
    let mut fx = Fixture::new();
    let pair = fx.pair;
    let (id, _) = fx.build("id", |b| {
        b.set_ret(pair);
        let x = b.param("x", pair);
        b.term(Term::Return(Some(x.into())));
    });
    assert_eq!(fx.params(id), vec![false]);
}

#[test]
fn a_parameter_stored_in_a_list_is_not_borrowable() {
    let mut fx = Fixture::new();
    let (pair, list) = (fx.pair, fx.list_pair);
    let (id, _) = fx.build("store", |b| {
        b.set_ret(list);
        let x = b.param("x", pair);
        let xs = b.assign(
            "xs",
            list,
            Value::List {
                elems: vec![x.into()],
                tail: None,
            },
        );
        b.term(Term::Return(Some(xs.into())));
    });
    assert_eq!(fx.params(id), vec![false]);
}

#[test]
fn a_parameter_captured_by_a_closure_is_not_borrowable() {
    let mut fx = Fixture::new();
    let (pair, thunk) = (fx.pair, fx.thunk);
    let (g, _) = fx.build("g", |b| {
        b.set_ret(TypeId::INT);
        b.term(Term::Return(Some(int(0))));
    });
    let (id, _) = fx.build("capture", |b| {
        let x = b.param("x", pair);
        b.assign(
            "closure",
            thunk,
            Value::MakeClosure {
                function: g,
                captures: vec![x],
            },
        );
        b.term(Term::Return(None));
    });
    assert_eq!(fx.params(id), vec![false]);
}

#[test]
fn a_parameter_saved_by_a_suspension_is_not_borrowable() {
    let mut fx = Fixture::new();
    let pair = fx.pair;
    let (id, _) = fx.build("suspender", |b| {
        b.set_suspends();
        let x = b.param("x", pair);
        let yield_ = Value::Runtime {
            op: emela_lir::RuntimeOp::Yield,
            args: Vec::new(),
        };
        b.stmt(Stmt::Let {
            dst: None,
            value: yield_,
            on_defect: None,
            on_suspend: Some(Suspend {
                state: 0,
                saved: vec![x],
            }),
        });
        b.term(Term::Return(None));
    });
    assert_eq!(fx.params(id), vec![false]);
}

#[test]
fn a_parameter_passed_to_a_borrowable_callee_is_borrowable() {
    let mut fx = Fixture::new();
    let pair = fx.pair;
    let (read, _) = fx.build("read", |b| {
        b.set_ret(TypeId::INT);
        let x = b.param("x", pair);
        let a = b.assign("a", TypeId::INT, Value::TupleGet { base: x, index: 0 });
        b.term(Term::Return(Some(a.into())));
    });
    let (caller, _) = fx.build("caller", |b| {
        b.set_ret(TypeId::INT);
        let x = b.param("x", pair);
        let n = b.assign(
            "n",
            TypeId::INT,
            Value::Call {
                callee: Callee::Direct(read),
                args: vec![x.into()],
                suspends: false,
                on_fail: None,
            },
        );
        b.term(Term::Return(Some(n.into())));
    });
    assert_eq!(fx.params(caller), vec![true]);
}

#[test]
fn a_parameter_passed_to_a_non_borrowable_callee_is_not_borrowable() {
    let mut fx = Fixture::new();
    let pair = fx.pair;
    let (id, _) = fx.build("id", |b| {
        b.set_ret(pair);
        let x = b.param("x", pair);
        b.term(Term::Return(Some(x.into())));
    });
    let (caller, _) = fx.build("caller", |b| {
        b.set_ret(pair);
        let x = b.param("x", pair);
        let y = b.assign(
            "y",
            pair,
            Value::Call {
                callee: Callee::Direct(id),
                args: vec![x.into()],
                suspends: false,
                on_fail: None,
            },
        );
        b.term(Term::Return(Some(y.into())));
    });
    assert_eq!(fx.params(caller), vec![false]);
}

/// The sharpest hazard: the runtime writes the array in place when the actual reference count is
/// one, so a borrowed array argument would mutate the caller's array.
#[test]
fn the_array_argument_of_array_set_is_not_borrowable() {
    let mut fx = Fixture::new();
    let (pair, array) = (fx.pair, fx.array_pair);
    let (id, _) = fx.build("set", |b| {
        b.set_ret(array);
        let a = b.param("a", array);
        let i = b.param("i", TypeId::INT);
        let v = b.param("v", pair);
        let r = b.assign(
            "r",
            array,
            Value::Builtin {
                op: Builtin::ArraySet,
                args: vec![a.into(), i.into(), v.into()],
            },
        );
        b.term(Term::Return(Some(r.into())));
    });
    assert_eq!(
        fx.params(id),
        vec![false, true, false],
        "the array is written in place and the value is stored in it; only the index is read"
    );
}

#[test]
fn an_indirect_call_makes_its_arguments_non_borrowable_but_not_its_callee() {
    let mut fx = Fixture::new();
    let (pair, thunk) = (fx.pair, fx.thunk);
    let (id, _) = fx.build("apply", |b| {
        b.set_ret(TypeId::INT);
        let callee = b.param("callee", thunk);
        let x = b.param("x", pair);
        let n = b.assign(
            "n",
            TypeId::INT,
            Value::Call {
                callee: Callee::Indirect(callee),
                args: vec![x.into()],
                suspends: false,
                on_fail: None,
            },
        );
        b.term(Term::Return(Some(n.into())));
    });
    assert_eq!(fx.params(id), vec![true, false]);
}

#[test]
fn an_external_callee_makes_its_arguments_non_borrowable() {
    let mut fx = Fixture::new();
    let pair = fx.pair;
    let host = fx.external("host", &[pair]);
    assert_eq!(fx.params(host), vec![false]);
    let (caller, _) = fx.build("caller", |b| {
        let x = b.param("x", pair);
        b.let_(
            None,
            Value::Call {
                callee: Callee::Direct(host),
                args: vec![x.into()],
                suspends: false,
                on_fail: None,
            },
        );
        b.term(Term::Return(None));
    });
    assert_eq!(fx.params(caller), vec![false]);
}

/// A cycle that only passes the value around proves nothing, so the least fixed point keeps both
/// parameters unborrowable.
#[test]
fn a_cycle_that_only_passes_the_value_keeps_it_unborrowable() {
    let mut fx = Fixture::new();
    let pair = fx.pair;
    let a = fx.declare("a");
    let b = fx.declare("b");
    fx.define(a, |b_| {
        let x = b_.param("x", pair);
        b_.let_(
            None,
            Value::Call {
                callee: Callee::Direct(b),
                args: vec![x.into()],
                suspends: false,
                on_fail: None,
            },
        );
        b_.term(Term::Return(None));
    });
    fx.define(b, |b_| {
        let y = b_.param("y", pair);
        b_.let_(
            None,
            Value::Call {
                callee: Callee::Direct(a),
                args: vec![y.into()],
                suspends: false,
                on_fail: None,
            },
        );
        b_.term(Term::Return(None));
    });
    assert_eq!(fx.params(a), vec![false]);
    assert_eq!(fx.params(b), vec![false]);
}

/// A cycle that reaches a read-only function becomes borrowable through the fixpoint: the fact is
/// added once the read-only callee has been proved.
#[test]
fn a_cycle_that_reaches_a_read_is_borrowable_at_the_fixpoint() {
    let mut fx = Fixture::new();
    let pair = fx.pair;
    let read = fx.declare("read");
    let (a, _) = fx.build("a", |b| {
        let x = b.param("x", pair);
        b.let_(
            None,
            Value::Call {
                callee: Callee::Direct(read),
                args: vec![x.into()],
                suspends: false,
                on_fail: None,
            },
        );
        b.term(Term::Return(None));
    });
    fx.define(read, |b| {
        b.set_ret(TypeId::INT);
        let x = b.param("x", pair);
        let n = b.assign("n", TypeId::INT, Value::TupleGet { base: x, index: 0 });
        b.term(Term::Return(Some(n.into())));
    });
    assert_eq!(fx.params(read), vec![true]);
    assert_eq!(fx.params(a), vec![true]);
}

/// The environment parameter of a lifted function is read by `env_get`; the caller keeps the
/// closure record alive.
#[test]
fn an_environment_parameter_read_by_env_get_is_borrowable() {
    let mut fx = Fixture::new();
    let (pair, thunk) = (fx.pair, fx.thunk);
    let (id, _) = fx.build("lifted", |b| {
        b.set_ret(TypeId::INT);
        let env = b.param("env", thunk);
        let x = b.assign("x", pair, Value::EnvGet { env, index: 0 });
        let a = b.assign("a", TypeId::INT, Value::TupleGet { base: x, index: 0 });
        b.term(Term::Return(Some(a.into())));
    });
    fx.m.functions[id].env = Some(vec![pair]);
    assert_eq!(fx.params(id), vec![true]);
}

#[test]
fn a_parameter_read_by_every_view_is_borrowable() {
    let mut fx = Fixture::new();
    let (pair, list) = (fx.pair, fx.list_pair);
    let (id, _) = fx.build("views", |b| {
        b.set_ret(TypeId::INT);
        let x = b.param("x", pair);
        let xs = b.param("xs", list);
        let a = b.assign("a", TypeId::INT, Value::TupleGet { base: x, index: 0 });
        let tag = b.assign("tag", TypeId::INT, Value::Tag(x));
        let empty = b.assign("empty", TypeId::BOOL, Value::ListIsEmpty(xs));
        let n = b.assign(
            "n",
            TypeId::INT,
            Value::Prim {
                op: emela_lir::PrimOp::Add,
                ty: emela_lir::PrimTy::I32,
                args: vec![a.into(), int(0)],
            },
        );
        let m = b.assign(
            "m",
            TypeId::INT,
            Value::Prim {
                op: emela_lir::PrimOp::Add,
                ty: emela_lir::PrimTy::I32,
                args: vec![n.into(), empty.into()],
            },
        );
        let k = b.assign(
            "k",
            TypeId::INT,
            Value::Prim {
                op: emela_lir::PrimOp::Add,
                ty: emela_lir::PrimTy::I32,
                args: vec![m.into(), tag.into()],
            },
        );
        b.term(Term::Return(Some(k.into())));
    });
    assert_eq!(fx.params(id), vec![true, true]);
}

#[test]
fn the_facts_are_deterministic() {
    let mut fx = Fixture::new();
    let pair = fx.pair;
    let (read, _) = fx.build("read", |b| {
        b.set_ret(TypeId::INT);
        let x = b.param("x", pair);
        let a = b.assign("a", TypeId::INT, Value::TupleGet { base: x, index: 0 });
        b.term(Term::Return(Some(a.into())));
    });
    fx.build("caller", |b| {
        let x = b.param("x", pair);
        b.let_(
            None,
            Value::Call {
                callee: Callee::Direct(read),
                args: vec![x.into()],
                suspends: false,
                on_fail: None,
            },
        );
        b.term(Term::Return(None));
    });
    assert_eq!(borrow::analyze(&fx.m), borrow::analyze(&fx.m));
}

#[test]
fn only_a_malformed_module_has_no_facts() {
    let fx = Fixture::new();
    assert!(borrow::analyze(&fx.m).is_known());

    let mut malformed = Module::new(emela_core::Module::new());
    malformed.declare("declared_but_never_defined");
    assert!(borrow::analyze(&malformed).is_unknown());
    assert!(
        borrow::analyze(&malformed).known().is_none(),
        "Unknown is not an empty set of facts"
    );
}

/// `Fact::Unknown` from a query is not "not borrowable": the consumer deopts.
#[test]
fn an_unkeyed_position_is_unknown_not_unborrowable() {
    let mut fx = Fixture::new();
    let (id, _) = fx.build("f", |b| {
        b.set_ret(TypeId::INT);
        b.param("x", TypeId::INT);
        b.term(Term::Return(Some(int(0))));
    });
    let facts = fx.facts();
    assert_eq!(facts.borrowed(id, 99), Fact::Unknown);
}

#[test]
fn an_unused_parameter_is_borrowable() {
    let mut fx = Fixture::new();
    let pair = fx.pair;
    let (id, _) = fx.build("ignore", |b| {
        b.set_ret(TypeId::INT);
        b.param("x", pair);
        b.term(Term::Return(Some(int(0))));
    });
    assert_eq!(fx.params(id), vec![true], "no use takes the reference");
}
