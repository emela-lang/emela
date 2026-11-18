//! Tests for closure devirtualisation (E5/PR-037): a call through a closure record that is
//! created and only called in the same function becomes a direct call of the lifted function.
//!
//! Programs are lowered (with the Core IR optimisations off), monomorphised, converted and
//! closure-converted, so the pass sees the shapes lowering makes. Every result must pass the
//! verifier; the ownership tests continue through the Heap IR and check the reference balance.

#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

use emela_core::{FnType, Roots, TypeId, monomorphize_from, verify_monomorphic};
use emela_lir::opt::analysis::Fact;
use emela_lir::opt::{self, LirRules, Options};
use emela_lir::{
    Callee, Const, FnBuilder, FnId, Function, Module, Operand, Stage, Stmt, Term, Value,
    convert_closures, dump, dump_function, explicit_defects, from_core, insert_rc, state_machines,
    to_heap_with, verify,
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

/// [`flow`] with the closures lifted (the stage the closure-level optimisations run at).
fn mid(src: &str) -> Module {
    let mut lir = flow(src);
    convert_closures(&mut lir);
    lir
}

/// Only closure devirtualisation on, so a test sees exactly its effect.
fn only_devirt(devirt: bool) -> Options {
    Options {
        rules: LirRules {
            devirt,
            ..LirRules::none()
        },
    }
}

/// Runs the pass and checks that the module still verifies.
fn run(m: &mut Module) -> opt::Stats {
    let stats = opt::run_mid_with(m, only_devirt(true));
    check(m);
    stats
}

/// Runs the pass through the whole Heap IR and checks that it verifies, reference balance
/// included (the verifier checks it at `Stage::RefCounted`).
fn run_to_heap(m: &mut Module) -> opt::Stats {
    let stats = opt::run_mid_with(m, only_devirt(true));
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

fn text(m: &Module, name: &str) -> String {
    dump_function(m, &m.functions[function(m, name)])
}

/// The number of indirect calls of a function.
fn indirect(f: &Function) -> usize {
    f.stmts
        .iter()
        .filter(|(_, stmt)| {
            matches!(
                stmt,
                Stmt::Let {
                    value: Value::Call {
                        callee: Callee::Indirect(_),
                        ..
                    },
                    ..
                }
            )
        })
        .count()
}

/// The number of indirect calls of the whole module.
fn indirect_calls(m: &Module) -> usize {
    m.functions.iter().map(|(_, f)| indirect(f)).sum()
}

/// The direct calls of the lifted function `callee` in `f`.
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

#[test]
fn a_closure_called_once_becomes_a_direct_call() {
    let mut m = mid(CAPTURE);
    let owner = function(&m, "run");
    let callee = lifted(&m, "run");
    assert_eq!(indirect(&m.functions[owner]), 1, "{}", text(&m, "run"));

    let stats = run(&mut m);
    assert_eq!(stats.devirt, 1);
    assert_eq!(stats.devirt_deopts, 0);
    assert_eq!(indirect(&m.functions[owner]), 0);
    assert_eq!(direct_calls_to(&m.functions[owner], callee), 1);
    assert_eq!(m.stage, Stage::ClosuresConverted);
    let body = text(&m, "run");
    assert!(!body.contains("call_indirect"), "{body}");
    assert!(
        body.contains(&format!("call {}", m.functions[callee].name)),
        "{body}"
    );
    // The record is kept: it is the environment argument, and nothing else moved.
    assert!(body.contains("make_closure"), "{body}");
}

#[test]
fn a_closure_called_twice_makes_both_calls_direct() {
    let mut m = mid("\
fn run(base: Int) -> Int {
  f = |x: Int| x + base
  f(1) + f(2)
}

fn main() { dbg(run(2)) }
");
    let owner = function(&m, "run");
    let callee = lifted(&m, "run");
    assert_eq!(indirect(&m.functions[owner]), 2, "{}", text(&m, "run"));

    let stats = run(&mut m);
    assert_eq!(stats.devirt, 2);
    assert_eq!(stats.devirt_deopts, 0);
    assert_eq!(indirect(&m.functions[owner]), 0);
    assert_eq!(direct_calls_to(&m.functions[owner], callee), 2);
    // `+` keeps its operands: the second call is still after the first.
    let body = text(&m, "run");
    let first = body
        .find(&format!("call {}", m.functions[callee].name))
        .unwrap();
    let second = body[first + 1..]
        .find(&format!("call {}", m.functions[callee].name))
        .map(|i| i + first + 1)
        .expect("two direct calls");
    assert!(first < second, "{body}");
}

/// A record that leaves the function (`apply` keeps it, `make` returns it, `store` puts it in a
/// list) is not devirtualised: the pass only rewrites a record whose every use it can see.
#[test]
fn an_escaping_closure_is_not_devirtualised() {
    for src in [
        // Passed to a function.
        "\
fn apply(f: fn(Int) -> Int, x: Int) -> Int { f(x) }

fn run(base: Int) -> Int {
  f = |x: Int| x + base
  apply(f, 1)
}

fn main() { dbg(run(2)) }
",
        // Returned from the function that made it.
        "\
fn make(base: Int) -> fn(Int) -> Int {
  |x: Int| x + base
}

fn main() {
  f = make(2)
  dbg(f(1))
}
",
        // Stored in a list.
        "\
fn store(base: Int) -> List[fn(Int) -> Int] {
  f = |x: Int| x + base
  [f]
}

fn main() { dbg(store(2)) }
",
    ] {
        let mut m = mid(src);
        let before = dump(&m);
        let stats = run(&mut m);
        assert_eq!(stats.devirt, 0, "{before}");
        assert_eq!(
            dump(&m),
            before,
            "the pass changed a module it had to leave alone"
        );
    }
}

