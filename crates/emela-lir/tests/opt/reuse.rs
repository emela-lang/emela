//! Perceus reuse (C4/PR-042): a uniquely owned record of the same constructor is written again
//! instead of being dropped and allocated, releasing the references its old fields held.
//!
//! The pass is off by default, so each test builds a RefCounted function with the `drop` and the
//! `Make` a reference-counted program would have and runs [`emela_lir::opt::reuse`] directly. The
//! backend half lives in `crates/emela-driver/tests/reuse.rs`.

use emela_core::{CtorRef, EnumDef, Module as CoreModule, TypeId, VariantDef};
use emela_lir::opt::reuse;
use emela_lir::opt::{LirRules, Options, run_heap, run_heap_with};
use emela_lir::{
    Callee, Const, FnBuilder, FnId, Module, Stage, Stmt, StmtId, Suspend, Term, Value, VarId,
    verify,
};

/// An enum with one constructor taking `tys`, interned in `core`.
fn enum_type(core: &mut CoreModule, name: &str, tys: Vec<TypeId>) -> (TypeId, CtorRef) {
    let id = core.enums.alloc(EnumDef {
        name: name.to_owned(),
        params: 0,
        variants: vec![VariantDef::positional(name, tys)],
        error: false,
        external: false,
    });
    let ty = core.enum_type(id, Vec::new());
    (
        ty,
        CtorRef {
            enum_id: id,
            variant: 0,
        },
    )
}

/// The IDs of the pair a test looks at.
struct Built {
    m: Module,
    f: FnId,
    old: VarId,
    new: VarId,
    make: StmtId,
    drop: StmtId,
}

/// A function `let a = Make(1, 2); drop a; let b = Make(3, 4); drop b; return ()`, with a scalar
/// `let` between the drop and the `Make` when `adjacent` is false.
fn build(core: &CoreModule, ctor: CtorRef, ty: TypeId, adjacent: bool) -> Built {
    let mut b = FnBuilder::new(core, "f");
    let old = b.assign(
        "a",
        ty,
        Value::Make {
            ctor,
            args: vec![Const::Int(1).into(), Const::Int(2).into()],
        },
    );
    let drop = b.stmt(Stmt::Drop(old));
    if !adjacent {
        b.assign("n", TypeId::INT, Value::Const(Const::Int(0)));
    }
    let new = b.var("b", ty);
    let make = b.stmt(Stmt::Let {
        dst: Some(new),
        value: Value::Make {
            ctor,
            args: vec![Const::Int(3).into(), Const::Int(4).into()],
        },
        on_defect: None,
        on_suspend: None,
    });
    b.stmt(Stmt::Drop(new));
    b.term(Term::Return(None));
    let f = b.finish();
    let mut m = Module::new(core.clone());
    let id = m.add(f);
    m.stage = Stage::RefCounted;
    Built {
        m,
        f: id,
        old,
        new,
        make,
        drop,
    }
}

/// Whether the sequence of the body still holds `stmt`.
fn holds(m: &Module, f: FnId, stmt: StmtId) -> bool {
    m.functions[f]
        .body
        .is_some_and(|body| m.functions[f].seqs[body].stmts.contains(&stmt))
}

/// A scalar-field single-variant record is reused: the marker is written, the drop is gone, the
/// module still verifies and dumping shows the difference.
#[test]
fn a_scalar_record_is_reused() {
    let mut core = CoreModule::new();
    let (ty, ctor) = enum_type(&mut core, "Point", vec![TypeId::INT, TypeId::INT]);
    let Built {
        mut m,
        f,
        old,
        new,
        make,
        drop,
    } = build(&core, ctor, ty, true);
    assert!(verify(&m).is_ok(), "the test module does not verify");
    let before = emela_lir::dump(&m);

    let stats = reuse::run(&mut m);

    assert_eq!(stats.reuse, 1, "the pair was not reused: {stats:?}");
    assert_eq!(stats.reuse_deopts, 0);
    assert_eq!(m.functions[f].reuse.get(&make), Some(&old));
    assert!(!holds(&m, f, drop), "the old drop is still there");
    assert!(holds(&m, f, make), "the Make was removed");
    assert_eq!(m.functions[f].vars[new].repr, emela_lir::Repr::Ref);
    assert_ne!(emela_lir::dump(&m), before, "the dump did not change");
    assert!(verify(&m).is_ok(), "the rewritten module does not verify");
}

