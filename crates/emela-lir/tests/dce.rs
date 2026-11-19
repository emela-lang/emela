//! Tests for the LIR dead code elimination pass (C7/PR-028).
//!
//! The pass removes a `let` whose value is pure and whose destination is never read. The tests
//! build the Heap IR by hand (the shapes the counting pass emits) and lower a few programs to the
//! Heap IR, then run the pass through `opt::run_heap_with` with only DCE on. Every result must pass
//! the verifier.

#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

use emela_core::{Module as CoreModule, Roots, TypeId, monomorphize_from, verify_monomorphic};
use emela_lir::opt::{self, LirRules, Options};
use emela_lir::{
    Callee, Const, FnBuilder, FnId, Module, PrimOp, PrimTy, Stage, Stmt, Term, Value,
    dump_function, from_core, to_heap_with, verify,
};
use support::try_lower_with_opt;

/// Only DCE on, so a test sees exactly its effect (SRA and the `rc` cleanup stay off).
fn only_dce(dce: bool) -> Options {
    Options {
        rules: LirRules {
            sra: false,
            sra_records: false,
            devirt: false,
            sra_closures: false,
            rc_cleanup: false,
            rc_borrow: false,
            dce,
            drop_spec: false,
            borrow: false,
            reuse: false,
            switch: false,
        },
    }
}

/// Lowers, monomorphizes and converts to the Flow IR, with the Core optimisations off.
fn flow(files: &[(&str, &str)], roots: Roots) -> Module {
    let m = try_lower_with_opt(
        files,
        roots,
        emela_core::core_sources(),
        emela_core::bundled_files(),
        false,
    )
    .unwrap_or_else(|e| panic!("{e:#?}"));
    let fn_roots: Vec<_> = match roots {
        Roots::All => m.functions.iter().map(|(id, _)| id).collect(),
        _ => m
            .functions
            .iter()
            .filter(|(_, f)| f.exported)
            .map(|(id, _)| id)
            .collect(),
    };
    let (mono, _) = monomorphize_from(&m, &fn_roots);
    if let Err(errors) = verify_monomorphic(&mono) {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!("not monomorphic:\n{}", lines.join("\n"));
    }
    from_core(mono).unwrap_or_else(|errors| {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!("conversion errors:\n{}", lines.join("\n"));
    })
}

/// A program at the Heap IR, with every LIR optimisation off.
fn refcounted(src: &str) -> Module {
    let mut m = flow(&[("main.emel", src)], Roots::Exported);
    to_heap_with(&mut m, false);
    assert_eq!(m.stage, Stage::RefCounted);
    m
}

/// A module whose functions are built by hand, at the Heap IR stage.
struct Fixture {
    m: Module,
    /// `List[Int]`, for the `dup`/`drop` test.
    list: TypeId,
}

impl Fixture {
    fn new() -> Self {
        let mut core = CoreModule::new();
        let list = core.list(TypeId::INT);
        Fixture {
            m: Module::new(core),
            list,
        }
    }

