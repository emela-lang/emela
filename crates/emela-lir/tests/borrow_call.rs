//! Tests for borrow inference (C2a/C2b, PR-026): the per-function borrowed-parameter signature
//! that reference counting carries through its ownership rules, and the flag that runs it in the
//! pipeline (on by default since M3).
//!
//! Programs are lowered (with the Core IR optimisations off), monomorphised, converted and taken to
//! `Stage::StateMachines`; a test then calls `insert_borrow_signature` explicitly, runs
//! `insert_rc` and checks the balance with the verifier. A test of the owned convention builds the
//! module with the flag off; the flag tests run it through `to_heap_with_options`.
//!
//! The signature is a calling-convention fact, not a use. The tests check the three places that
//! must agree: what the pass writes (`Function::borrowed`), what reference counting emits (no
//! `drop` in the callee of a borrowed parameter, no `dup` in the caller of a borrowed argument),
//! and the balance check of the verifier.

#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

use emela_core::{FnType, Roots, TypeId, monomorphize_from, verify_monomorphic};
use emela_lir::opt::analysis::borrow;
use emela_lir::opt::{LirRules, Options};
use emela_lir::{
    Callee, Const, FnBuilder, FnId, Module, Operand, Stage, Stmt, Term, Value, convert_closures,
    dump, explicit_defects, from_core, insert_borrow_signature, insert_rc, state_machines,
    to_heap_with, to_heap_with_options, verify,
};
use support::try_lower_with_opt;

/// Lowers (from `roots`), monomorphizes and converts to the Flow IR (before the Heap IR passes).
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

/// The module at `Stage::StateMachines`: after closure conversion, explicit defects and the state
/// machines, and before reference counting — the stage borrow inference runs at.
fn staged(src: &str) -> Module {
    let mut m = flow(&[("main.emel", src)], Roots::Exported);
    convert_closures(&mut m);
    explicit_defects(&mut m);
    state_machines(&mut m);
    assert_eq!(m.stage, Stage::StateMachines);
    m
}

fn check(m: &Module) {
    if let Err(errors) = verify(m) {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!(
            "the borrow signature left invalid LIR:\n{}",
            lines.join("\n")
        );
    }
}

fn function(m: &Module, name: &str) -> FnId {
    if let Some((id, _)) = m.functions.iter().find(|(_, f)| f.name == name) {
        return id;
    }
    m.functions
        .iter()
        .find(|(_, f)| f.name.starts_with(name))
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no function `{name}`"))
}

fn has_dup(m: &Module, f: FnId, v: emela_lir::VarId) -> bool {
    m.functions[f]
        .stmts
        .iter()
        .any(|(_, s)| matches!(s, Stmt::Dup(x) if *x == v))
}

fn has_drop(m: &Module, f: FnId, v: emela_lir::VarId) -> bool {
    m.functions[f]
        .stmts
        .iter()
        .any(|(_, s)| matches!(s, Stmt::Drop(x) if *x == v))
}

/// The variable passed at `index` to a direct call of `callee` in `caller`.
fn call_arg(m: &Module, caller: FnId, callee: FnId, index: usize) -> emela_lir::VarId {
    let f = &m.functions[caller];
    for (_, stmt) in f.stmts.iter() {
        let Stmt::Let {
            value:
                Value::Call {
                    callee: Callee::Direct(g),
                    args,
                    ..
                },
            ..
        } = stmt
        else {
            continue;
        };
        if *g == callee {
            let Operand::Var(v) = args[index] else {
                panic!("the argument is a constant");
            };
            return v;
        }
    }
    panic!(
        "no direct call of `{}` in `{}`",
        m.functions[callee].name, f.name
    );
}

/// A read-only parameter is borrowed: the callee does not drop it and the caller does not `dup`
/// the argument. The counted list view is read but not consumed, so the borrow removes counting
/// work (C2). The second run of the pass writes the same signature.
const FIRST: &str = "\
fn first(x: (Int, List[Int])) -> Int {
  (_, xs) = x
  xs match {
    [] -> -1
    [n, ..] -> n
  }
}

