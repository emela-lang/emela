//! Closure devirtualisation: a call through a closure record that is known to be one lifted
//! function becomes a direct call of that function.
//!
//! Closure conversion makes every function value a closure record, `{function, captures…}`, and
//! every call through it an indirect call: the record is the environment, the backends read the
//! function out of it and dispatch. When the record was made in the same function and is used for
//! nothing but being called, the function is statically known, so the dispatch is dead: the call
//! can name it directly.
//!
//! The rewrite keeps the record and its captures exactly where they are. The record is still
//! made, still owns the captures, is still released at the same point, and is passed as the
//! environment argument of the direct call, which borrows it just as the indirect call borrowed
//! its callee (see `heap::rc`, "Borrowed"). Devirtualisation therefore moves no ownership and no
//! drop point: only the dispatch changes. Removing the record and reading the captures out of
//! their variables is scalar replacement of the closure (C5/PR-043), which needs a stronger
//! ownership proof; this pass does not do it.
//!
//! # Contract
//!
//! ```text
//! Rule ID: E5 closure devirtualisation
//! Theory: dispatch elimination for a closure record that is created and only called in the same
//!         function
//! Equation: `c = make_closure f[captures]; … call_indirect c(args) …`
//!           -> `c = make_closure f[captures]; … call f(c, args) …`
//!           when every use of `c` is a `call_indirect c`, `c` does not escape and the call fits
//!           `f`'s signature
//! Preconditions: the module is at Stage::ClosuresConverted, so a closure is a lifted function and
//!                a `make_closure` record and there is no `Suspend` yet; the record is defined by a
//!                `let` with a destination and without a defect landing or a suspension edge; the
//!                record is not in the escape set of its function (and the analysis answers
//!                `Known`); every use of the record, anywhere in the function, is the callee of an
//!                indirect call; the target is a lifted function of the module; the call's
//!                `suspends` and failure edge are exactly the target's `fails` and `suspends`, the
//!                arguments are the target's parameters after the environment, and the result's
//!                representation is the target's
//! Postconditions: each such call is `call f(c, args)`; the record, its captures, its definition
//!                 and its uses are otherwise unchanged; the module is still at
//!                 Stage::ClosuresConverted and verifies
//! Preserves: evaluation order (the callee was a read of the record before the arguments, and the
//!            environment argument is a read of the same record before the same arguments), effects,
//!            the order and count of effects, failure edges, defect landings, suspension points and
//!            the frames they save, the releases of `with`, drop points and their relative order,
//!            the ownership of the record and of its captures, the evidence tables, and
//!            stdout/stderr/exit code and test results. The record is still owned by its function
//!            and still borrowed by the call, so reference counting inserts the same `dup`s,
//!            `drop`s and `Suspend::saved` entries as for the indirect call.
//! Deopt: the record of
//!        - escapes ([`escape`] says so: it is passed to a call, stored, captured, returned, failed
//!          with, boxed, or live across a suspension point, which is the case when such a call is
//!          followed by another use of the record);
//!        - is used for anything else, anywhere in the function: an operand, a capture, a handler
//!          operation, a `dup`/`drop`, a terminator, a view (`coerce` included);
//!        - has a `let` with no destination, a defect landing or a suspension edge;
//!        - makes a call whose signature the target does not fit (a type that only differs in the
//!          `fails`/`suspends` flags, an argument list that differs, another result representation);
//!        - belongs to a function the escape analysis answers `Unknown` for.
//!        Nothing is guessed: a candidate that fails one of these is left exactly as it was.
//! Budget: one pass over the statements of each function, no fixpoint and no re-run. A rewrite
//!         adds no statement and no variable, so the number of rewrites is bounded by the number of
//!         indirect calls of the module, and a second run finds no candidate (after the rewrite the
//!         record is an argument, not a callee).
//! Measures: [`Stats::devirt`] (indirect calls made direct) and [`Stats::devirt_deopts`]
//!           (candidates left alone, and functions left alone because the escape analysis answered
//!           `Unknown`).
//! ```

use rustc_hash::FxHashMap;

use super::Stats;
use crate::ir::{
    Callee, FnId, Function, Module, Operand, Stmt, StmtId, SwitchOn, Term, Value, VarId,
};
use crate::opt::analysis::Fact;
use crate::opt::analysis::escape::{self, EscapeSets};
use crate::repr::{Repr, repr_of};