/// The call sites may sit in a branch: the arguments are still evaluated before the call, in
/// order, and the record is still made where it was.
#[test]
fn a_call_inside_a_branch_keeps_its_order() {
    let mut m = mid("\
fn noisy(x: Int) -> Int {
  dbg(x)
  x
}

fn run(base: Int, c: Bool) -> Int {
  f = |x: Int| x + base
  if c { f(noisy(1)) } else { f(noisy(2)) }
}

fn main() { dbg(run(2, True)) }
");
    let owner = function(&m, "run");
    let callee = lifted(&m, "run");
    assert_eq!(indirect(&m.functions[owner]), 2, "{}", text(&m, "run"));

    let stats = run(&mut m);
    assert_eq!(stats.devirt, 2);
    assert_eq!(indirect(&m.functions[owner]), 0);
    let body = text(&m, "run");
    assert!(
        body.contains(&format!("call {}", m.functions[callee].name)),
        "{body}"
    );
    // The argument is a call of `noisy`, and its statement still comes first in each arm.
    let make = body.find("make_closure").expect("{body}");
    let noisy = body.find("call noisy(1)").expect("{body}");
    let call = body
        .find(&format!("call {}", m.functions[callee].name))
        .unwrap();
    assert!(make < noisy && noisy < call, "{body}");
    // Both arms are still there.
    assert_eq!(
        body.matches(&format!("call {}", m.functions[callee].name))
            .count(),
        2
    );
}

/// A call site inside a loop (a self tail call lowered to a loop) is rewritten, and the record is
/// made where the loop makes it.
#[test]
fn a_call_inside_a_loop_is_rewritten() {
    let mut m = mid("\
fn count(limit: Int, n: Int, acc: Int) -> Int {
  f = |x: Int| x + limit
  if n == 0 { acc } else { count(limit, n - 1, acc + f(n)) }
}

fn main() { dbg(count(2, 3, 0)) }
");
    let owner = function(&m, "count");
    let callee = lifted(&m, "count");
    assert_eq!(indirect(&m.functions[owner]), 1, "{}", text(&m, "count"));
    assert!(
        text(&m, "count").contains("loop("),
        "the fixture is not a loop"
    );

    let stats = run(&mut m);
    assert_eq!(stats.devirt, 1);
    assert_eq!(indirect(&m.functions[owner]), 0);
    let body = text(&m, "count");
    assert!(
        body.contains(&format!("call {}", m.functions[callee].name)),
        "{body}"
    );
    let make = body.find("make_closure").unwrap();
    let call = body
        .find(&format!("call {}", m.functions[callee].name))
        .unwrap();
    assert!(make < call, "{body}");
}

/// The record keeps its captures and its ownership: it is still made, still owns the capture, is
/// still borrowed by the call and is still released after it, and the lifted function still reads
/// the capture out of the environment. The reference balance holds.
#[test]
fn the_record_and_its_captures_are_kept() {
    let mut m = mid(CAPTURE);
    let callee = lifted(&m, "run");
    let stats = run_to_heap(&mut m);
    assert_eq!(stats.devirt, 1);
    assert_eq!(m.stage, Stage::RefCounted);

    let body = text(&m, "run");
    assert!(
        body.contains(&format!("make_closure {}", m.functions[callee].name)),
        "{body}"
    );
    assert!(
        body.contains(&format!("call {}(", m.functions[callee].name)),
        "{body}"
    );
    // The record is not consumed by the call: it is still released, after the call.
    let call = body
        .find(&format!("call {}(", m.functions[callee].name))
        .unwrap();
    let drop = body[call..]
        .find("drop f%")
        .map(|i| i + call)
        .expect("the record is released");
    assert!(call < drop, "{body}");
    // The lifted function reads the capture from the environment it was given.
    let lifted_body = text(&m, &m.functions[callee].name);
    assert!(lifted_body.contains("env_get env%"), "{lifted_body}");
}

