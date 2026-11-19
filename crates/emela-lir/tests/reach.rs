//! Reachability and function pruning on hand-built modules, at both LIR stages.
//!
//! The tests build the shapes the backends see: the Flow IR (a closure body nested in its
//! function) and the Heap IR (a lifted closure body as its own function, a handler value and
//! operation calls). The two-op `Console` effect of the fixture plays the role of the design
//! note's `Ping` program: only `print_line` is performed, so `print_error_line` must be
//! `referenced` but not `called`.

mod common;

use common::{Fixture, string};
use emela_core::External;
use emela_lir::analysis::reach::{Reachability, prune_functions};
use emela_lir::{Callee, Closure, FnBuilder, FnId, Function, Module, Stage, Term, Value};

/// A function of the fixture's Core module with a body that calls `callee` directly.
fn caller(fx: &Fixture, name: &str, callee: FnId) -> Function {
    let mut b = FnBuilder::new(&fx.core, name);
    b.let_(
        None,
        Value::Call {
            callee: Callee::Direct(callee),
            args: Vec::new(),
            suspends: false,
            on_fail: None,
        },
    );
    b.term(Term::Return(None));
    b.finish()
}

/// A body that calls `callee` directly and returns.
fn call_body(b: &mut FnBuilder<'_>, callee: FnId) {
    b.let_(
        None,
        Value::Call {
            callee: Callee::Direct(callee),
            args: Vec::new(),
            suspends: false,
            on_fail: None,
        },
    );
    b.term(Term::Return(None));
}

fn leaf(fx: &Fixture, name: &str) -> Function {
    let mut b = FnBuilder::new(&fx.core, name);
    b.term(Term::Return(None));
    b.finish()
}

/// A direct call reaches its callee, transitively; a function nothing calls is not reached.
#[test]
fn direct_calls_are_followed() {
    let fx = Fixture::new();
    let mut m = Module::new(fx.core.clone());
    let c = m.add(leaf(&fx, "c"));
    let b = m.add(caller(&fx, "b", c));
    let a = m.add(caller(&fx, "a", b));
    let unused = m.add(leaf(&fx, "unused"));

    let reach = Reachability::walk(&m, &[a]);
    assert!(reach.is_called(a));
    assert!(reach.is_called(b));
    assert!(reach.is_called(c));
    assert!(reach.is_referenced(c));
    assert!(!reach.is_called(unused));
    assert!(!reach.is_referenced(unused));
}

/// A nested Flow-IR closure body is reached where the closure is built, and it calls its own
/// direct callee.
#[test]
fn flow_closure_bodies_are_followed() {
    let fx = Fixture::new();
    let mut m = Module::new(fx.core.clone());
    let c = m.add(leaf(&fx, "c"));
    // fn build() { f = closure() { c() }; return }
    let mut b = FnBuilder::new(&fx.core, "build");
    let body = b.seq(|b| {
        b.let_(
            None,
            Value::Call {
                callee: Callee::Direct(c),
                args: Vec::new(),
                suspends: false,
                on_fail: None,
            },
        );
        b.term(Term::Return(None));
    });
    b.assign(
        "f",
        fx.thunk,
        Value::Closure(Box::new(Closure {
            params: Vec::new(),
            captures: Vec::new(),
            body,
            suspends: false,
        })),
    );
    b.term(Term::Return(None));
    let build = m.add(b.finish());

    let reach = Reachability::walk(&m, &[build]);
    assert!(reach.is_called(build));
    assert!(reach.is_called(c), "the closure body's call is reached");
}

/// A `FnRef` value is referenced and, as the old walk did, its target is called: closure
/// conversion makes a wrapper whose body calls it.
#[test]
fn fn_ref_names_and_reaches_its_target() {
    let fx = Fixture::new();
    let mut m = Module::new(fx.core.clone());
    let g = m.add(leaf(&fx, "g"));
    let mut b = FnBuilder::new(&fx.core, "main");
    b.assign("f", fx.thunk, Value::FnRef(g));
    b.term(Term::Return(None));
    let main = m.add(b.finish());

    let reach = Reachability::walk(&m, &[main]);
    assert!(reach.is_called(g));
    assert!(reach.is_referenced(g));
}