/// Devirtualises the closure calls whose record the escape analysis and the uses of the function
/// make statically known.
///
/// Preconditions: the module is at [`Stage::ClosuresConverted`](crate::ir::Stage::ClosuresConverted)
/// in the pipeline ([`super::run_mid_with`] calls it there). The escape facts are those of the
/// module ([`escape::analyze`]).
pub fn run(m: &mut Module) -> Stats {
    run_with(m, escape::analyze(m))
}

/// [`run`] with escape facts a caller already computed.
///
/// The pass reads the facts only to confirm that a record it would rewrite does not escape, so a
/// module-wide analysis can be shared with another pass. `Fact::Unknown` leaves every function
/// alone: it is never read as "nothing escapes". Tests use this entry to exercise that deopt,
/// because [`escape::analyze`] answers `Known` for every module today.
pub fn run_with(m: &mut Module, escaping: Fact<EscapeSets>) -> Stats {
    assert_eq!(
        m.stage,
        crate::ir::Stage::ClosuresConverted,
        "closure devirtualisation runs after closure conversion and before the state machines"
    );
    let mut stats = Stats::default();
    let sets = escaping.known();
    let functions: Vec<FnId> = m.functions.iter().map(|(id, _)| id).collect();
    for id in functions {
        let f = &m.functions[id];
        if f.body.is_none() {
            continue;
        }
        let uses = uses(f);
        let candidates = candidates(f, &uses);
        if candidates.is_empty() {
            continue;
        }
        let Some(escaping) = sets.as_ref().and_then(|sets| sets.get(&id)) else {
            // No answer for this function: do not read `Unknown` as "does not escape".
            stats.devirt_deopts += candidates.len() as u32;
            continue;
        };
        let mut rewrite: Vec<(StmtId, FnId)> = Vec::new();
        for candidate in &candidates {
            if escaping.contains(&candidate.record) {
                stats.devirt_deopts += 1;
                continue;
            }
            if candidate
                .sites
                .iter()
                .all(|&site| fits(m, f, site, candidate.record, candidate.function))
            {
                rewrite.extend(
                    candidate
                        .sites
                        .iter()
                        .map(|&site| (site, candidate.function)),
                );
            } else {
                stats.devirt_deopts += 1;
            }
        }
        if rewrite.is_empty() {
            continue;
        }
        let f = &mut m.functions[id];
        for (site, function) in &rewrite {
            make_direct(f, *site, *function);
        }
        stats.devirt += rewrite.len() as u32;
    }
    stats
}

/// A record that may be devirtualised: `record = make_closure function[captures]`, with every use
/// of `record` a `call_indirect record`.
struct Candidate {
    record: VarId,
    function: FnId,
    /// The `let`s of the indirect calls through `record`, in statement order.
    sites: Vec<StmtId>,
}

/// What the function does with each variable: the indirect calls through it, and every other use.
#[derive(Default)]
struct Uses {
    /// `let`s whose value is `call_indirect record(..)`.
    sites: Vec<StmtId>,
    /// Uses other than being the callee of an indirect call: an operand, a capture, a handler
    /// operation, a base of a view, a `dup`/`drop`, a terminator.
    other: u32,
}

