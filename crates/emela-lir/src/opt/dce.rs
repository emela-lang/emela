//! Dead code elimination: remove a `let` whose value is pure and whose destination is never read.
//!
//! Liveness is the only analysis this pass needs. At the Heap IR a variable is defined once, so
//! the question "is the destination live after its `let`?" is the question "is the variable read
//! anywhere on a way on from the statement?": its failure and defect landings, the continuations
//! after a suspension, the arms of a compound statement, the loop back edge and the terminators
//! all included. The pass asks [`liveness`] for that (its contract excludes the statement's own
//! destination, so the pass also counts the uses of the destination over the function; the two
//! agree because there is exactly one definition).
//!
//! Removing a read is only allowed when the statement can also be thrown away. A call, a builtin,
//! a runtime or concurrency operation, an allocation and a `dup`/`drop` may run effects, raise a
//! defect, suspend, allocate, or release a value with a finalizer, so the pass removes only
//! constants and primitive arithmetic. Deleting a defect or an effect is forbidden (parent plan
//! §3.3, audit C0/C7).
//!
//! Suspension frame pruning is **not** part of this pass. It would rewrite
//! [`Suspend::saved`](crate::ir::Suspend::saved), which fixes which variables a frame owns and
//! whether a lifted function's environment is borrowed at each view; [`frame_prune`] documents the
//! ownership proof it needs and the reason it has no candidate on the corpus (no pass of the Heap
//! IR can make a saved reference dead, and the only pass that can make a saved scalar dead, this
//! one, removes nothing on the corpus).
//!
//! [`frame_prune`]: crate::opt::frame_prune
//!
//! # Contract
//!
//! ```text
//! Rule ID: C7 LIR DCE (dead pure `let`)
//! Theory: liveness-based dead code elimination
//! Equation: `let d = v in e -> e` when `d` is not read after the statement and `v` is pure
//! Preconditions: the module is at Stage::RefCounted (B4 has an answer); the statement is a
//!                `let` with no defect landing and no suspension edge; its value is a constant or
//!                a primitive operation other than integer division or remainder; a destination
//!                has a scalar representation (not Repr::Ref) and no use on any way on, or there
//!                is no destination at all
//! Postconditions: the module is still at Stage::RefCounted and passes the verifier; every
//!                 remaining `dup`/`drop` still balances, and the removed statement's variables
//!                 and sequences are renumbered by `heap::compact`
//! Preserves: evaluation order, effects, the order and count of effects, fail and defect edges,
//!            suspension points and the frames they save, the releases of `with`, drop points and
//!            their relative order, the evidence tables, stdout/stderr/exit code and test results.
//!            A removed value allocates nothing, owns no reference and may not defect, so no
//!            release, finalizer or effect disappears, and no reference count changes.
//! Deopt: `Unknown` (or no answer for a statement) from liveness; any value the pass cannot
//!        classify as pure (a call, builtin, runtime or fiber operation, allocation, view,
//!        coercion); a `Repr::Ref` destination, even when its variable is dead; a statement with
//!        a defect landing or a suspension edge; and any statement the pass does not model
//!        (`Dup`, `Drop`, `Protect` and compound statements are never removed). Nothing is
//!        guessed: an unclassified value is left alone.
//! Budget: none. Each removal strictly decreases the number of statements, and the pass reaches
//!         its fixed point inside one run, so a second run removes nothing.
//! Measures: [`Stats::dce`] (removed statements) and [`Stats::dce_deopts`] (functions left alone
//!           because liveness answered `Unknown`).
//! ```

use std::collections::VecDeque;

use rustc_hash::{FxHashMap, FxHashSet};

use super::Stats;
use crate::heap::compact;
use crate::ir::{FnId, Function, Module, Operand, PrimOp, Stmt, StmtId, Term, Value, VarId};
use crate::opt::analysis::Fact;
use crate::opt::analysis::liveness::{self, LiveSets};
use crate::repr::Repr;

/// Removes the dead pure `let`s of every function whose liveness the analysis knows.
///
/// Preconditions: the module is at [`Stage::RefCounted`](crate::ir::Stage::RefCounted) in the
/// pipeline ([`super::run_heap_with`] calls it there). The pass itself accepts any stage and
/// deopts on every function when liveness answers `Unknown`, which is what the stages before
/// `DefectsExplicit` do; tests use that to check the deopt.
///
/// The functions are visited in arena order and the statements in sequence order, so the result
/// does not depend on the iteration order of a hash map. Removing a statement can make the
/// definitions it read dead in turn (`a = 1; b = a + 1; c = b + 1`), so the pass works the
/// definitions in a queue until nothing changes; one run reaches the fixed point. A changed
/// function is rebuilt with `heap::compact`, which drops the removed statements and numbers what
/// is left in the order met.
pub fn run(m: &mut Module) -> Stats {
    let mut stats = Stats::default();
    let sets = match liveness::analyze(m) {
        Fact::Known(sets) => sets,
        // `Unknown` is "no answer", not "not live". Do not touch a single function.
        Fact::Unknown => {
            stats.dce_deopts = m.functions.iter().filter(|(_, f)| f.body.is_some()).count() as u32;
            return stats;
        }
    };
    let functions: Vec<FnId> = m.functions.iter().map(|(id, _)| id).collect();
    for id in functions {
        if m.functions[id].body.is_none() {
            continue;
        }
        let f = m.functions[id].clone();
        let removed = plan(m, &f, id, &sets);
        if removed.is_empty() {
            continue;
        }
        let mut f = f;
        for (_, seq) in f.seqs.iter_mut() {
            seq.stmts.retain(|s| !removed.contains(s));
        }
        m.functions[id] = compact(&f);
        stats.dce += removed.len() as u32;
    }
    stats
}