/// A Flow-IR handler: the performed operation's body is reached, the other one is not, even
/// though both closures are built in a reachable function.
#[test]
fn flow_handler_operation_is_deferred_until_performed() {
    let fx = Fixture::new();
    let mut m = Module::new(fx.core.clone());
    let ping = m.add(leaf(&fx, "ping_body"));
    let pong = m.add(leaf(&fx, "pong_body"));

    // fn mk() -> handler Console { the two op closures; handler { print_line: p, print_error_line: q } }
    let mut b = FnBuilder::new(&fx.core, "mk");
    let p = {
        let body = b.seq(|b| call_body(b, ping));
        b.assign(
            "p",
            fx.string_to_unit,
            Value::Closure(Box::new(Closure {
                params: Vec::new(),
                captures: Vec::new(),
                body,
                suspends: false,
            })),
        )
    };
    let q = {
        let body = b.seq(|b| call_body(b, pong));
        b.assign(
            "q",
            fx.string_to_unit,
            Value::Closure(Box::new(Closure {
                params: Vec::new(),
                captures: Vec::new(),
                body,
                suspends: false,
            })),
        )
    };
    let h = b.assign(
        "h",
        fx.console_ty,
        Value::Handler {
            name: "Test".to_owned(),
            effect: fx.console,
            ops: vec![p, q],
        },
    );
    b.term(Term::Return(Some(h.into())));
    let mk = m.add(b.finish());

    let mut b = FnBuilder::new(&fx.core, "main");
    let h = b.assign(
        "h",
        fx.console_ty,
        Value::Call {
            callee: Callee::Direct(mk),
            args: Vec::new(),
            suspends: false,
            on_fail: None,
        },
    );
    b.let_(
        None,
        Value::Call {
            callee: Callee::Op {
                handler: h,
                effect: fx.console,
                op: 0,
            },
            args: vec![string("x")],
            suspends: false,
            on_fail: None,
        },
    );
    b.term(Term::Return(None));
    let main = m.add(b.finish());

    let reach = Reachability::walk(&m, &[main]);
    assert!(reach.performed().contains(&(fx.console, 0)));
    assert!(!reach.performed().contains(&(fx.console, 1)));
    assert!(reach.is_called(ping));
    assert!(
        !reach.is_called(pong),
        "an operation no reachable code performs does not run"
    );
}

/// The Heap IR shape of the design note's probe: two `MakeClosure` records in a handler, only one
/// operation performed. The unused operation is referenced (its index is named by the handler
/// record) but not called, and a host it alone calls is not reachable.
#[test]
fn heap_handler_operation_is_referenced_but_not_called() {
    let fx = Fixture::new();
    let mut m = Module::new(fx.core.clone());
    m.stage = Stage::ClosuresConverted;

    let host = m.add(Function {
        name: "host".to_owned(),
        external: Some(External {
            bindings: Vec::new(),
        }),
        ..leaf(&fx, "host")
    });
    let ping = m.add(leaf(&fx, "ping_body"));
    let pong = m.add(caller(&fx, "pong_body", host));

    let mut b = FnBuilder::new(&fx.core, "mk");
    let p = b.assign(
        "p",
        fx.string_to_unit,
        Value::MakeClosure {
            function: ping,
            captures: Vec::new(),
        },
    );
    let q = b.assign(
        "q",
        fx.string_to_unit,
        Value::MakeClosure {
            function: pong,
            captures: Vec::new(),
        },
    );
    let h = b.assign(
        "h",
        fx.console_ty,
        Value::Handler {
            name: "Test".to_owned(),
            effect: fx.console,
            ops: vec![p, q],
        },
    );
    b.term(Term::Return(Some(h.into())));
    let mk = m.add(b.finish());

    let mut b = FnBuilder::new(&fx.core, "main");
    let h = b.assign(
        "h",
        fx.console_ty,
        Value::Call {
            callee: Callee::Direct(mk),
            args: Vec::new(),
            suspends: false,
            on_fail: None,
        },
    );
    b.let_(
        None,
        Value::Call {
            callee: Callee::Op {
                handler: h,
                effect: fx.console,
                op: 0,
            },
            args: vec![string("x")],
            suspends: false,
            on_fail: None,
        },
    );
    b.term(Term::Return(None));
    let main = m.add(b.finish());

    let reach = Reachability::walk(&m, &[main]);
    assert!(reach.is_called(ping));
    assert!(reach.is_referenced(ping));
    assert!(
        reach.is_referenced(pong),
        "the handler record names the unused operation's function"
    );
    assert!(!reach.is_called(pong), "the unused operation does not run");
    assert!(
        !reach.externals().contains(&host),
        "the host is only called by an operation no reachable code performs"
    );
}

