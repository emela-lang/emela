//! Suspension frame pruning (C7/PR-028): the corpus has no stale frame slot, so the pass has no
//! candidates and is not implemented (see [`emela_lir::opt::frame_prune`]).
//!
//! The tests lower every program of the local corpus to the Heap IR with the default LIR passes,
//! scan the suspension frames and assert the documented result. The zero is not "the analysis found
//! nothing to look at": hand-built modules of the shapes that produce a stale slot (a saved scalar
//! nothing reads after the value, which `dce` can leave behind by removing a dead pure `let`, and a
//! saved reference) are scanned as controls, together with the two reasons a stale reference stays
//! saved (a finalizer and a kept view of it).

#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};

use emela_core::{Module as CoreModule, Roots, TypeId, monomorphize_from, verify_monomorphic};
use emela_lir::opt::frame_prune::{self, Report};
use emela_lir::opt::{self, Options, Stats};
use emela_lir::{
    Const, FnBuilder, Module, RuntimeOp, Stage, Stmt, Suspend, Term, Value, VarId, from_core,
    to_heap_with_options, verify,
};
use support::try_lower_with_opt;

/// Lowers, monomorphizes, converts to the Flow IR and runs the Heap IR with the default options.
fn heap(files: &[(&str, &str)], roots: Roots) -> (Module, Stats) {
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
    let mut lir = from_core(mono).unwrap_or_else(|errors| {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!("conversion errors:\n{}", lines.join("\n"));
    });
    let stats = to_heap_with_options(&mut lir, true, Options::default());
    if let Err(errors) = verify(&lir) {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!("Heap IR verifier errors:\n{}", lines.join("\n"));
    }
    (lir, stats)
}

/// The totals the scan found, plus what the passes that run before it changed.
#[derive(Default)]
struct Totals {
    report: Report,
    programs: u32,
    /// Programs on which the scan would prune a slot.
    candidates: u32,
    /// Statements `dce` removed: the only pass that can make a saved slot stale.
    dce: u32,
    /// `dup`/`drop` pairs `rc_cleanup` removed.
    rc_pairs: u32,
}

impl Totals {
    fn add(&mut self, program: &str, report: Report, stats: Stats) {
        self.programs += 1;
        let before = report.stale_scalars + report.stale_refs;
        self.report.edges += report.edges;
        self.report.unknown_functions += report.unknown_functions;
        self.report.slots += report.slots;
        self.report.stale += report.stale;
        self.report.stale_scalars += report.stale_scalars;
        self.report.stale_refs += report.stale_refs;
        self.report.kept_finalizers += report.kept_finalizers;
        self.report.kept_views += report.kept_views;
        self.dce += stats.dce;
        self.rc_pairs += stats.rc_pairs;
        if before > 0 {
            self.candidates += 1;
            println!("{program}: {report:?} has a stale slot");
        }
    }

    fn show(&self, label: &str) {
        println!(
            "{label}: {} programs, {:?}, {} with a stale slot; dce removed {}, rc_cleanup removed {} pairs",
            self.programs, self.report, self.candidates, self.dce, self.rc_pairs
        );
    }
}

/// Every C codegen test program goes through the default Heap IR; the scan finds no stale frame
/// slot in any of them.
#[test]
fn stale_frame_slots_on_the_c_corpus() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../emela-codegen-c/tests/programs");
    let mut programs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "emel"))
        .collect();
    programs.sort();
    assert!(!programs.is_empty());
    let mut total = Totals::default();
    for path in programs {
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let src = std::fs::read_to_string(&path).unwrap();
        let (m, stats) = heap(&[("main.emel", src.as_str())], Roots::Exported);
        total.add(&name, frame_prune::scan(&m), stats);
    }
    total.show("the C corpus");
    // 803 suspension edges, 1,344 saved slots, none stale (2026-11-19). The obstacle is the
    // pipeline, not the rule: the saved set is fixed before reference counting, `rc_cleanup`
    // refuses a pair whose variable a frame saves, and `dce` removes none of these programs'
    // statements, so no saved entry can lose its last use after the value.
    assert!(total.report.edges > 0, "the corpus has no suspension edge");
    assert_eq!(
        total.report.stale, 0,
        "a stale saved slot appeared; frame pruning now has a candidate: {:?}",
        total.report
    );
    assert_eq!(total.candidates, 0);
}