/// The statements of `f` the pass removes, at the fixed point.
fn plan(m: &Module, f: &Function, id: FnId, sets: &LiveSets) -> FxHashSet<StmtId> {
    let mut uses: FxHashMap<VarId, u32> = FxHashMap::default();
    let mut defs: FxHashMap<VarId, StmtId> = FxHashMap::default();
    collect(f, &mut uses, &mut defs);

    let mut removed = FxHashSet::default();
    let mut queue: VecDeque<StmtId> = VecDeque::new();
    for (_, seq) in f.seqs.iter() {
        for &s in &seq.stmts {
            if removable(m, f, id, sets, s, &uses) {
                queue.push_back(s);
            }
        }
    }
    while let Some(s) = queue.pop_front() {
        if !removed.insert(s) {
            continue;
        }
        let Stmt::Let { value, .. } = &f.stmts[s] else {
            removed.remove(&s);
            continue;
        };
        let mut vars = Vec::new();
        value.for_each_var(|v| vars.push(v));
        for v in vars {
            let Some(count) = uses.get_mut(&v) else {
                continue;
            };
            *count = count.saturating_sub(1);
            if *count == 0
                && let Some(&def) = defs.get(&v)
                && !removed.contains(&def)
                && removable(m, f, id, sets, def, &uses)
            {
                queue.push_back(def);
            }
        }
    }
    removed
}

/// Whether `s` may be removed: a pure `let` with no edge, whose destination is dead.
fn removable(
    m: &Module,
    f: &Function,
    id: FnId,
    sets: &LiveSets,
    s: StmtId,
    uses: &FxHashMap<VarId, u32>,
) -> bool {
    let Stmt::Let {
        dst,
        value,
        on_defect,
        on_suspend,
    } = &f.stmts[s]
    else {
        return false;
    };
    // A defect or suspension edge is a way out of the statement and part of its meaning.
    if on_defect.is_some() || on_suspend.is_some() {
        return false;
    }
    if !is_pure_value(m, value) {
        return false;
    }
    // `None` is "no answer", not "not live"; the analysis must have keyed the statement.
    if liveness::live_after(sets, id, s).is_none() {
        return false;
    }
    match dst {
        // Nothing is produced or observed.
        None => true,
        // A reference may be owned, released or saved by a frame even when no later statement
        // reads it. Scalars are the safe first case; references are deferred to an ownership
        // proof (audit C0/C7).
        Some(d) => f.vars[*d].repr != Repr::Ref && uses.get(d).copied().unwrap_or(0) == 0,
    }
}

/// Whether evaluating `v` and throwing the result away changes nothing observable.
///
/// Deliberately small: a constant, or a primitive operation other than division and remainder
/// (whose zero divisor is a defect). Everything else returns `false`: a call, builtin,
/// runtime or fiber operation may effect, fail, defect or suspend and may consume its arguments; an
/// allocation, tuple, list, closure, handler or evidence extension allocates and may have a
/// finalizer to run at the release; a view or coercion may keep a reference alive or box a value.
/// Not being able to classify a value is a reason to keep it.
fn is_pure_value(m: &Module, v: &Value) -> bool {
    // Deleting a value that may raise a defect is forbidden (parent plan §3.3).
    if crate::heap::may_defect(m, v) {
        return false;
    }
    match v {
        Value::Const(_) => true,
        Value::Prim {
            op: PrimOp::Div | PrimOp::Rem,
            ..
        } => false,
        Value::Prim { .. } => true,
        _ => false,
    }
}

/// Counts every read of every variable of `f`, and the statement that defines it.
fn collect(f: &Function, uses: &mut FxHashMap<VarId, u32>, defs: &mut FxHashMap<VarId, StmtId>) {
    for (_, seq) in f.seqs.iter() {
        for &s in &seq.stmts {
            match &f.stmts[s] {
                Stmt::Let {
                    dst,
                    value,
                    on_suspend,
                    ..
                } => {
                    if let Some(d) = dst {
                        defs.insert(*d, s);
                    }
                    value.for_each_var(|v| *uses.entry(v).or_insert(0) += 1);
                    if let Some(edge) = on_suspend {
                        for &v in &edge.saved {
                            *uses.entry(v).or_insert(0) += 1;
                        }
                    }
                }
                Stmt::If { cond, .. } => operand(cond, uses),
                Stmt::Switch { on, .. } => {
                    let (crate::ir::SwitchOn::Tag(v) | crate::ir::SwitchOn::Lit(v)) = on;
                    *uses.entry(*v).or_insert(0) += 1;
                }
                Stmt::Loop { inits, .. } => {
                    for o in inits {
                        operand(o, uses);
                    }
                }
                Stmt::Block { .. } | Stmt::Protect { .. } => {}
                Stmt::Dup(v) | Stmt::Drop(v) => *uses.entry(*v).or_insert(0) += 1,
            }
        }
        term(&seq.term, uses);
    }
}