/// The operation slot is not the `MakeClosure` destination: it is a `Coerce` temporary of it.
/// The analysis follows the value, so the deferral still happens.
#[test]
fn handler_operation_follows_a_coerced_temporary() {
    let fx = Fixture::new();
    let mut m = Module::new(fx.core.clone());
    m.stage = Stage::ClosuresConverted;
    let host = m.add(leaf(&fx, "host_body"));
    let pong = m.add(caller(&fx, "pong_body", host));
    let ping = m.add(leaf(&fx, "ping_body"));

    let mut b = FnBuilder::new(&fx.core, "mk");
    let p = b.assign(
        "p",
        fx.string_to_unit,
        Value::MakeClosure {
            function: ping,
            captures: Vec::new(),
        },
    );
    let q0 = b.assign(
        "q0",
        fx.string_to_unit,
        Value::MakeClosure {
            function: pong,
            captures: Vec::new(),
        },
    );
    // The operation slot names a copy of the record, not the `make_closure` destination.
    let q = b.assign("q", fx.string_to_unit, Value::Coerce(q0));
    let h = b.assign(
        "h",
        fx.console_ty,
        Value::Handler {
            name: "Test".to_owned(),
            effect: fx.console,
            ops: vec![p, q],
        },
    );
    b.term(Term::Return(Some(h.into())));
    let mk = m.add(b.finish());

    let mut b = FnBuilder::new(&fx.core, "main");
    let h = b.assign(
        "h",
        fx.console_ty,
        Value::Call {
            callee: Callee::Direct(mk),
            args: Vec::new(),
            suspends: false,
            on_fail: None,
        },
    );
    b.let_(
        None,
        Value::Call {
            callee: Callee::Op {
                handler: h,
                effect: fx.console,
                op: 0,
            },
            args: vec![string("x")],
            suspends: false,
            on_fail: None,
        },
    );
    b.term(Term::Return(None));
    let main = m.add(b.finish());

    let reach = Reachability::walk(&m, &[main]);
    assert!(reach.is_called(ping));
    assert!(reach.is_referenced(pong));
    assert!(
        !reach.is_called(pong),
        "the coerced slot still belongs to the unperformed operation"
    );
}

/// An operation that is performed in another reachable function releases the deferred body.
#[test]
fn performing_the_operation_in_another_function_reaches_the_body() {
    let fx = Fixture::new();
    let mut m = Module::new(fx.core.clone());
    m.stage = Stage::ClosuresConverted;
    let pong = m.add(leaf(&fx, "pong_body"));

    let mut b = FnBuilder::new(&fx.core, "mk");
    let q = b.assign(
        "q",
        fx.string_to_unit,
        Value::MakeClosure {
            function: pong,
            captures: Vec::new(),
        },
    );
    let h = b.assign(
        "h",
        fx.console_ty,
        Value::Handler {
            name: "Test".to_owned(),
            effect: fx.console,
            ops: vec![q],
        },
    );
    b.term(Term::Return(Some(h.into())));
    let mk = m.add(b.finish());

    // The handler value is passed to another function, which performs the operation.
    let mut b = FnBuilder::new(&fx.core, "perform");
    let h = b.param("h", fx.console_ty);
    b.let_(
        None,
        Value::Call {
            callee: Callee::Op {
                handler: h,
                effect: fx.console,
                op: 0,
            },
            args: vec![string("x")],
            suspends: false,
            on_fail: None,
        },
    );
    b.term(Term::Return(None));
    let perform = m.add(b.finish());

    let mut b = FnBuilder::new(&fx.core, "main");
    let h = b.assign(
        "h",
        fx.console_ty,
        Value::Call {
            callee: Callee::Direct(mk),
            args: Vec::new(),
            suspends: false,
            on_fail: None,
        },
    );
    b.let_(
        None,
        Value::Call {
            callee: Callee::Direct(perform),
            args: vec![h.into()],
            suspends: false,
            on_fail: None,
        },
    );
    b.term(Term::Return(None));
    let main = m.add(b.finish());

    let reach = Reachability::walk(&m, &[main]);
    assert!(reach.performed().contains(&(fx.console, 0)));
    assert!(reach.is_called(pong));
}

/// Error `show` functions and const functions are roots: the entry does not call them, but the
/// runtime names them.
#[test]
fn error_shows_and_consts_are_roots() {
    let fx = Fixture::new();
    let mut m = Module::new(fx.core.clone());
    let show = m.add(leaf(&fx, "show$E"));
    let mut b = FnBuilder::new(&fx.core, "K");
    b.set_constant();
    b.term(Term::Return(None));
    let konst = m.add(b.finish());
    let mut b = FnBuilder::new(&fx.core, "main");
    b.term(Term::Return(None));
    let main = m.add(b.finish());
    m.error_shows.push((fx.parse_error, show));

    let reach = Reachability::walk(&m, &[main]);
    assert!(reach.is_called(show));
    assert!(reach.is_called(konst));
}