fn main() {
  t = (0, [1, 2, 3])
  n = first(t)
  m = first(t)
  dbg(n + m)
}
";

#[test]
fn a_read_only_parameter_is_borrowed() {
    let mut m = staged(FIRST);
    let first = function(&m, "first");
    let main = function(&m, "main");
    let param = m.functions[first].params[0];
    assert!(m.functions[first].borrowed.is_empty(), "no signature yet");

    insert_borrow_signature(&mut m);
    assert_eq!(m.functions[first].borrowed, vec![true]);
    let signature = m.functions[first].borrowed.clone();
    insert_borrow_signature(&mut m);
    assert_eq!(
        m.functions[first].borrowed, signature,
        "a second run writes the same signature"
    );

    let arg = call_arg(&m, main, first, 0);
    insert_rc(&mut m);
    check(&m);
    assert!(
        !has_drop(&m, first, param),
        "the callee must not drop a borrowed parameter"
    );
    assert!(
        !has_dup(&m, main, arg),
        "the caller must not dup a borrowed argument, even when it stays live"
    );
}

/// Without the signature the program keeps the owned convention: the callee drops the parameter
/// and the caller `dup`s the argument that is still live after the first call.
#[test]
fn without_the_signature_the_callee_owns_the_parameter() {
    let mut m = staged(FIRST);
    let first = function(&m, "first");
    let main = function(&m, "main");
    let param = m.functions[first].params[0];
    assert!(m.functions[first].borrowed.is_empty());

    let arg = call_arg(&m, main, first, 0);
    insert_rc(&mut m);
    check(&m);
    assert!(
        has_drop(&m, first, param),
        "the owned convention drops the parameter"
    );
    assert!(
        has_dup(&m, main, arg),
        "the owned convention dups an argument that stays live"
    );
}

/// A parameter that the callee returns leaves the callee: it is not borrowed.
#[test]
fn a_parameter_returned_is_owned() {
    let src = "\
fn id(x: (Int, Int)) -> (Int, Int) { x }

fn main() {
  t = (1, 2)
  u = id(t)
  v = id(t)
  dbg(u)
  dbg(v)
}
";
    let mut m = staged(src);
    let id = function(&m, "id");
    let main = function(&m, "main");
    insert_borrow_signature(&mut m);
    assert!(
        m.functions[id].borrowed.is_empty(),
        "a returned parameter is not borrowed"
    );
    let param = m.functions[id].params[0];
    let arg = call_arg(&m, main, id, 0);
    insert_rc(&mut m);
    check(&m);
    assert!(
        !has_dup(&m, id, param),
        "the callee returns the reference it was given instead of duplicating it"
    );
    assert!(
        !has_drop(&m, id, param),
        "the returned reference leaves with the return value"
    );
    assert!(
        has_dup(&m, main, arg),
        "the caller dups the owned argument that stays live"
    );
}

/// A parameter that the callee stores in a container or captures in a closure leaves the callee:
/// it is not borrowed.
#[test]
fn a_parameter_stored_or_captured_is_owned() {
    let src = "\
enum Box { Box((Int, Int)) }

fn box_it(x: (Int, Int)) -> Box { Box(x) }

fn capture(x: (Int, Int)) -> fn() -> Int {
  || {
    (a, _) = x
    a
  }
}

fn main() {
  b = box_it((1, 2))
  f = capture((3, 4))
  b match { Box(p) -> dbg(p) }
  dbg(f())
}
";
    let mut m = staged(src);
    let box_it = function(&m, "box_it");
    let capture = function(&m, "capture");
    let before: Vec<Vec<bool>> = [box_it, capture]
        .iter()
        .map(|&id| m.functions[id].borrowed.clone())
        .collect();
    assert!(before.iter().all(Vec::is_empty));

    insert_borrow_signature(&mut m);
    assert!(
        m.functions[box_it].borrowed.is_empty(),
        "a stored parameter is not borrowed"
    );
    assert!(
        m.functions[capture].borrowed.is_empty(),
        "a captured parameter is not borrowed"
    );
    insert_rc(&mut m);
    check(&m);
}