/// Every example project goes through the default Heap IR; the scan finds no stale frame slot.
#[test]
fn stale_frame_slots_on_the_examples() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut projects: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path().join("src"))
        .filter(|p| p.join("main.emel").exists())
        .collect();
    projects.sort();
    assert!(!projects.is_empty());
    let mut total = Totals::default();
    for src in projects {
        let mut files: Vec<(String, String)> = std::fs::read_dir(&src)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "emel"))
            .map(|p| {
                let name = p.file_name().unwrap().to_string_lossy().into_owned();
                (name, std::fs::read_to_string(&p).unwrap())
            })
            .collect();
        files.sort_by_key(|(name, _)| (name != "main.emel", name.clone()));
        let files: Vec<(&str, &str)> = files
            .iter()
            .map(|(n, s)| (n.as_str(), s.as_str()))
            .collect();
        let name = src.parent().unwrap().file_name().unwrap().to_string_lossy();
        let (m, stats) = heap(&files, Roots::All);
        total.add(&name, frame_prune::scan(&m), stats);
    }
    total.show("the examples");
    assert!(
        total.report.edges > 0,
        "the examples have no suspension edge"
    );
    assert_eq!(total.report.stale, 0, "{:?}", total.report);
}

/// A `yield` with a defect landing that owns nothing, saving `saved`.
fn suspend(b: &mut FnBuilder<'_>, saved: Vec<VarId>) -> Stmt {
    let landing = b.seq(|b| b.term(Term::Unwind));
    Stmt::Let {
        dst: None,
        value: Value::Runtime {
            op: RuntimeOp::Yield,
            args: Vec::new(),
        },
        on_defect: Some(landing),
        on_suspend: Some(Suspend { state: 0, saved }),
    }
}

/// A hand-built module with one suspending function, at `stage`.
fn module(core: CoreModule, mut function: emela_lir::Function, stage: Stage) -> Module {
    // A yield may raise a defect, so its landing ends in `unwind`.
    function.defects = true;
    let mut m = Module::new(core);
    m.add(function);
    m.stage = stage;
    m
}

/// The scan is not vacuous: a saved scalar that nothing reads after the value is a stale slot. The
/// verifier accepts the module, so `saved_errors` does not require the entry: the frame may drop it.
#[test]
fn the_scan_finds_the_only_shape_that_makes_a_slot_stale() {
    let core = CoreModule::new();
    let mut b = FnBuilder::new(&core, "stale_scalar");
    b.set_ret(TypeId::INT);
    b.set_suspends();
    let v = b.assign("v", TypeId::INT, Value::Const(Const::Int(1)));
    let stmt = suspend(&mut b, vec![v]);
    b.stmt(stmt);
    b.term(Term::Return(Some(Const::Int(0).into())));
    let function = b.finish();
    let m = module(core, function, Stage::RefCounted);

    let report = frame_prune::scan(&m);
    assert_eq!(report.edges, 1, "{report:?}");
    assert_eq!(report.stale, 1, "{report:?}");
    assert_eq!(report.stale_scalars, 1, "{report:?}");
    assert_eq!(report.stale_refs, 0, "{report:?}");
    assert!(report.has_candidates());
    if let Err(errors) = verify(&m) {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!("{}", lines.join("\n"));
    }
}

/// A stale reference is a different kind of candidate: it owns a reference, so pruning it would need
/// a `drop` before the value. The scan separates it from a scalar.
#[test]
fn the_scan_separates_a_stale_reference_from_a_scalar() {
    let core = CoreModule::new();
    let mut b = FnBuilder::new(&core, "stale_ref");
    b.set_ret(TypeId::INT);
    b.set_suspends();
    let s = b.assign(
        "s",
        TypeId::STRING,
        Value::Const(Const::String("dead".into())),
    );
    let stmt = suspend(&mut b, vec![s]);
    b.stmt(stmt);
    b.term(Term::Return(Some(Const::Int(0).into())));
    // The state machines, so the hand-built edge is the only reference fact that matters; the
    // balance of the Heap IR is not what this test is about.
    let function = b.finish();
    let m = module(core, function, Stage::StateMachines);

    let report = frame_prune::scan(&m);
    assert_eq!(report.stale, 1, "{report:?}");
    assert_eq!(report.stale_scalars, 0, "{report:?}");
    assert_eq!(report.stale_refs, 1, "{report:?}");
    assert_eq!(report.kept_finalizers, 0, "{report:?}");
    if let Err(errors) = verify(&m) {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!("{}", lines.join("\n"));
    }
}

