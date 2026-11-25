//! Perceus reuse / FBIP (C4/PR-042): a uniquely owned object of a constructor is written again
//! with the same constructor, instead of being released and allocated again. Every reference held
//! by a field of the old object is released before the new value is written into that slot.
//!
//! # Contract
//!
//! ```text
//! Rule ID: C4 Perceus reuse (constructor reuse, same constructor)
//! Theory: Perceus reuse / FBIP (Reinking et al.); uniqueness from the ownership analysis (B6)
//!         and the reference-counting drop point
//! Equation: `let v = Make C(args0); …; drop v; let d = Make C(args1)` becomes
//!           `let v = Make C(args0); …; let d = Make C(args1)` with `d` reusing the block of `v`;
//!           the backend releases every slot of the old block that holds a reference, in slot
//!           order, and then writes the new fields into the same slots
//! Preconditions: the module is at Stage::RefCounted; `v` is defined in the same function by a
//!                `let` whose value is `Make` with the same constructor as the new one; `v` is
//!                read exactly once in the whole function and that read is this `drop`; `v` and
//!                the new destination are `Repr::Ref` and have the same type; the constructor is
//!                neither an error enum nor `@external` and its type has no finalizer; the new
//!                `let` has no defect landing and no suspension edge; the ownership analysis does
//!                not answer `Known(false)` for `v` after its definition. A field may be held as
//!                a reference: it is released exactly once, before it is overwritten.
//! Postconditions: the module is still at Stage::RefCounted and passes the verifier. The `drop`
//!                 is removed and [`Function::reuse`](crate::ir::Function::reuse) maps the new
//!                 `Make` statement to `v`; the backends assign the destination the old block,
//!                 release each old field whose type is held as a reference, and write the new
//!                 fields into it. The variable `v` is consumed by that `Make` (the balance check
//!                 and the ownership analysis read the map): its reference moves to the
//!                 destination instead of leaving on the drop.
//! Preserves: evaluation order and every effect, failure, defect and suspension edge; the
//!            arguments are values computed before the `Make`, as they were; the fields of the old
//!            block release the references they own, once each, at the point of the removed drop
//!            (the two statements are adjacent), in slot order; the order and count of the
//!            releases of `with` and of every other drop; the evidence tables; stdout, stderr, exit
//!            code and test results. Only the lifetime of one block and the number of
//!            allocations change, both implementation-defined (`[target.unspecified]`).
//! Deopt: `Known(false)` from the ownership analysis at `v`'s definition (`Unknown` is not a veto:
//!        the exact one-use proof stands on its own); a `v`
//!        with a use other than the drop (a `dup`, a view or its base, a call argument, a
//!        container, a capture, a switch or condition, a loop initial value, a terminator, a
//!        frame save, a landing); `v` not defined by a `Make` of the same constructor in the
//!        function; a destination or a value that is not `Repr::Ref`, or a destination of another
//!        type than `v` (the same constructor of other type arguments has other reference slots);
//!        an error enum, an `@external` type or a type with a finalizer; a defect landing or a
//!        suspension edge on the new `let`. A `Drop` not immediately before the `Make` is not a
//!        candidate.
//! Budget: one use count and one scan of every function's sequences; O(statements).
//! Measures: [`Stats::reuse`] (reused constructors) and [`Stats::reuse_deopts`] (candidate pairs
//!           left alone).
//! ```
//!
//! # Position in the pipeline
//!
//! [`run_heap_with`](crate::opt::run_heap_with) runs this pass last, after the `rc` cleanup, DCE
//! and drop specialisation. Reference counting has then placed every `drop`, so `drop v`
//! immediately before a `Make` is a real last use of `v`; DCE has removed the dead scalar `let`s
//! that would otherwise sit between the two; and drop specialisation has recorded the kind of
//! every drop the pass may remove. A drop removed here leaves no entry in
//! [`Function::drop_kinds`](crate::ir::Function::drop_kinds) because the pass deletes the entry
//! with the statement. Drop specialisation may already have counted that drop in
//! [`Stats::drop_specialized`](crate::opt::Stats::drop_specialized): the counters describe each
//! pass's own run, in order, not the statements that remain at the end.
//!
//! # Why the proof is "one use, and it is the drop"
//!
//! The reuse writes the fields of a live block. That is only not observable when nothing else can
//! read the block: no other reference, no view of it, no frame that keeps it, no alias that a
//! container or an evidence table holds. The ownership analysis sees a variable's own uses but not
//! aliases made through a value the pass does not follow; the stronger and exact proof is that the
//! variable is read exactly once in the whole function, by the drop that is being removed. A
//! second reference would need a `dup`, a view, a consuming use or a frame save, and each of those
//! is a read of the variable, so the count of one excludes all of them. This is why the pass also
//! accepts `Unknown` from the ownership analysis (it is a belt-and-braces check) while it would
//! never accept `Known(false)`.
//!
//! # Why a reference field can be released before it is overwritten
//!
//! At [`Stage::RefCounted`] the balance check ([`heap::rc_check`](crate::heap::rc_check)) holds
//! every function to the ownership rules of [`heap::rc`](crate::heap::rc). A field of `Make` is a
//! consuming use ([`heap::rc::value_uses`](crate::heap::rc)), so the reference counting pass gave
//! the operand an owned reference before the statement: the variable's own last reference, or a
//! `dup` when it stays live, and a view without a `dup` is rejected by the balance check. A
//! `Repr::Ref` constant is static data (`rc == 0`) and carries no count. The operand therefore
//! keeps its value alive on its own, independently of the old block.
//!
//! The old slot owns one reference too, the one the old `Make` moved into it. Releasing the old
//! slot with the same drop the removed `drop` would have run returns exactly that one reference;
//! the block stays alive as the destination. When the operand and the old field denote the same
//! object, its count is at least two (the slot's reference and the operand's), so the release
//! leaves at least one and the following store cannot free it. A release in this runtime runs no
//! user code: the finalizers of the C and WASM runtimes are the runtime's own fiber, scope,
//! `Deferred` and resume bookkeeping, so releasing the slots one at a time in slot order has no
//! observable order. The set of references released is exactly the one the old block's drop would
//! have released, once each.
//!
//! The one-use proof also rules out reaching the old fields through another variable: a view of
//! the old value or of one of its fields reads the old variable, and a second reference to the old
//! block (a `dup`, a container, a frame) reads it as well. The block is therefore not reachable
//! from anywhere else. The objects its fields point to may have other owners; their references
//! are exactly what keeps them alive when the old slot's reference is released.
//!
//! Arrays, lists, closures and tuples are not constructors of this rule.
//!
//! # Why the drop must be immediately before the `Make`
//!
//! The rewrite removes the release and gives the block to the value the `Make` builds, so the two
//! statements must be adjacent in the same sequence. A statement between them could be left
//! without running the `Make` (a defect landing that returns or unwinds, or a later way out of the
//! region), and the block would then be owned by nobody on that path: the balance check would
//! reject the function and a backend would leak the block. Adjacent statements run the same number
//! of times on every path, so the owner changes exactly where the release was, and the pass has
//! the same amount of allocation (one object) on every path.
//!
//! # Status
//!
//! C4/PR-042: same-constructor reuse of a uniquely owned value, reference fields included. Behind
//! [`LirRules::reuse`](crate::opt::LirRules::reuse), off by default.