    fn build(&mut self, name: &str, f: impl FnOnce(&mut FnBuilder<'_>)) -> FnId {
        let mut b = FnBuilder::new(&self.m.core, name);
        f(&mut b);
        let function = b.finish();
        self.m.add(function)
    }

    /// Runs the pass with only DCE on or off, at the stage of its precondition, and verifies.
    fn run(&mut self, dce: bool) -> opt::Stats {
        self.m.stage = Stage::RefCounted;
        let stats = opt::run_heap_with(&mut self.m, only_dce(dce));
        if let Err(errors) = verify(&self.m) {
            let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
            panic!("DCE produced invalid IR:\n{}", lines.join("\n"));
        }
        stats
    }

    fn dump(&self, id: FnId) -> String {
        dump_function(&self.m, &self.m.functions[id])
    }
}

/// An unused scalar literal is removed, and the verifier accepts the result.
#[test]
fn an_unused_scalar_literal_is_removed() {
    let mut fx = Fixture::new();
    let id = fx.build("literal", |b| {
        b.set_ret(TypeId::INT);
        let x = b.var("x", TypeId::INT);
        b.let_(Some(x), Value::Const(Const::Int(42)));
        b.term(Term::Return(Some(Const::Int(0).into())));
    });
    let stats = fx.run(true);
    assert_eq!(stats.dce, 1);
    assert_eq!(stats.dce_deopts, 0);
    let text = fx.dump(id);
    assert!(!text.contains("x ="), "{text}");
    assert!(verify(&fx.m).is_ok());
}

/// An unused pure arithmetic `let` is removed. A chain of them is removed in one run: removing the
/// last statement makes the definition it read dead in turn.
#[test]
fn unused_pure_arithmetic_is_removed_to_a_fixed_point() {
    let mut fx = Fixture::new();
    let id = fx.build("arithmetic", |b| {
        b.set_ret(TypeId::INT);
        let a = b.assign("a", TypeId::INT, Value::Const(Const::Int(1)));
        let c = b.assign(
            "c",
            TypeId::INT,
            Value::Prim {
                op: PrimOp::Add,
                ty: PrimTy::I32,
                args: vec![a.into(), Const::Int(2).into()],
            },
        );
        let d = b.assign(
            "d",
            TypeId::INT,
            Value::Prim {
                op: PrimOp::Mul,
                ty: PrimTy::I32,
                args: vec![c.into(), Const::Int(3).into()],
            },
        );
        let _ = d;
        b.term(Term::Return(Some(Const::Int(0).into())));
    });
    let stats = fx.run(true);
    assert_eq!(stats.dce, 3);
    let text = fx.dump(id);
    assert!(!text.contains(" = "), "{text}");
}

/// A used `let` is kept.
#[test]
fn a_used_let_is_kept() {
    let mut fx = Fixture::new();
    let id = fx.build("used", |b| {
        b.set_ret(TypeId::INT);
        let x = b.assign("x", TypeId::INT, Value::Const(Const::Int(42)));
        b.term(Term::Return(Some(x.into())));
    });
    let stats = fx.run(true);
    assert_eq!(stats.dce, 0);
    assert!(fx.dump(id).contains(" = 42"));
}

/// A call may effect, fail, defect or suspend, so an unused one is kept.
#[test]
fn an_unused_call_is_kept() {
    let mut fx = Fixture::new();
    let callee = fx.build("callee", |b| {
        b.set_ret(TypeId::INT);
        b.term(Term::Return(Some(Const::Int(0).into())));
    });
    let id = fx.build("caller", |b| {
        b.set_ret(TypeId::INT);
        b.let_(
            None,
            Value::Call {
                callee: Callee::Direct(callee),
                args: Vec::new(),
                suspends: false,
                on_fail: None,
            },
        );
        b.term(Term::Return(Some(Const::Int(7).into())));
    });
    let stats = fx.run(true);
    assert_eq!(stats.dce, 0);
    let text = fx.dump(id);
    assert!(text.contains("call callee"), "{text}");
}

/// Integer division may defect (a zero divisor), so an unused division is kept even when its
/// divisor is a nonzero constant (the pass does not reason about the value).
#[test]
fn an_unused_division_is_kept() {
    let mut fx = Fixture::new();
    let id = fx.build("division", |b| {
        b.set_ret(TypeId::INT);
        b.let_(
            None,
            Value::Prim {
                op: PrimOp::Div,
                ty: PrimTy::I32,
                args: vec![Const::Int(10).into(), Const::Int(2).into()],
            },
        );
        b.term(Term::Return(Some(Const::Int(0).into())));
    });
    let stats = fx.run(true);
    assert_eq!(stats.dce, 0);
    assert!(fx.dump(id).contains('/'));
}

/// A `dup` or `drop` is never removed: pairing them is the rc cleanup's job, and removing one
/// alone would unbalance the reference count.
#[test]
fn a_dup_or_drop_is_never_removed() {
    let mut fx = Fixture::new();
    let list = fx.list;
    let id = fx.build("references", |b| {
        b.set_ret(TypeId::INT);
        let p = b.param("p", list);
        b.stmt(Stmt::Dup(p));
        b.stmt(Stmt::Drop(p));
        b.stmt(Stmt::Drop(p));
        b.term(Term::Return(Some(Const::Int(0).into())));
    });
    let stats = fx.run(true);
    assert_eq!(stats.dce, 0);
    let text = fx.dump(id);
    assert!(text.contains("dup"), "{text}");
    assert!(text.contains("drop"), "{text}");
}

/// A variable read only by a defect landing is live on that way on, so its definition is kept.
#[test]
fn a_let_live_only_on_a_defect_landing_is_kept() {
    let mut fx = Fixture::new();
    let callee = fx.build("may_defect", |b| {
        b.set_ret(TypeId::INT);
        b.term(Term::Return(Some(Const::Int(0).into())));
    });
    fx.m.functions[callee].defects = true;
    let id = fx.build("landing", |b| {
        b.set_ret(TypeId::INT);
        let x = b.assign("x", TypeId::INT, Value::Const(Const::Int(5)));
        let landing = b.seq(|b| b.term(Term::Return(Some(x.into()))));
        let n = b.var("n", TypeId::INT);
        b.stmt(Stmt::Let {
            dst: Some(n),
            value: Value::Call {
                callee: Callee::Direct(callee),
                args: Vec::new(),
                suspends: false,
                on_fail: None,
            },
            on_defect: Some(landing),
            on_suspend: None,
        });
        b.term(Term::Return(Some(Const::Int(7).into())));
    });
    let stats = fx.run(true);
    assert_eq!(stats.dce, 0);
    assert!(fx.dump(id).contains(" = 5"));
}

/// The flag off leaves the module and the counters untouched.
#[test]
fn the_flag_off_leaves_the_module_identical() {
    let mut fx = Fixture::new();
    fx.build("literal", |b| {
        b.set_ret(TypeId::INT);
        let x = b.var("x", TypeId::INT);
        b.let_(Some(x), Value::Const(Const::Int(42)));
        b.term(Term::Return(Some(Const::Int(0).into())));
    });
    fx.m.stage = Stage::RefCounted;
    let before = emela_lir::dump(&fx.m);
    let stats = fx.run(false);
    assert_eq!(stats.dce, 0);
    assert_eq!(stats.dce_deopts, 0);
    assert_eq!(emela_lir::dump(&fx.m), before);
}

/// A second run removes nothing and leaves the dump identical.
#[test]
fn the_pass_is_idempotent() {
    let mut fx = Fixture::new();
    fx.build("idem", |b| {
        b.set_ret(TypeId::INT);
        let a = b.assign("a", TypeId::INT, Value::Const(Const::Int(1)));
        let c = b.assign(
            "c",
            TypeId::INT,
            Value::Prim {
                op: PrimOp::Add,
                ty: PrimTy::I32,
                args: vec![a.into(), Const::Int(2).into()],
            },
        );
        let _ = c;
        b.term(Term::Return(Some(Const::Int(0).into())));
    });
    assert_eq!(fx.run(true).dce, 2);
    let after_first = emela_lir::dump(&fx.m);
    let stats = fx.run(true);
    assert_eq!(stats.dce, 0);
    assert_eq!(emela_lir::dump(&fx.m), after_first);
}

/// A runtime operation that prints is an effect: it is kept even with no destination.
#[test]
fn an_unremovable_effect_is_kept() {
    let mut fx = Fixture::new();
    let id = fx.build("effect", |b| {
        b.set_ret(TypeId::INT);
        b.let_(
            None,
            Value::Runtime {
                op: emela_lir::RuntimeOp::DebugPrint,
                args: vec![Const::String("kept".into()).into()],
            },
        );
        b.term(Term::Return(Some(Const::Int(0).into())));
    });
    let stats = fx.run(true);
    assert_eq!(stats.dce, 0);
    assert!(fx.dump(id).contains("debug_print"));
}

/// An unused `make`, tuple or list allocates a reference; the counting pass gives each a `drop`,
/// which makes the variable live, and the value itself is not pure. Either way it is kept.
#[test]
fn an_unused_allocation_is_kept() {
    let mut m = refcounted(
        "\
enum Thing {
  One(n: Int),
  Empty,
}

fn main() -> Int {
  e = One(1)
  t = (1, 2)
  l = [1, 2]
  0
}
",
    );
    let before = emela_lir::dump(&m);
    let stats = opt::run_heap_with(&mut m, only_dce(true));
    assert_eq!(stats.dce, 0, "{stats:?}");
    assert_eq!(emela_lir::dump(&m), before);
    assert!(before.contains("make "), "{before}");
    assert!(before.contains("tuple("), "{before}");
    assert!(before.contains("list["), "{before}");
}

/// Liveness has no answer before the Heap IR, so the pass changes nothing and counts a deopt per
/// function, instead of reading `Unknown` as "not live".
#[test]
fn unknown_liveness_deopts_the_whole_function() {
    let mut m = flow(
        &[(
            "main.emel",
            "\
fn main() -> Int {
  x = 1
  0
}
",
        )],
        Roots::Exported,
    );
    assert_eq!(m.stage, Stage::Flow);
    let before = emela_lir::dump(&m);
    let functions = m.functions.iter().filter(|(_, f)| f.body.is_some()).count();
    let stats = opt::dce::run(&mut m);
    assert_eq!(stats.dce, 0);
    assert_eq!(stats.dce_deopts as usize, functions);
    assert_eq!(emela_lir::dump(&m), before);
}
