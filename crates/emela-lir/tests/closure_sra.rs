//! Tests for closure scalar replacement (C5/PR-043, closure half): after closure devirtualisation,
//! a closure record that is only passed to its own calls disappears and its scalar captures become
//! parameters of a specialisation of the lifted function. A record that is used as a value, captures
//! a reference, or names a failing or suspending function stays where it is.
//!
//! Programs are lowered (with the Core IR optimisations off), monomorphised, converted and
//! closure-converted, so the pass sees the shapes lowering makes. Every result must pass the
//! verifier; the ownership test continues through the Heap IR and checks the reference balance.

#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

use emela_core::{FnType, Roots, TypeId, monomorphize_from, verify_monomorphic};
use emela_lir::opt::{self, LirRules, Options};
use emela_lir::{
    Callee, FnBuilder, FnId, Function, Module, PrimOp, PrimTy, Stage, Stmt, Term, Value,
    convert_closures, dump, dump_function, explicit_defects, from_core, insert_rc, state_machines,
    verify,
};
use support::try_lower_with_opt;

/// Lowers, monomorphizes and converts to the Flow IR, with the Core optimisations off.
fn flow(src: &str) -> Module {
    let m = try_lower_with_opt(
        &[("main.emel", src)],
        Roots::Exported,
        emela_core::core_sources(),
        emela_core::bundled_files(),
        false,
    )
    .unwrap_or_else(|e| panic!("{e:#?}"));
    let fn_roots: Vec<_> = m
        .functions
        .iter()
        .filter(|(_, f)| f.exported)
        .map(|(id, _)| id)
        .collect();
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

/// [`flow`] with the closures lifted (the stage of the closure-level optimisations).
fn mid(src: &str) -> Module {
    let mut lir = flow(src);
    convert_closures(&mut lir);
    lir
}

/// Closure SRA after E5: only those two flags on, so a test sees exactly their effect.
fn closure_sra(devirt: bool, sra_closures: bool) -> Options {
    Options {
        rules: LirRules {
            devirt,
            sra_closures,
            ..LirRules::none()
        },
    }
}

/// Runs the pass and checks that the module still verifies.
fn run(m: &mut Module) -> opt::Stats {
    let stats = opt::run_mid_with(m, closure_sra(true, true));
    check(m);
    stats
}

/// Runs the pass through the whole Heap IR and checks that it verifies, reference balance
/// included (the verifier checks it at `Stage::RefCounted`).
fn run_to_heap(m: &mut Module) -> opt::Stats {
    let stats = opt::run_mid_with(m, closure_sra(true, true));
    explicit_defects(m);
    state_machines(m);
    insert_rc(m);
    check(m);
    stats
}

fn check(m: &Module) {
    if let Err(errors) = verify(m) {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!("the pass left invalid LIR:\n{}", lines.join("\n"));
    }
}

fn function(m: &Module, name: &str) -> FnId {
    if let Some((id, _)) = m.functions.iter().find(|(_, f)| f.name == name) {
        return id;
    }
    m.functions
        .iter()
        .find(|(_, f)| f.name.starts_with(&format!("{name}$")))
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no function `{name}`"))
}

/// The lifted function of a closure made in `owner`, the first one lifting made there.
fn lifted(m: &Module, owner: &str) -> FnId {
    let prefix = format!("{owner}$closure");
    m.functions
        .iter()
        .find(|(_, f)| f.name.starts_with(&prefix))
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no lifted closure of `{owner}`"))
}

/// The specialisation of `target` this pass made, if any.
fn specialised(m: &Module, target: FnId) -> Option<FnId> {
    let prefix = format!("{}$sra", m.functions[target].name);
    m.functions
        .iter()
        .find(|(_, f)| f.name == prefix || f.name.starts_with(&format!("{prefix}$")))
        .map(|(id, _)| id)
}

fn text(m: &Module, id: FnId) -> String {
    dump_function(m, &m.functions[id])
}

fn direct_calls_to(f: &Function, callee: FnId) -> usize {
    f.stmts
        .iter()
        .filter(|(_, stmt)| {
            matches!(
                stmt,
                Stmt::Let {
                    value: Value::Call {
                        callee: Callee::Direct(g),
                        ..
                    },
                    ..
                } if *g == callee
            )
        })
        .count()
}

const CAPTURE: &str = "\
fn run(base: Int) -> Int {
  f = |x: Int| x + base
  f(1)
}