/// A reachable direct call to an external names it in `externals`, in first-met order.
#[test]
fn externals_are_recorded_in_call_order() {
    let fx = Fixture::new();
    let mut m = Module::new(fx.core.clone());
    let e2 = m.add(Function {
        name: "e2".to_owned(),
        external: Some(External {
            bindings: Vec::new(),
        }),
        ..leaf(&fx, "e2")
    });
    let e1 = m.add(Function {
        name: "e1".to_owned(),
        external: Some(External {
            bindings: Vec::new(),
        }),
        ..leaf(&fx, "e1")
    });
    let mut b = FnBuilder::new(&fx.core, "main");
    for callee in [e1, e2] {
        b.let_(
            None,
            Value::Call {
                callee: Callee::Direct(callee),
                args: Vec::new(),
                suspends: false,
                on_fail: None,
            },
        );
    }
    b.term(Term::Return(None));
    let main = m.add(b.finish());

    let reach = Reachability::walk(&m, &[main]);
    assert_eq!(reach.externals(), &[e1, e2]);
}

/// Pruning removes a function nothing names and remaps every `FnId` of the module, the roots and
/// the error shows; a referenced-only function stays.
#[test]
fn pruning_remaps_every_function_name() {
    let fx = Fixture::new();
    let mut m = Module::new(fx.core.clone());
    m.stage = Stage::ClosuresConverted;
    let show = m.add(leaf(&fx, "show$E"));
    let called = m.add(leaf(&fx, "called"));
    let only_from_stub = m.add(leaf(&fx, "only_from_stub"));
    // The referenced-only function's body names a function of its own: with the body replaced by
    // a stub, that name can be removed too.
    let referenced = m.add(caller(&fx, "referenced", only_from_stub));
    let unused = m.add(leaf(&fx, "unused"));
    m.error_shows.push((fx.parse_error, show));

    // fn main() { called(); h = handler { print_line: make_closure referenced[] }; return }
    // No reachable code performs the operation, so `referenced` is not called.
    let mut b = FnBuilder::new(&fx.core, "main");
    b.let_(
        None,
        Value::Call {
            callee: Callee::Direct(called),
            args: Vec::new(),
            suspends: false,
            on_fail: None,
        },
    );
    let f = b.assign(
        "f",
        fx.string_to_unit,
        Value::MakeClosure {
            function: referenced,
            captures: Vec::new(),
        },
    );
    b.assign(
        "h",
        fx.console_ty,
        Value::Handler {
            name: "Test".to_owned(),
            effect: fx.console,
            ops: vec![f],
        },
    );
    b.term(Term::Return(None));
    let main = m.add(b.finish());

    let reach = Reachability::walk(&m, &[main]);
    assert!(!reach.is_called(unused));
    assert!(!reach.is_referenced(unused));

    let prune = prune_functions(&mut m, &[main], &reach);
    assert_eq!(prune.removed, 2);
    assert_eq!(prune.roots.len(), 1);
    let names: Vec<&str> = m.functions.iter().map(|(_, f)| f.name.as_str()).collect();
    assert!(!names.contains(&"unused"), "{names:?}");
    assert!(!names.contains(&"only_from_stub"), "{names:?}");
    assert!(names.contains(&"called"));
    assert!(names.contains(&"referenced"));
    assert!(names.contains(&"show$E"));

    let (new_main, _) = m.functions.iter().find(|(_, f)| f.name == "main").unwrap();
    assert_eq!(prune.roots[0], new_main);

    // The `make_closure` still names `referenced`, and the error show is remapped.
    let main_fn = &m.functions[new_main];
    let target = main_fn.stmts.iter().find_map(|(_, s)| match s {
        emela_lir::Stmt::Let {
            value: Value::MakeClosure { function, .. },
            ..
        } => Some(*function),
        _ => None,
    });
    let referenced = target.unwrap();
    assert_eq!(m.functions[referenced].name, "referenced");
    // A referenced but never called body is a stub: its own callee's name is gone.
    let body = m.functions[referenced].body.unwrap();
    assert_eq!(m.functions[referenced].seqs[body].term, Term::Unreachable);
    let (_, show) = m.error_shows[0];
    assert_eq!(m.functions[show].name, "show$E");

    // The pruned module is still a consistent arena: every function name is an index, and the
    // root still finds its function.
    for (_, f) in m.functions.iter() {
        for (_, s) in f.stmts.iter() {
            if let emela_lir::Stmt::Let {
                value:
                    Value::Call {
                        callee: Callee::Direct(id),
                        ..
                    },
                ..
            } = s
            {
                let _ = &m.functions[*id];
            }
        }
    }
}