/// A failing call keeps its failure edge; a suspending call keeps its `!` and the variables its
/// suspension edge saves. Both still verify at the Heap IR.
#[test]
fn flags_and_frames_are_preserved() {
    let mut m = mid("\
error Bad(reason: String)

fn run(base: Int) -> Int fails Bad {
  f = |x: Int| if x > 0 { x + base } else { fail Bad(reason: \"no\") }
  f(1)
}

fn main() { dbg(run(2) escape { Bad(reason:) -> 0 }) }
");
    let owner = function(&m, "run");
    let callee = lifted(&m, "run");
    let stats = run(&mut m);
    assert_eq!(stats.devirt, 1);
    assert_eq!(indirect(&m.functions[owner]), 0);
    let body = text(&m, "run");
    let call = body
        .lines()
        .find(|l| l.contains(&format!("call {}(", m.functions[callee].name)))
        .expect("{body}");
    assert!(call.contains("on_fail"), "the failure edge is gone: {call}");

    // The same through the Heap IR: the landing and the defect cleanup keep the record.
    let mut m = mid("\
effect Clock {
  suspend fn sleep(ms: Int) -> Int
}

handler InstantClock: Clock {
  suspend fn sleep(ms, resume) { resume(ms) }
}

fn run(base: Int) -> Int uses { Clock } {
  f = |x: Int| (use Clock).sleep(x) + base
  f(1)
}

fn main() {
  with InstantClock { dbg(run(2)) }
}
");
    let owner = function(&m, "run");
    let callee = lifted(&m, "run");
    let stats = run_to_heap(&mut m);
    assert_eq!(stats.devirt, 1);
    assert_eq!(indirect(&m.functions[owner]), 0);
    let body = text(&m, "run");
    let call = body
        .lines()
        .find(|l| l.contains(&format!("call {}(", m.functions[callee].name)))
        .expect("{body}");
    assert!(call.contains('!'), "the call no longer suspends: {call}");
    assert!(
        call.contains("on_suspend"),
        "the suspension edge is gone: {call}"
    );
}

/// A devirtualised suspending call saves exactly the variables the indirect call saved: the record
/// is still borrowed, so the state machine's frames do not change.
#[test]
fn a_suspending_call_keeps_the_same_frame() {
    const SOURCE: &str = "\
effect Clock {
  suspend fn sleep(ms: Int) -> Int
}

handler InstantClock: Clock {
  suspend fn sleep(ms, resume) { resume(ms) }
}

fn run(base: Int) -> Int uses { Clock } {
  f = |x: Int| (use Clock).sleep(x) + base
  f(1)
}

fn main() {
  with InstantClock { dbg(run(2)) }
}
";
    fn frames(m: &Module) -> Vec<Vec<String>> {
        let mut out = Vec::new();
        for (_, f) in m.functions.iter() {
            for (_, stmt) in f.stmts.iter() {
                if let Stmt::Let {
                    on_suspend: Some(suspend),
                    ..
                } = stmt
                {
                    out.push(
                        suspend
                            .saved
                            .iter()
                            .map(|&v| format!("{}%{}", f.vars[v].hint, u32::from(v.into_raw())))
                            .collect(),
                    );
                }
            }
        }
        out
    }

    let mut on = mid(SOURCE);
    let stats = run_to_heap(&mut on);
    assert_eq!(stats.devirt, 1);
    assert!(
        !frames(&on).is_empty(),
        "the fixture has no suspension point"
    );

    let mut off = mid(SOURCE);
    assert_eq!(opt::run_mid_with(&mut off, only_devirt(false)).devirt, 0);
    explicit_defects(&mut off);
    state_machines(&mut off);
    insert_rc(&mut off);
    check(&off);

    assert_eq!(frames(&on), frames(&off), "the frames changed");
}

/// With the flag off the pass does not run and the dump is unchanged; running it twice changes
/// nothing the second time.
#[test]
fn the_flag_is_off_by_default_and_the_pass_is_idempotent() {
    let mut m = mid(CAPTURE);
    let before = dump(&m);

    let stats = opt::run_mid_with(&mut m, only_devirt(false));
    assert_eq!(stats.devirt, 0);
    assert_eq!(dump(&m), before);

    let mut m = mid(CAPTURE);
    let stats = opt::run_mid(&mut m, true);
    assert_eq!(stats.devirt, 0, "default rules run the pass");
    assert_eq!(dump(&m), before);

    let first = run(&mut m);
    assert_eq!(first.devirt, 1);
    let after = dump(&m);
    let second = run(&mut m);
    assert_eq!(second.devirt, 0);
    // The record is an argument now, not a callee, so it is not even a candidate.
    assert_eq!(second.devirt_deopts, 0);
    assert_eq!(dump(&m), after);
}

/// `Unknown` escape is not "nothing escapes": the pass leaves the function alone and counts the
/// deopt.
#[test]
fn an_unknown_escape_set_deopts() {
    let mut m = mid(CAPTURE);
    let before = dump(&m);
    let stats = opt::devirt::run_with(&mut m, Fact::Unknown);
    assert_eq!(stats.devirt, 0);
    assert_eq!(stats.devirt_deopts, 1);
    assert_eq!(dump(&m), before);
    check(&m);
}

/// The default rules (devirtualisation off) leave the call indirect through the whole pipeline.
#[test]
fn the_default_rules_keep_the_indirect_call() {
    let mut m = flow(CAPTURE);
    let stats = to_heap_with(&mut m, true);
    assert_eq!(stats.devirt, 0);
    assert_eq!(indirect_calls(&m), 1);
    check(&m);
}

/// A hand-built lifted function that cannot fail, called through a record whose type may fail: the
/// call site does not fit the target (a direct call cannot have a failure edge the target does not
/// have), so the pass leaves it alone. The record is used only as the callee, so the use walk
/// accepts it: the deopt comes from the signature check.
#[test]
fn a_call_site_that_does_not_fit_the_target_deopts() {
    let mut core = emela_core::Module::new();
    let wide = core.func(FnType {
        params: vec![TypeId::INT],
        ret: TypeId::INT,
        fails: true,
        suspends: false,
        ev: false,
        sets: None,
    });
    let mut m = Module::new(core);

    // `g(env, x) -> Int`, an infallible lifted function with no captures.
    let mut b = FnBuilder::new(&m.core, "g");
    b.set_ret(TypeId::INT);
    b.param("env", wide);
    let _ = b.param("x", TypeId::INT);
    b.term(Term::Return(Some(Operand::Const(Const::Int(0)))));
    let mut g = b.finish();
    g.env = Some(Vec::new());
    let g = m.add(g);

    // `caller() -> Int fails`: `c = make_closure g[]; call_indirect c(1) on_fail fail`.
    let mut b = FnBuilder::new(&m.core, "caller");
    b.set_ret(TypeId::INT);
    b.set_fails();
    let c = b.assign(
        "c",
        wide,
        Value::MakeClosure {
            function: g,
            captures: Vec::new(),
        },
    );
    let t = b.assign(
        "t",
        TypeId::INT,
        Value::Call {
            callee: Callee::Indirect(c),
            args: vec![Operand::Const(Const::Int(1))],
            suspends: false,
            on_fail: Some(emela_lir::OnFail::Propagate),
        },
    );
    b.term(Term::Return(Some(t.into())));
    m.add(b.finish());
    m.stage = Stage::ClosuresConverted;
    check(&m);

    let before = dump(&m);
    let stats = run(&mut m);
    assert_eq!(stats.devirt, 0, "{before}");
    assert_eq!(stats.devirt_deopts, 1, "{before}");
    assert_eq!(dump(&m), before);
}

/// The pass is not a silent no-op on the corpus: the example projects contain closure records
/// that are only called, and the pass makes those calls direct. Every result still verifies.
#[test]
fn the_examples_have_calls_to_devirtualise() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut projects: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path().join("src"))
        .filter(|p| p.join("main.emel").exists())
        .collect();
    projects.sort();
    assert!(!projects.is_empty());

    let mut total = 0;
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
        // The entry first.
        files.sort_by_key(|(name, _)| (name != "main.emel", name.clone()));
        let files: Vec<(&str, &str)> = files
            .iter()
            .map(|(n, s)| (n.as_str(), s.as_str()))
            .collect();
        let m = try_lower_with_opt(
            &files,
            Roots::All,
            emela_core::core_sources(),
            emela_core::bundled_files(),
            false,
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));
        let roots: Vec<_> = m.functions.iter().map(|(id, _)| id).collect();
        let (mono, _) = monomorphize_from(&m, &roots);
        if verify_monomorphic(&mono).is_err() {
            continue;
        }
        let mut lir = from_core(mono).unwrap_or_else(|errors| {
            let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
            panic!("conversion errors:\n{}", lines.join("\n"));
        });
        convert_closures(&mut lir);
        let stats = opt::run_mid_with(&mut lir, only_devirt(true));
        check(&lir);
        total += stats.devirt;
    }
    assert!(total > 0, "no call of the examples was devirtualised");
}