/// A stale reference with a finalizer is kept: a `drop` before the suspension would run the
/// finalizer earlier than the frame's release does.
#[test]
fn the_scan_keeps_a_stale_reference_with_a_finalizer() {
    let core = CoreModule::new();
    let mut b = FnBuilder::new(&core, "finalizer");
    b.set_ret(TypeId::INT);
    b.set_suspends();
    let a = b.param("a", TypeId::ANY);
    let stmt = suspend(&mut b, vec![a]);
    b.stmt(stmt);
    b.term(Term::Return(Some(Const::Int(0).into())));
    let function = b.finish();
    let m = module(core, function, Stage::StateMachines);

    let report = frame_prune::scan(&m);
    assert_eq!(report.stale, 1, "{report:?}");
    assert_eq!(report.kept_finalizers, 1, "{report:?}");
    assert_eq!(report.stale_refs, 0, "{report:?}");
    assert!(!report.has_candidates());
    if let Err(errors) = verify(&m) {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!("{}", lines.join("\n"));
    }
}

/// A stale base that a kept variable still reads out of is kept: dropping the base would leave the
/// kept view of it dangling.
#[test]
fn the_scan_keeps_a_stale_base_of_a_kept_view() {
    let mut core = CoreModule::new();
    let pair = core.tuple(vec![TypeId::INT, TypeId::STRING]);
    let mut b = FnBuilder::new(&core, "kept_view");
    b.set_ret(TypeId::INT);
    b.set_suspends();
    let t = b.param("t", pair);
    // `s` is a view of `t` and is read after the suspension, so it stays live and stays saved.
    let s = b.assign("s", TypeId::STRING, Value::TupleGet { base: t, index: 1 });
    let stmt = suspend(&mut b, vec![t, s]);
    b.stmt(stmt);
    let n = b.assign(
        "n",
        TypeId::INT,
        Value::Builtin {
            op: emela_core::Builtin::StringLength,
            args: vec![s.into()],
        },
    );
    b.term(Term::Return(Some(n.into())));
    let function = b.finish();
    let m = module(core, function, Stage::StateMachines);

    let report = frame_prune::scan(&m);
    assert_eq!(report.stale, 1, "{report:?}");
    assert_eq!(report.kept_views, 1, "{report:?}");
    assert_eq!(report.stale_refs, 0, "{report:?}");
    if let Err(errors) = verify(&m) {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!("{}", lines.join("\n"));
    }
}

/// `dce` is what makes a saved scalar stale, and the scan follows it. The dead pure `let` reads the
/// saved scalar; removing it takes the scalar's last use after the value away.
#[test]
fn dce_makes_a_saved_scalar_stale() {
    let core = CoreModule::new();
    let mut b = FnBuilder::new(&core, "dce_then_scan");
    b.set_ret(TypeId::INT);
    b.set_suspends();
    let v = b.assign("v", TypeId::INT, Value::Const(Const::Int(1)));
    let stmt = suspend(&mut b, vec![v]);
    b.stmt(stmt);
    // `w = v + 1` is dead, pure and scalar: `dce` removes it.
    let w = b.var("w", TypeId::INT);
    b.stmt(Stmt::Let {
        dst: Some(w),
        value: Value::Prim {
            op: emela_lir::PrimOp::Add,
            ty: emela_lir::PrimTy::I32,
            args: vec![v.into(), Const::Int(1).into()],
        },
        on_defect: None,
        on_suspend: None,
    });
    b.term(Term::Return(Some(Const::Int(0).into())));
    let function = b.finish();
    let mut m = module(core, function, Stage::RefCounted);

    // Before `dce`, `v` is live after the suspension and the slot is needed.
    let before = frame_prune::scan(&m);
    assert_eq!(before.stale, 0, "{before:?}");
    // After it, the slot is stale: the only thing between the suspension and the end that read `v`
    // is gone.
    let stats = opt::dce::run(&mut m);
    assert_eq!(stats.dce, 1, "{stats:?}");
    let after = frame_prune::scan(&m);
    assert_eq!(after.stale_scalars, 1, "{after:?}");
    assert!(after.has_candidates());
    if let Err(errors) = verify(&m) {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!("{}", lines.join("\n"));
    }
}