fn operand(o: &Operand, uses: &mut FxHashMap<VarId, u32>) {
    if let Operand::Var(v) = o {
        *uses.entry(*v).or_insert(0) += 1;
    }
}

fn term(t: &Term, uses: &mut FxHashMap<VarId, u32>) {
    match t {
        Term::Yield(v) | Term::Return(v) | Term::Break { value: v, .. } => {
            if let Some(o) = v {
                operand(o, uses);
            }
        }
        Term::Fail(v) | Term::Defect(v, _) => operand(v, uses),
        Term::Recur(args) => {
            for o in args {
                operand(o, uses);
            }
        }
        Term::Unwind | Term::Unreachable => {}
    }
}

#[cfg(test)]
mod tests {
    use emela_core::{Module as CoreModule, TypeId};

    use super::*;
    use crate::ir::{Const, RuntimeOp, Stage};
    use crate::{FnBuilder, Module};

    /// A module with one function, at `stage`.
    fn module(stage: Stage) -> (Module, FnId) {
        let core = CoreModule::new();
        let mut b = FnBuilder::new(&core, "f");
        b.set_ret(TypeId::INT);
        let x = b.assign("x", TypeId::INT, Value::Const(Const::Int(1)));
        b.term(Term::Return(Some(x.into())));
        let f = b.finish();
        let mut m = Module::new(core);
        let id = m.add(f);
        m.stage = stage;
        (m, id)
    }

    /// Before the Heap IR the analysis answers `Unknown`: the pass changes nothing and reports one
    /// deopt per function, rather than reading `Unknown` as "not live".
    #[test]
    fn unknown_liveness_deopts_the_whole_function() {
        let (mut m, _) = module(Stage::Flow);
        let before = crate::dump(&m);
        let stats = run(&mut m);
        assert_eq!(stats.dce, 0);
        assert_eq!(stats.dce_deopts, 1);
        assert_eq!(crate::dump(&m), before);
    }

    /// Only constants and safe primitive operations are pure; every other value is kept.
    #[test]
    fn only_constants_and_safe_primitives_are_pure() {
        let core = CoreModule::new();
        let mut b = FnBuilder::new(&core, "g");
        let p = b.var("p", TypeId::INT);
        b.term(Term::Return(Some(p.into())));
        let f = b.finish();
        let mut m = Module::new(core);
        m.add(f);
        let prim = |op| Value::Prim {
            op,
            ty: crate::ir::PrimTy::I32,
            args: vec![Const::Int(1).into(), Const::Int(2).into()],
        };
        assert!(is_pure_value(&m, &Value::Const(Const::Int(1))));
        assert!(is_pure_value(&m, &prim(PrimOp::Add)));
        assert!(!is_pure_value(&m, &prim(PrimOp::Div)));
        assert!(!is_pure_value(&m, &prim(PrimOp::Rem)));
        assert!(!is_pure_value(
            &m,
            &Value::Tuple(vec![Const::Int(1).into()])
        ));
        assert!(!is_pure_value(
            &m,
            &Value::List {
                elems: vec![Const::Int(1).into()],
                tail: None,
            }
        ));
        assert!(!is_pure_value(
            &m,
            &Value::Builtin {
                op: emela_core::Builtin::StringLength,
                args: vec![Const::String("x".into()).into()],
            }
        ));
        assert!(!is_pure_value(
            &m,
            &Value::Runtime {
                op: RuntimeOp::DebugPrint,
                args: vec![Const::String("x".into()).into()],
            }
        ));
        assert!(!is_pure_value(
            &m,
            &Value::Call {
                callee: crate::ir::Callee::Indirect(p),
                args: Vec::new(),
                suspends: false,
                on_fail: None,
            }
        ));
    }

    /// A dead reference is kept: the pass has no ownership proof for it.
    #[test]
    fn a_dead_reference_is_kept() {
        let core = CoreModule::new();
        let mut b = FnBuilder::new(&core, "ref");
        b.set_ret(TypeId::INT);
        let s = b.var("s", TypeId::STRING);
        let stmt = b.stmt(Stmt::Let {
            dst: Some(s),
            value: Value::Const(Const::String("dead".into())),
            on_defect: None,
            on_suspend: None,
        });
        b.term(Term::Return(Some(Const::Int(0).into())));
        let f = b.finish();
        let mut m = Module::new(core);
        let id = m.add(f);
        m.stage = Stage::RefCounted;
        let Fact::Known(sets) = liveness::analyze(&m) else {
            panic!("liveness is unknown at the Heap IR");
        };
        let uses = FxHashMap::default();
        assert!(!removable(&m, &m.functions[id], id, &sets, stmt, &uses));
    }
}