/// Classifies every use in the function.
///
/// Every statement is in some sequence, so walking the sequence arena (and not only the body) sees
/// them all; the result is conservative for a statement an earlier pass left unreachable. Failure
/// landings and defect landings are sequences of the arena too, so this walk covers them.
fn uses(f: &Function) -> FxHashMap<VarId, Uses> {
    fn entry(uses: &mut FxHashMap<VarId, Uses>, v: VarId) -> &mut Uses {
        uses.entry(v).or_default()
    }

    fn operand(uses: &mut FxHashMap<VarId, Uses>, o: &Operand) {
        if let Operand::Var(v) = o {
            entry(uses, *v).other += 1;
        }
    }

    fn stmt(f: &Function, id: StmtId, uses: &mut FxHashMap<VarId, Uses>) {
        match &f.stmts[id] {
            Stmt::Let {
                value, on_suspend, ..
            } => {
                match value {
                    Value::Call {
                        callee: Callee::Indirect(v),
                        args,
                        ..
                    } => {
                        entry(uses, *v).sites.push(id);
                        args.iter().for_each(|o| operand(uses, o));
                    }
                    other => other.for_each_var(|v| entry(uses, v).other += 1),
                }
                if let Some(suspend) = on_suspend {
                    for &v in &suspend.saved {
                        entry(uses, v).other += 1;
                    }
                }
            }
            Stmt::If { cond, .. } => operand(uses, cond),
            Stmt::Switch { on, .. } => {
                let (SwitchOn::Tag(v) | SwitchOn::Lit(v)) = on;
                entry(uses, *v).other += 1;
            }
            Stmt::Loop { inits, .. } => inits.iter().for_each(|o| operand(uses, o)),
            Stmt::Block { .. } | Stmt::Protect { .. } => {}
            Stmt::Dup(v) | Stmt::Drop(v) => entry(uses, *v).other += 1,
        }
    }

    fn term(t: &Term, uses: &mut FxHashMap<VarId, Uses>) {
        let mut operand = |o: &Operand| operand(uses, o);
        match t {
            Term::Yield(v) | Term::Return(v) | Term::Break { value: v, .. } => {
                if let Some(v) = v {
                    operand(v);
                }
            }
            Term::Fail(v) | Term::Defect(v, _) => operand(v),
            Term::Recur(args) => args.iter().for_each(&mut operand),
            Term::Unwind | Term::Unreachable => {}
        }
    }

    let mut uses = FxHashMap::default();
    for (_, seq) in f.seqs.iter() {
        for &id in &seq.stmts {
            stmt(f, id, &mut uses);
        }
        term(&seq.term, &mut uses);
    }
    uses
}

/// The records of the function that are created locally and used only as the callee of indirect
/// calls, in statement order.
fn candidates(f: &Function, uses: &FxHashMap<VarId, Uses>) -> Vec<Candidate> {
    let mut out = Vec::new();
    for (_, seq) in f.seqs.iter() {
        for &id in &seq.stmts {
            let Stmt::Let {
                dst: Some(record),
                value: Value::MakeClosure { function, .. },
                on_defect: None,
                on_suspend: None,
            } = &f.stmts[id]
            else {
                continue;
            };
            let Some(used) = uses.get(record) else {
                continue;
            };
            if used.other > 0 || used.sites.is_empty() {
                continue;
            }
            out.push(Candidate {
                record: *record,
                function: *function,
                sites: used.sites.clone(),
            });
        }
    }
    out
}

/// Whether the indirect call at `site` through `record` can be the direct call of `function`.
///
/// The record becomes the environment argument, which a lifted function takes first; the call's
/// arguments follow it. The flags and the result must agree with the target, because a direct
/// call is checked against the target's body while an indirect one is checked against the record's
/// function type, which may be wider: a type may fail or suspend where the target does not.
fn fits(m: &Module, f: &Function, site: StmtId, record: VarId, function: FnId) -> bool {
    let target = &m.functions[function];
    if target.env.is_none() {
        return false;
    }
    let Stmt::Let {
        dst,
        value:
            Value::Call {
                callee: Callee::Indirect(callee),
                args,
                suspends,
                on_fail,
            },
        ..
    } = &f.stmts[site]
    else {
        return false;
    };
    if *callee != record {
        return false;
    }
    // The environment comes first, then the arguments the record's function type lists.
    if target.params.len() != args.len() + 1 {
        return false;
    }
    let operands = &target.params[1..];
    if operands
        .iter()
        .zip(args)
        .any(|(&p, a)| target.vars[p].repr != operand_repr(f, a))
    {
        return false;
    }
    if *suspends != target.suspends || on_fail.is_some() != target.fails {
        return false;
    }
    let result = repr_of(&m.core, target.ret);
    dst.is_none_or(|d| f.vars[d].repr == result)
}

/// The representation of an operand, as the verifier checks it.
fn operand_repr(f: &Function, o: &Operand) -> Repr {
    match o {
        Operand::Var(v) => f.vars[*v].repr,
        Operand::Const(c) => c.repr(),
    }
}

/// `call_indirect record(args)` becomes `call function(record, args)`. The record is the
/// environment, and the call above checked that the arguments are the target's parameters after it.
fn make_direct(f: &mut Function, site: StmtId, function: FnId) {
    let Stmt::Let {
        value: Value::Call { callee, args, .. },
        ..
    } = &mut f.stmts[site]
    else {
        unreachable!("a devirtualisation site is a call");
    };
    let Callee::Indirect(record) = *callee else {
        unreachable!("a devirtualisation site is an indirect call");
    };
    *callee = Callee::Direct(function);
    args.insert(0, Operand::Var(record));
}