use emela_core::{CtorRef, Module as CoreModule, Type};
use rustc_hash::FxHashMap;

use super::Stats;
use super::analysis::{Fact, ownership};
use super::drop_spec::has_finalizer;
use crate::ir::{FnId, Function, Module, Operand, Stage, Stmt, StmtId, Value, VarId};
use crate::repr::Repr;

/// Reuses the uniquely owned constructors of the module and returns what it changed.
///
/// Preconditions, postconditions, preserved behaviour, deopt, budget and measures: see the module
/// documentation. Running it again finds no `drop` followed by a `Make`, so it changes nothing (it
/// is idempotent).
pub fn run(m: &mut Module) -> Stats {
    let mut stats = Stats::default();
    if m.stage != Stage::RefCounted {
        // Before the Heap IR there is no drop to consume and no uniqueness answer: every function
        // is a deopt, and nothing is read as "not unique". This is the same rule DCE uses.
        stats.reuse_deopts = m.functions.iter().filter(|(_, f)| f.body.is_some()).count() as u32;
        return stats;
    }
    // `None` is "no answer": the exact one-use proof below stands on its own, and the analysis is
    // only allowed to veto a candidate with `Known(false)`.
    let facts = ownership::analyze(m).known();
    let core = &m.core;
    let ids: Vec<FnId> = m.functions.iter().map(|(id, _)| id).collect();
    for id in ids {
        if m.functions[id].body.is_none() {
            continue;
        }
        let f = &mut m.functions[id];
        let uses = uses_of(f);
        let defs = defs_of(f);
        let seqs: Vec<_> = f.seqs.iter().map(|(sid, _)| sid).collect();
        for sid in seqs {
            let mut i = 0;
            loop {
                let len = f.seqs[sid].stmts.len();
                if i + 1 >= len {
                    break;
                }
                let drop_id = f.seqs[sid].stmts[i];
                let make_id = f.seqs[sid].stmts[i + 1];
                let Stmt::Drop(v) = f.stmts[drop_id] else {
                    i += 1;
                    continue;
                };
                // The candidate must be a `let d = Make` with no edge of its own.
                let make = match f.stmts[make_id].clone() {
                    Stmt::Let {
                        dst: Some(d),
                        value: Value::Make { ctor, .. },
                        on_defect: None,
                        on_suspend: None,
                    } => Some((d, ctor)),
                    _ => None,
                };
                let Some((d, ctor)) = make else {
                    i += 1;
                    continue;
                };
                // Without a definition by a `Make` in this function, `v` is not a constructor this
                // rule could reuse: it is not a candidate, not a deopt.
                let Some(&def_id) = defs.get(&v) else {
                    i += 1;
                    continue;
                };
                let definition = match f.stmts[def_id].clone() {
                    Stmt::Let {
                        dst: Some(dv),
                        value: Value::Make { ctor: c0, .. },
                        on_defect: None,
                        on_suspend: None,
                    } if dv == v => Some(c0),
                    _ => None,
                };
                let Some(c0) = definition else {
                    i += 1;
                    continue;
                };
                if eligible(core, f, facts.as_ref(), id, def_id, &uses, v, d, c0, ctor) {
                    // The owner of the block moves to `d` at the `Make`; the old variable is dead
                    // and the balance check reads the map (see `heap::rc_check`).
                    f.seqs[sid].stmts.remove(i);
                    f.reuse.insert(make_id, v);
                    f.drop_kinds.remove(&drop_id);
                    stats.reuse += 1;
                } else {
                    stats.reuse_deopts += 1;
                    i += 1;
                }
            }
        }
    }
    stats
}