/// Running the pass again finds no drop followed by a `Make` and changes nothing.
#[test]
fn the_pass_is_idempotent() {
    let mut core = CoreModule::new();
    let (ty, ctor) = enum_type(&mut core, "Point", vec![TypeId::INT, TypeId::INT]);
    let Built { mut m, f, .. } = build(&core, ctor, ty, true);
    let first = reuse::run(&mut m);
    assert_eq!(first.reuse, 1);
    let after = emela_lir::dump(&m);
    let second = reuse::run(&mut m);
    assert_eq!(second.reuse, 0, "a second run reused again: {second:?}");
    assert_eq!(second.reuse_deopts, 0);
    assert_eq!(emela_lir::dump(&m), after);
    assert_eq!(m.functions[f].reuse.len(), 1);
}

/// A second use (a `dup`) disproves uniqueness: the drop and the allocation stay.
#[test]
fn a_value_with_another_use_deopts() {
    // The reference is duplicated and both copies are dropped, so the last drop is adjacent to the
    // Make and the value is read three times in all (the `dup` and two drops).
    let mut core = CoreModule::new();
    let (ty, ctor) = enum_type(&mut core, "Point", vec![TypeId::INT, TypeId::INT]);
    let mut b = FnBuilder::new(&core, "f");
    let a = b.assign(
        "a",
        ty,
        Value::Make {
            ctor,
            args: vec![Const::Int(1).into(), Const::Int(2).into()],
        },
    );
    b.stmt(Stmt::Dup(a));
    b.stmt(Stmt::Drop(a));
    let drop = b.stmt(Stmt::Drop(a));
    let new = b.var("b", ty);
    let make = b.stmt(Stmt::Let {
        dst: Some(new),
        value: Value::Make {
            ctor,
            args: vec![Const::Int(3).into(), Const::Int(4).into()],
        },
        on_defect: None,
        on_suspend: None,
    });
    b.stmt(Stmt::Drop(new));
    b.term(Term::Return(None));
    let f = b.finish();
    let mut m = Module::new(core.clone());
    let id = m.add(f);
    m.stage = Stage::RefCounted;
    assert!(verify(&m).is_ok());

    let stats = reuse::run(&mut m);

    assert_eq!(stats.reuse, 0);
    assert_eq!(stats.reuse_deopts, 1);
    assert_eq!(m.functions[id].reuse.get(&make), None);
    assert!(holds(&m, id, drop));
    assert!(holds(&m, id, make));
    assert!(verify(&m).is_ok());
}

/// A view of the value is a second use: the pair deopts.
#[test]
fn a_view_of_the_value_deopts() {
    let mut core = CoreModule::new();
    let (ty, ctor) = enum_type(&mut core, "Point", vec![TypeId::INT, TypeId::INT]);
    let mut b = FnBuilder::new(&core, "f");
    let a = b.assign(
        "a",
        ty,
        Value::Make {
            ctor,
            args: vec![Const::Int(1).into(), Const::Int(2).into()],
        },
    );
    let x = b.assign(
        "x",
        TypeId::INT,
        Value::Field {
            base: a,
            ctor,
            index: 0,
        },
    );
    let drop = b.stmt(Stmt::Drop(a));
    let new = b.var("b", ty);
    let make = b.stmt(Stmt::Let {
        dst: Some(new),
        value: Value::Make {
            ctor,
            args: vec![x.into(), Const::Int(4).into()],
        },
        on_defect: None,
        on_suspend: None,
    });
    b.stmt(Stmt::Drop(new));
    b.term(Term::Return(None));
    let f = b.finish();
    let mut m = Module::new(core.clone());
    let id = m.add(f);
    m.stage = Stage::RefCounted;
    assert!(verify(&m).is_ok());

    let stats = reuse::run(&mut m);

    assert_eq!(stats.reuse, 0);
    assert_eq!(stats.reuse_deopts, 1);
    assert!(holds(&m, id, drop));
    assert!(holds(&m, id, make));
}