fn main() { dbg(run(2)) }
";

/// The record disappears, the call names a specialisation with the capture first, and that
/// specialisation has no environment and reads its first parameter instead of `env_get`.
#[test]
fn a_closure_called_in_its_function_loses_the_record() {
    let mut m = mid(CAPTURE);
    let owner = function(&m, "run");
    let target = lifted(&m, "run");
    assert!(
        text(&m, owner).contains("make_closure"),
        "{}",
        text(&m, owner)
    );

    let stats = run(&mut m);
    assert_eq!(stats.devirt, 1, "{}", text(&m, owner));
    assert_eq!(stats.sra_closures, 1);
    assert_eq!(stats.sra_closure_deopts, 0);

    let clone = specialised(&m, target).expect("the lifted function was not specialised");
    let body = text(&m, owner);
    assert!(!body.contains("make_closure"), "{body}");
    assert!(!body.contains("env_get"), "{body}");
    assert_eq!(direct_calls_to(&m.functions[owner], clone), 1);
    assert!(body.contains("(base%") || body.contains("(base"), "{body}");

    let specialised_body = text(&m, clone);
    assert!(specialised_body.contains("fn "), "{specialised_body}");
    assert!(!specialised_body.contains("env["), "{specialised_body}");
    assert!(!specialised_body.contains("env_get"), "{specialised_body}");
    assert_eq!(m.stage, Stage::ClosuresConverted);

    // The original function is untouched: another record of it may still need its environment.
    let original = text(&m, target);
    assert!(original.contains("env["), "{original}");
    assert!(original.contains("env_get"), "{original}");
}