/// Whether the pair is a sound reuse: every condition of the module contract.
#[allow(clippy::too_many_arguments)]
fn eligible(
    core: &CoreModule,
    f: &Function,
    facts: Option<&ownership::UniqueRefs>,
    function: FnId,
    def: StmtId,
    uses: &FxHashMap<VarId, u32>,
    v: VarId,
    d: VarId,
    old_ctor: CtorRef,
    new_ctor: CtorRef,
) -> bool {
    // A value held by value has no block to reuse: the destination and the value are both
    // references when the backend may have allocated one.
    if v == d || f.vars[v].repr != Repr::Ref || f.vars[d].repr != Repr::Ref {
        return false;
    }
    // The same type fixes the same field layout, and so the same reference slots: the same
    // constructor of other type arguments can hold a reference in other slots.
    if f.vars[v].ty != f.vars[d].ty {
        return false;
    }
    // The descriptor, the tag and the number of non-representation slots come from the
    // constructor: the same constructor gives the same descriptor.
    if old_ctor != new_ctor {
        return false;
    }
    // The exact uniqueness proof: one read in the whole function. The drop is a read, so it is
    // that one read.
    if uses.get(&v).copied().unwrap_or(0) != 1 {
        return false;
    }
    // Error values are told apart at run time and their descriptors are not shared by constructor
    // alone; an `@external` type and every finalizer-bearing type must run a release, which this
    // rule does not model. A reference field is fine: the backends release the old field before
    // they overwrite it (see the module documentation). (An enum type is never
    // Handler/Evidence/Cell/FiberScope/Any; the finalizer test is stated in terms of the type,
    // like drop specialisation states it.)
    let Type::Enum(enum_id, _) = core.ty(f.vars[v].ty) else {
        return false;
    };
    if *enum_id != new_ctor.enum_id {
        return false;
    }
    let enum_def = &core.enums[*enum_id];
    if enum_def.error || enum_def.external || has_finalizer(core, f.vars[v].ty) {
        return false;
    }
    // The ownership analysis may only veto. `Known(false)` says some use shares the value; `Unknown`
    // is not a "not unique" answer for this consumer.
    if let Some(Fact::Known(false)) =
        facts.map(|facts| ownership::unique_after(facts, function, def, v))
    {
        return false;
    }
    true
}