/// A record with a reference field is reused: the pass marks it and the drop is gone. The
/// backends release the old field before writing the new one (the runtime tests exercise that);
/// the LIR keeps the same shape as for a scalar field.
#[test]
fn a_ref_field_record_is_reused() {
    let mut core = CoreModule::new();
    let (ty, ctor) = enum_type(&mut core, "Named", vec![TypeId::STRING, TypeId::INT]);
    let mut b = FnBuilder::new(&core, "f");
    let s = b.assign(
        "s",
        TypeId::STRING,
        Value::Const(Const::String("old".into())),
    );
    let a = b.assign(
        "a",
        ty,
        Value::Make {
            ctor,
            args: vec![s.into(), Const::Int(1).into()],
        },
    );
    let t = b.assign(
        "t",
        TypeId::STRING,
        Value::Const(Const::String("new".into())),
    );
    let drop = b.stmt(Stmt::Drop(a));
    let new = b.var("b", ty);
    let make = b.stmt(Stmt::Let {
        dst: Some(new),
        value: Value::Make {
            ctor,
            args: vec![t.into(), Const::Int(2).into()],
        },
        on_defect: None,
        on_suspend: None,
    });
    b.stmt(Stmt::Drop(new));
    b.term(Term::Return(None));
    let f = b.finish();
    let mut m = Module::new(core.clone());
    let id = m.add(f);
    m.stage = Stage::RefCounted;
    assert!(verify(&m).is_ok(), "{:?}", verify(&m).unwrap_err());

    let stats = reuse::run(&mut m);

    assert_eq!(stats.reuse, 1, "the pair was not reused: {stats:?}");
    assert_eq!(stats.reuse_deopts, 0);
    assert_eq!(m.functions[id].reuse.get(&make), Some(&a));
    assert!(!holds(&m, id, drop), "the old drop is still there");
    assert!(holds(&m, id, make), "the Make was removed");
    assert!(verify(&m).is_ok());

    // A second run finds no drop followed by a `Make`.
    let after = emela_lir::dump(&m);
    let second = reuse::run(&mut m);
    assert_eq!(second.reuse, 0, "a second run reused again: {second:?}");
    assert_eq!(emela_lir::dump(&m), after);
}

/// The same constructor with another type argument has another reference layout (the reference
/// may move to another slot), so the block cannot be written with the other construction's
/// fields: deopt.
#[test]
fn a_value_of_another_instantiation_deopts() {
    let mut core = CoreModule::new();
    let param = core.type_param(0);
    let id = core.enums.alloc(EnumDef {
        name: "Box".to_owned(),
        params: 1,
        variants: vec![VariantDef::positional("Box", vec![param])],
        error: false,
        external: false,
    });
    let boxed_int = core.enum_type(id, vec![TypeId::INT]);
    let boxed_text = core.enum_type(id, vec![TypeId::STRING]);
    let ctor = CtorRef {
        enum_id: id,
        variant: 0,
    };
    let mut b = FnBuilder::new(&core, "f");
    let a = b.assign(
        "a",
        boxed_int,
        Value::Make {
            ctor,
            args: vec![Const::Int(1).into()],
        },
    );
    let s = b.assign(
        "s",
        TypeId::STRING,
        Value::Const(Const::String("new".into())),
    );
    let drop = b.stmt(Stmt::Drop(a));
    let new = b.var("b", boxed_text);
    let make = b.stmt(Stmt::Let {
        dst: Some(new),
        value: Value::Make {
            ctor,
            args: vec![s.into()],
        },
        on_defect: None,
        on_suspend: None,
    });
    b.stmt(Stmt::Drop(new));
    b.term(Term::Return(None));
    let f = b.finish();
    let mut m = Module::new(core);
    let fid = m.add(f);
    m.stage = Stage::RefCounted;
    assert!(verify(&m).is_ok(), "{:?}", verify(&m).unwrap_err());

    let stats = reuse::run(&mut m);

    assert_eq!(stats.reuse, 0, "another instantiation was reused");
    assert_eq!(stats.reuse_deopts, 1);
    assert!(holds(&m, fid, drop));
    assert!(holds(&m, fid, make));
    assert!(verify(&m).is_ok());
}

/// A reference-field record whose old value has a second use still deopts: rewriting the block
/// would be observable through that use.
#[test]
fn a_ref_field_value_with_another_use_deopts() {
    let mut core = CoreModule::new();
    let (ty, ctor) = enum_type(&mut core, "Named", vec![TypeId::STRING, TypeId::INT]);
    let mut b = FnBuilder::new(&core, "f");
    b.set_ret(TypeId::STRING);
    let s = b.assign(
        "s",
        TypeId::STRING,
        Value::Const(Const::String("old".into())),
    );
    let a = b.assign(
        "a",
        ty,
        Value::Make {
            ctor,
            args: vec![s.into(), Const::Int(1).into()],
        },
    );
    let t = b.assign(
        "t",
        TypeId::STRING,
        Value::Const(Const::String("new".into())),
    );
    let x = b.assign(
        "x",
        TypeId::STRING,
        Value::Field {
            base: a,
            ctor,
            index: 0,
        },
    );
    b.stmt(Stmt::Dup(x));
    let drop = b.stmt(Stmt::Drop(a));
    let new = b.var("b", ty);
    let make = b.stmt(Stmt::Let {
        dst: Some(new),
        value: Value::Make {
            ctor,
            args: vec![t.into(), Const::Int(2).into()],
        },
        on_defect: None,
        on_suspend: None,
    });
    b.stmt(Stmt::Drop(new));
    b.term(Term::Return(Some(x.into())));
    let f = b.finish();
    let mut m = Module::new(core);
    let id = m.add(f);
    m.stage = Stage::RefCounted;
    assert!(verify(&m).is_ok(), "{:?}", verify(&m).unwrap_err());

    let stats = reuse::run(&mut m);

    assert_eq!(stats.reuse, 0);
    assert_eq!(stats.reuse_deopts, 1);
    assert!(holds(&m, id, drop));
    assert!(holds(&m, id, make));
}