/// A function value with no captures (a wrapper made from a function reference) loses its record
/// too: the specialisation takes only the function's own parameters.
#[test]
fn a_capture_free_closure_loses_its_record() {
    let mut m = mid("\
fn inc(x: Int) -> Int { x + 1 }

fn run(a: Int) -> Int {
  f = inc
  f(a)
}

fn main() { dbg(run(1)) }
");
    let owner = function(&m, "run");
    let stats = run(&mut m);
    assert_eq!(stats.sra_closures, 1, "{}", text(&m, owner));
    assert_eq!(stats.sra_closure_deopts, 0);
    let body = text(&m, owner);
    assert!(!body.contains("make_closure"), "{body}");
    let target = function(&m, "inc$fn");
    let clone = specialised(&m, target).expect("not specialised");
    assert!(!text(&m, clone).contains("env["), "{}", text(&m, clone));
    assert_eq!(direct_calls_to(&m.functions[owner], clone), 1);
}

/// Two call sites of the same record are both rewritten, and the capture is read at each of them.
#[test]
fn every_call_of_the_record_is_rewritten() {
    let mut m = mid("\
fn run(base: Int, c: Bool) -> Int {
  f = |x: Int| x + base
  if c { f(1) + f(2) } else { f(3) }
}

fn main() { dbg(run(2, True)) }
");
    let owner = function(&m, "run");
    let target = lifted(&m, "run");
    let stats = run(&mut m);
    assert_eq!(stats.sra_closures, 1);
    assert_eq!(stats.devirt, 3);
    let clone = specialised(&m, target).expect("not specialised");
    assert_eq!(direct_calls_to(&m.functions[owner], clone), 3);
    assert_eq!(
        text(&m, owner).matches("base%0,").count(),
        3,
        "{}",
        text(&m, owner)
    );
}

/// A record made in a loop may capture the loop variable. The pass reads it at the call, which is
/// in the same iteration: a loop only rebinds its variable at `recur`, after every statement of
/// the body, so the value is the one the record was made with.
#[test]
fn a_loop_variable_capture_is_read_in_the_same_iteration() {
    let mut m = mid("\
fn step(i: Int, limit: Int, total: Int) -> Int {
  if limit <= i {
    total
  } else {
    f = |x: Int| x + i
    step(i + 1, limit, total + f(1))
  }
}

fn main() { dbg(step(0, 5, 0)) }
");
    let owner = function(&m, "step");
    assert!(text(&m, owner).contains("loop("), "{}", text(&m, owner));
    let target = lifted(&m, "step");
    let stats = run(&mut m);
    assert_eq!(stats.sra_closures, 1);
    let clone = specialised(&m, target).expect("not specialised");
    assert_eq!(direct_calls_to(&m.functions[owner], clone), 1);
    let body = text(&m, owner);
    assert!(!body.contains("make_closure"), "{body}");
    // The call reads the loop variable, not a copy made by the record.
    let recur = body.find("recur(").expect("the loop recurs");
    let call = body
        .find(&format!("call {}(", m.functions[clone].name))
        .expect("the call is rewritten");
    assert!(call < recur, "{body}");
}

/// The flag off leaves the module alone; so does closure SRA without E5, because the call is still
/// indirect and the record is the callee.
#[test]
fn the_flag_is_off_by_default_and_needs_the_direct_call_of_e5() {
    let mut off = mid(CAPTURE);
    let before = dump(&off);
    let stats = opt::run_mid_with(&mut off, closure_sra(false, false));
    assert_eq!(stats.sra_closures, 0);
    assert_eq!(stats.sra_closure_deopts, 0);
    assert_eq!(dump(&off), before);

    let mut default = mid(CAPTURE);
    let stats = opt::run_mid(&mut default, true);
    assert_eq!(stats.sra_closures, 0);
    assert_eq!(dump(&default), before, "a default build changed");

    let mut no_devirt = mid(CAPTURE);
    let stats = opt::run_mid_with(&mut no_devirt, closure_sra(false, true));
    assert_eq!(stats.sra_closures, 0);
    assert_eq!(stats.sra_closure_deopts, 1);
    assert_eq!(dump(&no_devirt), before);
    check(&no_devirt);
}

/// A record that leaves the function, is passed on or is stored in a list stays: the pass only
/// replaces a record whose every use it can see.
#[test]
fn an_escaping_closure_keeps_its_record() {
    for src in [
        "\
fn make(base: Int) -> fn(Int) -> Int {
  |x: Int| x + base
}

fn main() {
  f = make(2)
  dbg(f(1))
}
",
        "\
fn apply(f: fn(Int) -> Int, x: Int) -> Int { f(x) }

fn run(base: Int) -> Int {
  f = |x: Int| x + base
  apply(f, 1)
}

fn main() { dbg(run(2)) }
",
        "\
fn store(base: Int) -> List[fn(Int) -> Int] {
  f = |x: Int| x + base
  [f]
}

fn main() { dbg(List.length(store(2))) }
",
    ] {
        let mut m = mid(src);
        let before = dump(&m);
        let stats = run(&mut m);
        assert_eq!(stats.sra_closures, 0, "{before}");
        assert_eq!(
            dump(&m),
            before,
            "the pass changed a module it had to leave alone"
        );
    }
}

/// A record with a second, non-call use is not replaced, even when one call would fit.
#[test]
fn a_record_used_as_a_value_keeps_its_record() {
    let mut m = mid("\
fn run(base: Int, c: Bool) -> Int {
  f = |x: Int| x + base
  if c { f(1) } else { f(2) }
}

fn main() { dbg(run(2, True)) }
");
    let before = dump(&m);
    let stats = run(&mut m);
    assert_eq!(stats.sra_closures, 1);
    assert_ne!(dump(&m), before);

    // The same shape with the record yielded as a value: not a call site.
    let mut m = mid("\
fn run(base: Int, c: Bool) -> Int {
  f = |x: Int| x + base
  g = if c { f } else { f }
  g(1) + f(2)
}

fn main() { dbg(run(2, True)) }
");
    let before = dump(&m);
    let stats = run(&mut m);
    assert_eq!(stats.sra_closures, 0, "{before}");
    assert_eq!(dump(&m), before);
}