/// A function whose value is taken may be called indirectly, and the indirect caller passes owned
/// arguments because the signature is not visible at that call. Its signature is forced empty, even
/// when its body only reads the parameter.
#[test]
fn functions_used_as_values_keep_the_owned_convention() {
    let src = "\
fn first(x: (Int, Int)) -> Int {
  (a, _) = x
  a
}

fn apply(f: fn((Int, Int)) -> Int, p: (Int, Int)) -> Int { f(p) }

fn main() {
  g = first
  dbg(apply(g, (1, 2)))
}
";
    let mut m = staged(src);
    insert_borrow_signature(&mut m);
    let mut targets = Vec::new();
    for (_, f) in m.functions.iter() {
        for (_, s) in f.stmts.iter() {
            if let Stmt::Let {
                value: Value::MakeClosure { function, .. },
                ..
            } = s
            {
                targets.push(*function);
            }
        }
    }
    assert!(!targets.is_empty(), "the value-taken closure has a target");
    for &g in &targets {
        assert!(
            m.functions[g].borrowed.iter().all(|b| !b),
            "`{}` is used as a value and keeps the owned convention",
            m.functions[g].name
        );
    }
    insert_rc(&mut m);
    check(&m);
}

/// An exported function is called by the host with owned arguments, and a `FnRef` takes a
/// function's value. Both are forced to the owned convention; an ordinary read-only function is
/// still marked borrowed. The functions are built by hand so that the test names the exact shape.
#[test]
fn exported_functions_and_functions_taken_as_values_are_forced_owned() {
    let mut core = emela_core::Module::new();
    let ints = core.list(TypeId::INT);
    let pair = core.tuple(vec![TypeId::INT, ints]);
    let fn_ty = core.func(FnType {
        params: vec![pair],
        ret: TypeId::INT,
        fails: false,
        suspends: false,
        ev: false,
        sets: None,
    });
    let mut m = Module::new(core);

    let read_only = |m: &mut Module, name: &str, exported: bool| {
        let mut b = FnBuilder::new(&m.core, name);
        b.set_ret(TypeId::INT);
        if exported {
            b.set_exported();
        }
        let x = b.param("x", pair);
        let _n = b.assign("n", TypeId::INT, Value::TupleGet { base: x, index: 0 });
        let xs = b.assign("xs", ints, Value::TupleGet { base: x, index: 1 });
        let _empty = b.assign("empty", TypeId::BOOL, Value::ListIsEmpty(xs));
        b.term(Term::Return(Some(Const::Int(0).into())));
        m.add(b.finish())
    };
    let plain = read_only(&mut m, "plain", false);
    let exported = read_only(&mut m, "exported", true);
    let taken = read_only(&mut m, "taken", false);

    // `taker` takes the value of `taken` (`let g = taken`).
    let mut b = FnBuilder::new(&m.core, "taker");
    b.set_ret(TypeId::INT);
    let p = b.param("p", pair);
    let _g = b.assign("g", fn_ty, Value::FnRef(taken));
    b.term(Term::Return(Some(Const::Int(0).into())));
    let _ = p;
    m.add(b.finish());

    // The pass runs at `StateMachines`; the hand-built module has no suspensions or closures to
    // make explicit, so the stage is set directly.
    m.stage = Stage::StateMachines;
    insert_borrow_signature(&mut m);
    assert_eq!(
        m.functions[plain].borrowed,
        vec![true],
        "an ordinary read-only parameter is borrowed"
    );
    assert!(
        m.functions[exported].borrowed.is_empty(),
        "an exported function keeps the owned convention"
    );
    assert!(
        m.functions[taken].borrowed.is_empty(),
        "a function whose value is taken keeps the owned convention"
    );
}