/// A different constructor has another tag and descriptor: deopt.
#[test]
fn a_different_constructor_deopts() {
    let mut core = CoreModule::new();
    let id = core.enums.alloc(EnumDef {
        name: "Two".to_owned(),
        params: 0,
        variants: vec![
            VariantDef::positional("P", vec![TypeId::INT, TypeId::INT]),
            VariantDef::positional("Q", vec![TypeId::INT, TypeId::INT]),
        ],
        error: false,
        external: false,
    });
    let ty = core.enum_type(id, Vec::new());
    let p = CtorRef {
        enum_id: id,
        variant: 0,
    };
    let q = CtorRef {
        enum_id: id,
        variant: 1,
    };
    let mut b = FnBuilder::new(&core, "f");
    let a = b.assign(
        "a",
        ty,
        Value::Make {
            ctor: p,
            args: vec![Const::Int(1).into(), Const::Int(2).into()],
        },
    );
    let drop = b.stmt(Stmt::Drop(a));
    let new = b.var("b", ty);
    let make = b.stmt(Stmt::Let {
        dst: Some(new),
        value: Value::Make {
            ctor: q,
            args: vec![Const::Int(3).into(), Const::Int(4).into()],
        },
        on_defect: None,
        on_suspend: None,
    });
    b.stmt(Stmt::Drop(new));
    b.term(Term::Return(None));
    let f = b.finish();
    let mut m = Module::new(core);
    let fid = m.add(f);
    m.stage = Stage::RefCounted;
    assert!(verify(&m).is_ok());

    let stats = reuse::run(&mut m);

    assert_eq!(stats.reuse, 0);
    assert_eq!(stats.reuse_deopts, 1);
    assert!(holds(&m, fid, drop));
    assert!(holds(&m, fid, make));
}

/// An error value and an `@external` type (a finalizer) keep the drop and the allocation.
#[test]
fn an_error_or_host_type_deopts() {
    for (error, external) in [(true, false), (false, true)] {
        let mut core = CoreModule::new();
        let id = core.enums.alloc(EnumDef {
            name: "Special".to_owned(),
            params: 0,
            variants: vec![VariantDef::positional(
                "Special",
                vec![TypeId::INT, TypeId::INT],
            )],
            error,
            external,
        });
        let ty = core.enum_type(id, Vec::new());
        let ctor = CtorRef {
            enum_id: id,
            variant: 0,
        };
        let Built {
            mut m,
            f,
            make,
            drop,
            ..
        } = build(&core, ctor, ty, true);
        assert!(verify(&m).is_ok());

        let stats = reuse::run(&mut m);

        assert_eq!(stats.reuse, 0, "error={error} external={external}");
        assert_eq!(stats.reuse_deopts, 1);
        assert!(holds(&m, f, drop));
        assert!(holds(&m, f, make));
    }
}

/// A value the backend holds by value (`Repr` other than `Ref`) has no block to reuse: the pass
/// skips it. The LIR has no by-value representation for a record, so the test sets one.
#[test]
fn a_value_held_by_value_is_skipped() {
    let mut core = CoreModule::new();
    let (ty, ctor) = enum_type(&mut core, "Point", vec![TypeId::INT, TypeId::INT]);
    let Built {
        mut m,
        f,
        old,
        new,
        make,
        drop,
    } = build(&core, ctor, ty, true);
    // What the C backend's by-value layout does not tell the LIR: the value is not a reference.
    m.functions[f].vars[old].repr = emela_lir::Repr::I32;
    m.functions[f].vars[new].repr = emela_lir::Repr::I32;

    let stats = reuse::run(&mut m);

    assert_eq!(stats.reuse, 0);
    assert_eq!(stats.reuse_deopts, 1);
    assert!(holds(&m, f, drop));
    assert!(holds(&m, f, make));
}