/// How many times every variable of `f` is read, including the reads of landings, of suspension
/// frames and of unreachable statements (a conservative over-count).
fn uses_of(f: &Function) -> FxHashMap<VarId, u32> {
    let mut uses = FxHashMap::default();
    let mut add = |v: VarId| *uses.entry(v).or_insert(0) += 1;
    for (_, stmt) in f.stmts.iter() {
        match stmt {
            Stmt::Let {
                value, on_suspend, ..
            } => {
                value.for_each_var(&mut add);
                if let Some(edge) = on_suspend {
                    for &v in &edge.saved {
                        add(v);
                    }
                }
            }
            Stmt::If { cond, .. } => {
                if let Operand::Var(v) = cond {
                    add(*v);
                }
            }
            Stmt::Switch { on, .. } => {
                let (crate::ir::SwitchOn::Tag(v) | crate::ir::SwitchOn::Lit(v)) = on;
                add(*v);
            }
            Stmt::Loop { inits, .. } => {
                for o in inits {
                    if let Operand::Var(v) = o {
                        add(*v);
                    }
                }
            }
            Stmt::Block { .. } | Stmt::Protect { .. } => {}
            Stmt::Dup(v) | Stmt::Drop(v) => add(*v),
        }
    }
    for (_, seq) in f.seqs.iter() {
        match &seq.term {
            crate::ir::Term::Yield(v)
            | crate::ir::Term::Return(v)
            | crate::ir::Term::Break { value: v, .. } => {
                if let Some(Operand::Var(v)) = v {
                    add(*v);
                }
            }
            crate::ir::Term::Fail(v) | crate::ir::Term::Defect(v, _) => {
                if let Operand::Var(v) = v {
                    add(*v);
                }
            }
            crate::ir::Term::Recur(args) => {
                for o in args {
                    if let Operand::Var(v) = o {
                        add(*v);
                    }
                }
            }
            crate::ir::Term::Unwind | crate::ir::Term::Unreachable => {}
        }
    }
    uses
}

/// The `let` statement that defines each variable of `f`.
fn defs_of(f: &Function) -> FxHashMap<VarId, StmtId> {
    let mut defs = FxHashMap::default();
    for (sid, stmt) in f.stmts.iter() {
        if let Stmt::Let { dst: Some(d), .. } = stmt {
            defs.insert(*d, sid);
        }
    }
    defs
}