const NAPS: &str = "\
effect Clock {
  suspend fn sleep(ms: Int) -> Int
}

handler InstantClock: Clock {
  suspend fn sleep(ms, resume) { resume(ms) }
}

fn nap(ms: Int, label: String) -> String uses { Clock } {
  n = (use Clock).sleep(ms)
  \"#{label} #{ms} #{n}\"
}

fn main() {
  with InstantClock { dbg(nap(1, \"a\")) }
}
";

/// A borrowed parameter that a suspension frame saves must be owned by the frame: reference
/// counting `dup`s it at the start of the function and drops it like any other owned variable.
///
/// B6 does not mark a parameter that a frame saves borrowed, and the integration keeps a parameter
/// owned when a caller owns the counted argument of a suspending call (here `main` passes the
/// evidence table to the suspending `nap`). So the test first checks that
/// `insert_borrow_signature` leaves the parameter owned, then installs the signature by hand to
/// exercise the counting rule that a more precise analysis would need.
#[test]
fn a_borrowed_parameter_that_a_frame_saves_is_owned_by_the_frame() {
    let mut m = staged(NAPS);
    let nap = function(&m, "nap");
    let label = m.functions[nap].params[1];
    assert!(
        m.functions[nap].borrowed.is_empty(),
        "no signature before the pass"
    );

    insert_borrow_signature(&mut m);
    assert_ne!(
        m.functions[nap].borrowed.get(1),
        Some(&true),
        "a frame-saved parameter stays owned"
    );

    // The signature a later, more precise analysis could write. (The third parameter is the
    // evidence table; the suspending call from `main` keeps it owned in this configuration.)
    let n = m.functions[nap].params.len();
    m.functions[nap].borrowed.resize(n, false);
    m.functions[nap].borrowed[1] = true;
    insert_rc(&mut m);
    check(&m);
    let body = m.functions[nap].body.expect("nap has a body");
    let first = m.functions[nap].seqs[body].stmts[0];
    assert!(
        matches!(m.functions[nap].stmts[first], Stmt::Dup(v) if v == label),
        "a frame-saved borrowed parameter is dup'd at the start"
    );
    assert!(
        has_drop(&m, nap, label),
        "the frame-owned parameter is dropped like any other owned variable"
    );
}

/// A counted argument that the caller owns keeps the callee's parameter owned when the call may
/// suspend: the caller's frame was built under the owned convention and does not save an argument
/// the call would leave owned. `main` owns the evidence table it passes to the suspending `nap`.
#[test]
fn a_suspending_call_keeps_an_owned_argument_owned() {
    let mut m = staged(NAPS);
    let nap = function(&m, "nap");
    insert_borrow_signature(&mut m);
    assert_ne!(
        m.functions[nap].borrowed.get(2),
        Some(&true),
        "a counted argument of a suspending call stays owned"
    );
    insert_rc(&mut m);
    check(&m);
}

/// `Array.set` writes the array in place when its actual count is one, so its array argument must
/// stay owned. B6 blocks it and the signature keeps the argument owned.
#[test]
fn array_set_keeps_its_array_argument_owned() {
    let src = "\
import Std.Array.{Array}

fn put(a: Array[Int], v: Int) -> Array[Int] {
  Array.set(a, 0, v)
}

fn main() {
  a = Array.from_list([1, 2])
  b = put(a, 3)
  c = put(b, 4)
  dbg(c)
}
";
    let mut m = staged(src);
    let put = function(&m, "put");
    let facts = borrow::analyze(&m)
        .known()
        .expect("a well-formed module has borrow facts");
    assert_eq!(
        facts.borrowed_params[&put],
        vec![false, false],
        "the array argument is written in place and the value is stored"
    );

    insert_borrow_signature(&mut m);
    assert!(
        m.functions[put].borrowed.is_empty(),
        "the array argument of Array.set is not borrowed"
    );
    insert_rc(&mut m);
    check(&m);
}

/// A list parameter that the function only reads. A list is a heap reference on both backends, so
/// the call-site `dup` that the flag removes is visible in the emitted code as well as here.
const LIST_FIRST: &str = "\
fn first(xs: List[Int]) -> Int {
  xs match {
    [] -> -1
    [x, ..] -> x
  }
}

fn main() {
  xs = [1, 2, 3]
  a = first(xs)
  b = first(xs)
  dbg(a + b)
}
";

/// The default LIR options with borrow inference on.
fn borrow_options() -> Options {
    Options {
        rules: LirRules {
            borrow: true,
            ..LirRules::default()
        },
    }
}

/// `to_heap_with` is `to_heap_with_options` with the default options: the same Heap IR and the
/// same counters, with borrow inference on (M3).
#[test]
fn the_default_options_match_to_heap_with() {
    let mut plain = flow(&[("main.emel", LIST_FIRST)], Roots::Exported);
    let mut options = flow(&[("main.emel", LIST_FIRST)], Roots::Exported);
    let plain_stats = to_heap_with(&mut plain, true);
    let option_stats = to_heap_with_options(&mut options, true, Options::default());
    assert_eq!(dump(&plain), dump(&options));
    assert_eq!(format!("{plain_stats:?}"), format!("{option_stats:?}"));
    assert!(option_stats.borrow_functions >= 1, "{option_stats:?}");
    assert!(option_stats.borrow_params >= 1, "{option_stats:?}");
    assert!(option_stats.borrow_args >= 1, "{option_stats:?}");
    check(&plain);
}

/// The flag runs the pass in the pipeline: the signature appears, the call site no longer `dup`s
/// the argument, the callee no longer drops the parameter, the counters report it, and the module
/// still passes the balance check.
#[test]
fn the_borrow_flag_turns_the_signature_on_in_the_pipeline() {
    let mut m = flow(&[("main.emel", LIST_FIRST)], Roots::Exported);
    let stats = to_heap_with_options(&mut m, true, borrow_options());
    let first = function(&m, "first");
    let main = function(&m, "main");
    let param = m.functions[first].params[0];
    let arg = call_arg(&m, main, first, 0);
    assert_eq!(m.functions[first].borrowed, vec![true]);
    check(&m);
    assert!(
        !has_drop(&m, first, param),
        "the callee does not drop a borrowed parameter"
    );
    assert!(
        !has_dup(&m, main, arg),
        "the caller does not dup a borrowed argument even though it stays live"
    );
    assert!(stats.borrow_functions >= 1, "{stats:?}");
    assert!(stats.borrow_params >= 1, "{stats:?}");
    assert!(stats.borrow_args >= 1, "{stats:?}");
}

/// The flag does not weaken the ownership rules: a parameter the callee returns or stores keeps
/// the owned convention, so the caller `dup`s the argument and the callee drops or moves it.
#[test]
fn the_borrow_flag_keeps_returned_and_stored_parameters_owned() {
    let src = "\
enum Box { Box((Int, Int)) }

fn id(x: (Int, Int)) -> (Int, Int) { x }

fn store(x: (Int, Int)) -> Box { Box(x) }

fn main() {
  t = (1, 2)
  u = id(t)
  v = id(t)
  b = store((3, 4))
  dbg(u)
  dbg(v)
  b match { Box(p) -> dbg(p) }
}
";
    let mut m = flow(&[("main.emel", src)], Roots::Exported);
    to_heap_with_options(&mut m, true, borrow_options());
    let id = function(&m, "id");
    let store = function(&m, "store");
    let main = function(&m, "main");
    let id_param = m.functions[id].params[0];
    let store_param = m.functions[store].params[0];
    let arg = call_arg(&m, main, id, 0);
    check(&m);
    assert!(
        m.functions[id].borrowed.is_empty(),
        "a returned parameter is owned"
    );
    assert!(
        m.functions[store].borrowed.is_empty(),
        "a stored parameter is owned"
    );
    assert!(
        has_dup(&m, main, arg),
        "the owned call dups the argument that stays live"
    );
    assert!(
        !has_dup(&m, id, id_param),
        "the callee moves the reference it returns"
    );
    assert!(
        !has_dup(&m, store, store_param) && !has_drop(&m, store, store_param),
        "the callee moves the reference it stores into the record"
    );
}

/// C2: a wrapper whose only use of a parameter is as an indirect callee removes no counting work.
/// The B6 facts may borrow `f` (the indirect callee is a read), but the narrowed signature keeps
/// it owned, so the caller passes the closure record and the callee drops it as before.
const APPLY: &str = "\
fn first(x: (Int, List[Int])) -> Int {
  (_, xs) = x
  xs match {
    [] -> -1
    [n, ..] -> n
  }
}

fn apply(f: fn((Int, List[Int])) -> Int, x: (Int, List[Int])) -> Int { f(x) }

fn main() {
  g = first
  dbg(apply(g, (0, [1, 2])))
}
";

#[test]
fn a_parameter_used_only_as_an_indirect_callee_is_owned() {
    let mut m = staged(APPLY);
    let apply = function(&m, "apply");
    let facts = borrow::analyze(&m)
        .known()
        .expect("a well-formed module has borrow facts");
    assert_eq!(
        facts.borrowed_params[&apply].first(),
        Some(&true),
        "the facts still say the indirect callee is a read"
    );

    insert_borrow_signature(&mut m);
    assert!(
        m.functions[apply].borrowed.is_empty(),
        "borrowing the callee value saves no reference operation inside `apply`"
    );
    insert_rc(&mut m);
    check(&m);
}

/// C2: a read-only recursive traversal keeps its borrow. The `rest` view of the parameter is
/// passed at `check`'s borrowed position, so it is not consumed; that view is the counting work
/// the borrow removes, and the fixpoint keeps it.
const CHECK: &str = "\
fn check(xs: List[Int]) -> Int {
  xs match {
    [] -> 0
    [x, ..rest] -> x + check(rest)
  }
}