/// A record made in a suspending function stays: a frame could save it, and after the rewrite it
/// would save the captures instead, so the pass does not touch a function that suspends.
#[test]
fn a_record_made_in_a_suspending_function_keeps_its_record() {
    let mut m = mid("\
effect Clock {
  suspend fn sleep(ms: Int) -> Int
}

handler InstantClock: Clock {
  suspend fn sleep(ms, resume) { resume(ms) }
}

fn run(base: Int) -> Int uses { Clock } {
  f = |x: Int| x + base
  (use Clock).sleep(1)
  f(2)
}

fn main() {
  with InstantClock { dbg(run(2)) }
}
");
    let target = lifted(&m, "run");
    assert!(m.functions[function(&m, "run")].suspends);
    let stats = run(&mut m);
    assert_eq!(stats.sra_closures, 0);
    assert!(stats.sra_closure_deopts > 0);
    assert!(specialised(&m, target).is_none());
    let body = text(&m, function(&m, "run"));
    assert!(body.contains("make_closure"), "{body}");
}

/// A captured reference would move its ownership; the first slice replaces scalars only.
#[test]
fn a_reference_capture_keeps_the_record() {
    let mut m = mid("\
fn run(s: String) -> Int {
  f = |x: Int| x + String.length(s)
  f(1)
}

fn main() { dbg(run(\"abc\")) }
");
    let target = lifted(&m, "run");
    let stats = run(&mut m);
    assert_eq!(stats.devirt, 1, "the call did not become direct");
    assert_eq!(stats.sra_closures, 0);
    assert_eq!(stats.sra_closure_deopts, 1);
    assert!(specialised(&m, target).is_none());
    // E5 moved the call; the record and its environment read are still there.
    let body = text(&m, function(&m, "run"));
    assert!(body.contains("make_closure"), "{body}");
    assert!(text(&m, target).contains("env_get"), "{}", text(&m, target));
}

/// With the record gone the programme still reaches the Heap IR with a balanced reference count:
/// no `make_closure` is left and no `drop` of the record was invented.
#[test]
fn the_rewritten_function_reaches_the_heap_ir_balanced() {
    let mut m = mid(CAPTURE);
    let stats = run_to_heap(&mut m);
    assert_eq!(stats.sra_closures, 1);
    assert_eq!(m.stage, Stage::RefCounted);
    let body = text(&m, function(&m, "run"));
    assert!(!body.contains("make_closure"), "{body}");
    assert!(!body.contains("dup "), "{body}");
}

/// A second run finds nothing: the records are gone and the clones are copies of already
/// rewritten bodies.
#[test]
fn the_pass_is_idempotent() {
    let mut m = mid(CAPTURE);
    let first = run(&mut m);
    assert_eq!(first.sra_closures, 1);
    let after = dump(&m);
    let second = run(&mut m);
    assert_eq!(second.sra_closures, 0);
    assert_eq!(second.devirt, 0);
    assert_eq!(second.sra_closure_deopts, 0);
    assert_eq!(dump(&m), after);
}

// Hand-built IR. Lowering cannot make a lifted function that suspends, fails or uses its
// environment beyond the capture reads without an evidence capture, so those deopts are built by
// hand; the same holds for one lifted function named by two records with different captures.

/// A module with a lifted `fn(Int) -> Int` and a caller that makes a record and calls it.
struct Fixture {
    m: Module,
    ty: TypeId,
    ty_fails: TypeId,
    ty_suspends: TypeId,
}

impl Fixture {
    fn new() -> Self {
        let mut core = emela_core::Module::new();
        let ty = core.func(FnType {
            params: vec![TypeId::INT],
            ret: TypeId::INT,
            fails: false,
            suspends: false,
            ev: false,
            sets: None,
        });
        let ty_fails = core.func(FnType {
            params: vec![TypeId::INT],
            ret: TypeId::INT,
            fails: true,
            suspends: false,
            ev: false,
            sets: None,
        });
        let ty_suspends = core.func(FnType {
            params: vec![TypeId::INT],
            ret: TypeId::INT,
            fails: false,
            suspends: true,
            ev: false,
            sets: None,
        });
        Fixture {
            m: Module::new(core),
            ty,
            ty_fails,
            ty_suspends,
        }
    }

    /// `g(env, x) = env_get env#0 + x`, with the given environment.
    fn target(&mut self, name: &str, env: Vec<TypeId>) -> FnId {
        let mut b = FnBuilder::new(&self.m.core, name);
        b.set_ret(TypeId::INT);
        let e = b.param("env", self.ty);
        let x = b.param("x", TypeId::INT);
        let read = b.assign("c", TypeId::INT, Value::EnvGet { env: e, index: 0 });
        let sum = b.assign(
            "sum",
            TypeId::INT,
            Value::Prim {
                op: PrimOp::Add,
                ty: PrimTy::I32,
                args: vec![read.into(), x.into()],
            },
        );
        b.term(Term::Return(Some(sum.into())));
        let mut f = b.finish();
        f.env = Some(env);
        self.m.add(f)
    }

    /// `run(a, x) = call g(make_closure g[a], x)`, with the record typed `record_ty`.
    fn caller(&mut self, target: FnId, captures: usize, direct: bool, record_ty: TypeId) -> FnId {
        let mut b = FnBuilder::new(&self.m.core, "run");
        b.set_ret(TypeId::INT);
        let mut args = Vec::new();
        for i in 0..captures {
            args.push(b.param(&format!("a{i}"), TypeId::INT));
        }
        let x = b.param("x", TypeId::INT);
        let record = b.assign(
            "r",
            record_ty,
            Value::MakeClosure {
                function: target,
                captures: args.clone(),
            },
        );
        let callee = if direct {
            Callee::Direct(target)
        } else {
            Callee::Indirect(record)
        };
        let call_args = if direct {
            vec![record.into(), x.into()]
        } else {
            vec![x.into()]
        };
        let call = b.assign(
            "call",
            TypeId::INT,
            Value::Call {
                callee,
                args: call_args,
                suspends: false,
                on_fail: None,
            },
        );
        b.term(Term::Return(Some(call.into())));
        self.m.add(b.finish())
    }
}

fn only_sra() -> Options {
    Options {
        rules: LirRules {
            sra_closures: true,
            ..LirRules::none()
        },
    }
}

fn stage(m: &mut Module) {
    m.stage = Stage::ClosuresConverted;
    check(m);
}

/// A suspending lifted function is left alone.
#[test]
fn a_suspending_target_keeps_the_record() {
    let mut fx = Fixture::new();
    let target = fx.target("g", vec![TypeId::INT]);
    fx.m.functions[target].suspends = true;
    let caller = fx.caller(target, 1, true, fx.ty_suspends);
    // The call is suspending too, as the verifier requires of a direct call.
    let call = fx.m.functions[caller]
        .stmts
        .iter()
        .find_map(|(id, s)| {
            matches!(
                s,
                Stmt::Let {
                    value: Value::Call { .. },
                    ..
                }
            )
            .then_some(id)
        })
        .unwrap();
    let Stmt::Let {
        value: Value::Call { suspends, .. },
        ..
    } = &mut fx.m.functions[caller].stmts[call]
    else {
        unreachable!()
    };
    *suspends = true;
    stage(&mut fx.m);

    let before = dump(&fx.m);
    let stats = opt::run_mid_with(&mut fx.m, only_sra());
    assert_eq!(stats.sra_closures, 0, "{before}");
    assert_eq!(stats.sra_closure_deopts, 1);
    assert_eq!(dump(&fx.m), before);
}

/// A fallible lifted function is left alone.
#[test]
fn a_failing_target_keeps_the_record() {
    let mut fx = Fixture::new();
    let target = fx.target("g", vec![TypeId::INT]);
    fx.m.functions[target].fails = true;
    let caller = fx.caller(target, 1, true, fx.ty_fails);
    fx.m.functions[caller].fails = true;
    let call = fx.m.functions[caller]
        .stmts
        .iter()
        .find_map(|(id, s)| {
            matches!(
                s,
                Stmt::Let {
                    value: Value::Call { .. },
                    ..
                }
            )
            .then_some(id)
        })
        .unwrap();
    let Stmt::Let {
        value: Value::Call { on_fail, .. },
        ..
    } = &mut fx.m.functions[caller].stmts[call]
    else {
        unreachable!()
    };
    *on_fail = Some(emela_lir::OnFail::Propagate);
    stage(&mut fx.m);

    let before = dump(&fx.m);
    let stats = opt::run_mid_with(&mut fx.m, only_sra());
    assert_eq!(stats.sra_closures, 0, "{before}");
    assert_eq!(stats.sra_closure_deopts, 1);
    assert_eq!(dump(&fx.m), before);
}

/// One lifted function made into two records with different captures: each record gets its own
/// specialisation and its call passes its own capture.
#[test]
fn two_records_of_one_function_get_their_own_specialisations() {
    let mut fx = Fixture::new();
    let target = fx.target("g", vec![TypeId::INT]);
    // `run(a, b, x) = g(a)(x) + g(b)(x)`.
    let mut b = FnBuilder::new(&fx.m.core, "run");
    b.set_ret(TypeId::INT);
    let a = b.param("a", TypeId::INT);
    let c = b.param("b", TypeId::INT);
    let x = b.param("x", TypeId::INT);
    let r1 = b.assign(
        "r1",
        fx.ty,
        Value::MakeClosure {
            function: target,
            captures: vec![a],
        },
    );
    let r2 = b.assign(
        "r2",
        fx.ty,
        Value::MakeClosure {
            function: target,
            captures: vec![c],
        },
    );
    let n1 = b.assign(
        "n1",
        TypeId::INT,
        Value::Call {
            callee: Callee::Direct(target),
            args: vec![r1.into(), x.into()],
            suspends: false,
            on_fail: None,
        },
    );
    let n2 = b.assign(
        "n2",
        TypeId::INT,
        Value::Call {
            callee: Callee::Direct(target),
            args: vec![r2.into(), x.into()],
            suspends: false,
            on_fail: None,
        },
    );
    let sum = b.assign(
        "sum",
        TypeId::INT,
        Value::Prim {
            op: PrimOp::Add,
            ty: PrimTy::I32,
            args: vec![n1.into(), n2.into()],
        },
    );
    b.term(Term::Return(Some(sum.into())));
    let caller = fx.m.add(b.finish());
    stage(&mut fx.m);

    let stats = opt::run_mid_with(&mut fx.m, only_sra());
    assert_eq!(stats.sra_closures, 2);
    check(&fx.m);
    let body = text(&fx.m, caller);
    let names: Vec<String> =
        fx.m.functions
            .iter()
            .filter(|(_, f)| f.name.starts_with("g$sra"))
            .map(|(_, f)| f.name.clone())
            .collect();
    assert_eq!(names.len(), 2, "{body}");
    assert!(body.contains("(a%"), "{body}");
    assert!(body.contains("(b%") || body.contains("(c%"), "{body}");
    assert!(!body.contains("make_closure"), "{body}");
}

/// The environment of the lifted function is not only read through the capture reads: the record
/// cannot be taken apart.
#[test]
fn an_environment_used_beyond_its_reads_keeps_the_record() {
    let mut fx = Fixture::new();
    // `g(env, x) = { env_get env#0; box env; env_get env#0 + x }`.
    let mut b = FnBuilder::new(&fx.m.core, "g");
    b.set_ret(TypeId::INT);
    let env = b.param("env", fx.ty);
    let x = b.param("x", TypeId::INT);
    let read = b.assign("c", TypeId::INT, Value::EnvGet { env, index: 0 });
    let _boxed = b.assign("boxed", TypeId::ANY, Value::Coerce(env));
    let sum = b.assign(
        "sum",
        TypeId::INT,
        Value::Prim {
            op: PrimOp::Add,
            ty: PrimTy::I32,
            args: vec![read.into(), x.into()],
        },
    );
    b.term(Term::Return(Some(sum.into())));
    let mut g = b.finish();
    g.env = Some(vec![TypeId::INT]);
    let target = fx.m.add(g);
    let _caller = fx.caller(target, 1, true, fx.ty);
    stage(&mut fx.m);

    let before = dump(&fx.m);
    let stats = opt::run_mid_with(&mut fx.m, only_sra());
    assert_eq!(stats.sra_closures, 0, "{before}");
    assert_eq!(stats.sra_closure_deopts, 1);
    assert_eq!(dump(&fx.m), before);
}