/// A value a suspension frame saves is read by the frame: deopt.
#[test]
fn a_frame_saved_value_deopts() {
    let mut core = CoreModule::new();
    let (ty, ctor) = enum_type(&mut core, "Point", vec![TypeId::INT, TypeId::INT]);
    // A suspending callee that may not defect: the call needs a suspension edge, not a landing.
    let g = {
        let mut b = FnBuilder::new(&core, "g");
        b.set_suspends();
        b.set_ret(TypeId::INT);
        let n = b.assign("n", TypeId::INT, Value::Const(Const::Int(0)));
        b.term(Term::Return(Some(n.into())));
        b.finish()
    };
    let mut m = Module::new(core);
    let gid = m.add(g);
    let (func, drop, make, new) = {
        let mut b = FnBuilder::new(&m.core, "f");
        b.set_suspends();
        let a = b.assign(
            "a",
            ty,
            Value::Make {
                ctor,
                args: vec![Const::Int(1).into(), Const::Int(2).into()],
            },
        );
        b.stmt(Stmt::Let {
            dst: None,
            value: Value::Call {
                callee: Callee::Direct(gid),
                args: Vec::new(),
                suspends: true,
                on_fail: None,
            },
            on_defect: None,
            on_suspend: Some(Suspend {
                state: 0,
                saved: vec![a],
            }),
        });
        let drop = b.stmt(Stmt::Drop(a));
        let new = b.var("b", ty);
        let make = b.stmt(Stmt::Let {
            dst: Some(new),
            value: Value::Make {
                ctor,
                args: vec![Const::Int(3).into(), Const::Int(4).into()],
            },
            on_defect: None,
            on_suspend: None,
        });
        b.stmt(Stmt::Drop(new));
        b.term(Term::Return(None));
        let f = b.finish();
        (f, drop, make, new)
    };
    let f = m.add(func);
    let _ = new;
    m.stage = Stage::RefCounted;
    assert!(verify(&m).is_ok(), "{:?}", verify(&m).unwrap_err());

    let stats = reuse::run(&mut m);

    assert_eq!(stats.reuse, 0);
    assert_eq!(stats.reuse_deopts, 1);
    assert!(holds(&m, f, drop));
    assert!(holds(&m, f, make));
}

/// A statement between the drop and the `Make` (here the `Prim` of a dead scalar) is not adjacent:
/// the pair is left alone and the statements are not even counted as candidates.
#[test]
fn a_non_adjacent_pair_is_not_a_candidate() {
    let mut core = CoreModule::new();
    let (ty, ctor) = enum_type(&mut core, "Point", vec![TypeId::INT, TypeId::INT]);
    let Built { mut m, .. } = build(&core, ctor, ty, false);
    let stats = reuse::run(&mut m);
    assert_eq!(stats.reuse, 0);
    assert_eq!(stats.reuse_deopts, 0);
}

/// With the flag off nothing runs: the dump is the same and the counters stay zero. `run_heap`
/// uses the default rules, where reuse is opt-in.
#[test]
fn the_flag_off_leaves_the_heap_ir_alone() {
    let mut core = CoreModule::new();
    let (ty, ctor) = enum_type(&mut core, "Point", vec![TypeId::INT, TypeId::INT]);
    let Built { mut m, .. } = build(&core, ctor, ty, true);
    let before = emela_lir::dump(&m);
    let stats = run_heap(&mut m, true);
    assert_eq!(stats.reuse, 0);
    assert_eq!(stats.reuse_deopts, 0);
    assert_eq!(emela_lir::dump(&m), before);

    let mut core = CoreModule::new();
    let (ty, ctor) = enum_type(&mut core, "Point", vec![TypeId::INT, TypeId::INT]);
    let Built { mut m, .. } = build(&core, ctor, ty, true);
    let before = emela_lir::dump(&m);
    let stats = run_heap_with(
        &mut m,
        Options {
            rules: LirRules {
                reuse: true,
                ..LirRules::none()
            },
        },
    );
    assert_eq!(stats.reuse, 1);
    assert_ne!(emela_lir::dump(&m), before);
}

/// A module before the Heap IR has no drop and no uniqueness answer: every function is a deopt and
/// nothing changes.
#[test]
fn a_module_before_the_heap_ir_deopts() {
    let mut core = CoreModule::new();
    let (ty, ctor) = enum_type(&mut core, "Point", vec![TypeId::INT, TypeId::INT]);
    let Built { mut m, .. } = build(&core, ctor, ty, true);
    m.stage = Stage::Flow;
    let before = emela_lir::dump(&m);
    let stats = reuse::run(&mut m);
    assert_eq!(stats.reuse, 0);
    assert_eq!(stats.reuse_deopts, 1);
    assert_eq!(emela_lir::dump(&m), before);
}