fn main() {
  xs = [1, 2, 3]
  a = check(xs)
  b = check(xs)
  dbg(a + b)
}
";

#[test]
fn a_read_only_traversal_that_reads_a_view_keeps_its_borrow() {
    let mut m = staged(CHECK);
    let check_fn = function(&m, "check");
    insert_borrow_signature(&mut m);
    assert_eq!(
        m.functions[check_fn].borrowed,
        vec![true],
        "the unconsumed `rest` view is what the borrow saves"
    );
    insert_rc(&mut m);
    check(&m);
}

/// C2: `insert`-style tree descent reads both children and stores them into the new node, so every
/// view of the node parameter is consumed. Borrowing the node therefore moves a drop to the caller
/// without removing a `dup`/`drop` pair; the position is cleared and the owned convention is
/// balanced.
const INSERT: &str = "\
enum Tree {
  Leaf
  Node(left: Tree, value: Int, right: Tree)
}

fn insert(t: Tree, x: Int) -> Tree {
  t match {
    Leaf -> Node(left: Leaf, value: x, right: Leaf)
    Node(left:, value: here, right:) -> if x < here {
      Node(left: insert(left, x), value: here, right:)
    } else {
      Node(left:, value: here, right: insert(right, x))
    }
  }
}

fn depth(t: Tree) -> Int {
  t match {
    Leaf -> 0
    Node(left:, right:, ..) -> 1 + depth(left) + depth(right)
  }
}

fn main() {
  t = Node(left: Leaf, value: 2, right: Leaf)
  u = insert(t, 1)
  v = insert(u, 3)
  dbg(depth(v))
}
";

#[test]
fn a_tree_insert_that_stores_every_view_is_owned() {
    let mut m = staged(INSERT);
    let insert_fn = function(&m, "insert");
    insert_borrow_signature(&mut m);
    assert!(
        m.functions[insert_fn].borrowed.is_empty(),
        "every view of the node is stored, so the borrow removes no counting work"
    );
    insert_rc(&mut m);
    check(&m);
}
