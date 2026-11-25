//! Switch normalisation: the target-independent form of a `switch`, made once in the LIR.
//!
//! Both backends used to decide how to dispatch a `switch` themselves, and each re-derived facts
//! about its cases. This pass writes the canonical form instead, so the C and WASM emitters only
//! encode what is left: C lists the labels of a case and lets its compiler choose between a jump
//! table and comparisons; WASM asks [`variant_span`] whether a `br_table` is worth its bytes and
//! otherwise writes the chain of `if`s.
//!
//! # Contract
//!
//! ```text
//! Rule ID: E4 switch normalisation (canonical cases, shared bodies, direct body)
//! Theory: canonical form of a multi-way branch (case grouping and ordering)
//! Equation:
//!   switch s { k1 -> B, k2 -> B }        becomes  switch s { k1, k2 -> B }
//!   switch s { k1 -> B }                 becomes  B                      (when no other arm)
//!   switch s { k1 -> B, ..., default -> B } becomes B                   (all arms the same)
//!   switch s { 0 -> B, 2 -> C }          becomes  switch s { 0 -> B, 2 -> C } (canonical order)
//! Preconditions: the module is at Stage::RefCounted. The pass reads `Stmt::Switch`; Flow IR
//!                and the stages before reference counting are left alone.
//! Postconditions: the module is still at Stage::RefCounted and passes the verifier. Every case
//!                 has at least one key, the keys of a case are sorted and distinct, the cases are
//!                 ordered by their least key, no key occurs in two cases, and two cases share one
//!                 body only when their bodies are the same sequence. A switch whose arms all run
//!                 the same body, or a tag switch whose single case covers every constructor, is
//!                 replaced by a `block` with that body (or by an unreachable block when it has no
//!                 arm at all); the sequences of the dropped arms are unreachable and are removed
//!                 by `heap::compact`.
//! Preserves: evaluation order, effects, fail and defect edges, suspension points and the frames
//!            they save, the order and count of releases, the evidence tables and drop points.
//!            The pass only joins arms that are structurally the same sequence (same statements,
//!            same terminators, same landings, same variables), so the body it keeps has the same
//!            effects and the same edges as the one it replaces. A direct body is chosen only when
//!            every arm is that same sequence, so exactly one arm still runs, and it is that body.
//!            stdout, stderr, exit code and test results are unchanged.
//! Deopt: a switch whose cases have different bodies, different landings or different suspension
//!        edges has them left unmerged. A literal switch is not made direct while its default has
//!        a different body (it is reachable for every other literal), and a tag switch is not made
//!        direct while it has a default with a different body. A case whose keys are unordered is
//!        only reordered, never split; a key that already appears in another case is dropped with
//!        the duplicate (the verifier rejects such a module, so this only keeps the pass total).
//! Budget: one walk of every function's statements; the body comparison of each pair of cases in
//!         one switch is O(size of the bodies). Statistically O(statements) for the switches the
//!         corpus has.
//! Measures: [`Stats::switch_cases_merged`] (case bodies joined) and [`Stats::switch_direct`]
//!           (switches replaced by a direct block).
//! ```
//!
//! # Why the body comparison is structural
//!
//! The pattern compiler gives each arm its own sequence, even when two variants have the same
//! arm (`Circle(_) | Rect(_) -> 0`): the rows are compiled per variant, so two equal bodies are
//! two `SeqId`s. Joining only cases that already share a `SeqId` would find almost nothing.
//! [`same_body`] compares the two trees statement by statement instead. It compares variables by
//! identity, so two arms that bind their own local variables are left alone unless those
//! variables are the same; that is the conservative slice. Landings and suspension edges are part
//! of the compared statements, so arms that fail, raise a defect or suspend differently are never
//! joined.
//!
//! # Position in the pipeline
//!
//! [`run_heap_with`](crate::opt::run_heap_with) runs it after the `rc` cleanup and DCE (the last
//! point at which bodies can become equal or dead) and before drop specialisation and reuse. Those
//! two write maps keyed by statement IDs, so they must see the final numbering; the pass compacts
//! a function it changed, and its own counters are independent of the IDs.
//!
//! # Status
//!
//! Behind [`LirRules::switch`](crate::opt::LirRules::switch), on by default: the rewrite only
//! joins identical sequences or removes a dispatch whose every arm is the same sequence, so it is
//! behaviour-preserving. It makes the emitted C and WASM smaller and is the canonical form the
//! backends read.

use std::cmp::Ordering;

use emela_core::Type;
use rustc_hash::FxHashSet;

use super::Stats;
use crate::heap::compact;
use crate::ir::{
    Case, CaseKey, Const, FnId, Function, LabelData, Module, Seq, SeqId, Stage, Stmt, SwitchOn,
    Term,
};

/// Normalises every switch of the module and returns what it changed.
///
/// Preconditions, postconditions, preserved behaviour, deopt, budget and measures: see the module
/// documentation. Running it again finds every switch canonical and changes nothing (it is
/// idempotent).
pub fn run(m: &mut Module) -> Stats {
    let mut stats = Stats::default();
    if m.stage != Stage::RefCounted {
        // The canonical form is what the Heap IR backends read, and the stage before reference
        // counting may still rewrite or remove a switch. Leave it alone rather than guess.
        return stats;
    }
    let ids: Vec<FnId> = m.functions.iter().map(|(id, _)| id).collect();
    for id in ids {
        if m.functions[id].body.is_none() {
            continue;
        }
        let mut f = m.functions[id].clone();
        let reachable = reachable_seqs(&f);
        let mut changed = false;
        for sid in reachable {
            let stmts = f.seqs[sid].stmts.clone();
            for stmt in stmts {
                let Stmt::Switch {
                    dst,
                    on,
                    cases,
                    default,
                } = f.stmts[stmt].clone()
                else {
                    continue;
                };
                let before = cases.len();
                let original = cases.clone();
                let cases = normalize_cases(&f, cases);
                let merged = before.saturating_sub(cases.len()) as u32;
                match direct_body(&m.core, &f, on, &cases, &default) {
                    Some(body) => {
                        let body = match body {
                            Some(body) => body,
                            // No arm at all: the switch never continues, whatever its
                            // destination.
                            None => f.seqs.alloc(Seq {
                                stmts: Vec::new(),
                                term: Term::Unreachable,
                            }),
                        };
                        let label = f.labels.alloc(LabelData {
                            hint: "switch".to_owned(),
                        });
                        f.stmts[stmt] = Stmt::Block { dst, label, body };
                        stats.switch_direct += 1;
                        stats.switch_cases_merged += merged;
                        changed = true;
                    }
                    None => {
                        if cases != original {
                            if let Stmt::Switch { cases: slot, .. } = &mut f.stmts[stmt] {
                                *slot = cases;
                            }
                            changed = true;
                        }
                        stats.switch_cases_merged += merged;
                    }
                }
            }
        }
        if changed {
            // The joined-away bodies are unreachable now. `compact` drops them and numbers what is
            // left; no map keyed by a statement or variable ID has been written yet in this stage
            // (drop specialisation and reuse run after this pass).
            m.functions[id] = compact(&f);
        }
    }
    stats
}

/// The sequences reachable from the function body, so that a pass does not rewrite an arm it has
/// just made unreachable.
fn reachable_seqs(f: &Function) -> Vec<SeqId> {
    let mut seen = FxHashSet::default();
    let mut out = Vec::new();
    if let Some(body) = f.body {
        push_seq(f, body, &mut seen, &mut out);
    }
    out
}

fn push_seq(f: &Function, id: SeqId, seen: &mut FxHashSet<SeqId>, out: &mut Vec<SeqId>) {
    if !seen.insert(id) {
        return;
    }
    out.push(id);
    for &stmt in &f.seqs[id].stmts {
        for_each_seq(&f.stmts[stmt], &mut |child| push_seq(f, child, seen, out));
    }
}

/// Calls `visit` for every sequence a statement holds.
fn for_each_seq(stmt: &Stmt, visit: &mut impl FnMut(SeqId)) {
    match stmt {
        Stmt::Let { on_defect, .. } => {
            if let Some(landing) = on_defect {
                visit(*landing);
            }
        }
        Stmt::If { then, else_, .. } => {
            visit(*then);
            visit(*else_);
        }
        Stmt::Switch { cases, default, .. } => {
            for case in cases {
                visit(case.body);
            }
            if let Some(d) = default {
                visit(*d);
            }
        }
        Stmt::Loop { body, .. } | Stmt::Block { body, .. } => visit(*body),
        Stmt::Protect { body, cleanup, .. } => {
            visit(*body);
            visit(*cleanup);
        }
        Stmt::Dup(_) | Stmt::Drop(_) => {}
    }
}

/// The canonical cases: keys sorted and distinct, cases ordered by their least key, and cases
/// with the same body joined.
fn normalize_cases(f: &Function, cases: Vec<Case>) -> Vec<Case> {
    let mut canonical: Vec<Case> = Vec::with_capacity(cases.len());
    for mut case in cases {
        case.keys.sort_by(key_order);
        case.keys
            .dedup_by(|a, b| key_order(a, b) == Ordering::Equal);
        if case.keys.is_empty() {
            continue;
        }
        canonical.push(case);
    }
    canonical.sort_by(|a, b| key_order(a.first_key(), b.first_key()));
    // A key belongs to the first case that lists it. The verifier rejects duplicates; keeping the
    // first keeps the pass total on a module that does not.
    let mut used: Vec<CaseKey> = Vec::new();
    for case in &mut canonical {
        case.keys.retain(|key| {
            if used
                .iter()
                .any(|seen| key_order(seen, key) == Ordering::Equal)
            {
                false
            } else {
                used.push(key.clone());
                true
            }
        });
    }
    canonical.retain(|case| !case.keys.is_empty());

    let mut joined: Vec<Case> = Vec::with_capacity(canonical.len());
    for case in canonical {
        if let Some(group) = joined
            .iter_mut()
            .find(|group| same_body(f, group.body, case.body))
        {
            group.keys.extend(case.keys);
        } else {
            joined.push(case);
        }
    }
    for case in &mut joined {
        case.keys.sort_by(key_order);
    }
    joined
}

/// The body a switch always runs, if it has one: `Some(Some(body))` for the body,
/// `Some(None)` when there is no arm at all (so the switch never yields), and `None` when the
/// switch still has to dispatch.
fn direct_body(
    core: &emela_core::Module,
    f: &Function,
    on: SwitchOn,
    cases: &[Case],
    default: &Option<SeqId>,
) -> Option<Option<SeqId>> {
    let all_same = match cases.first().map(|case| case.body) {
        Some(first) => {
            cases.iter().all(|case| same_body(f, first, case.body))
                && default.is_none_or(|d| same_body(f, first, d))
        }
        None => true,
    };
    if let Some(first) = cases.first().map(|case| case.body) {
        if all_same {
            return Some(Some(first));
        }
    } else {
        return Some(*default);
    }
    // A tag switch whose single case covers every constructor takes the case whatever the tag is,
    // so its default (if the compiler emitted one) cannot run.
    if cases.len() == 1 && variants_covered(core, f, on, &cases[0].keys) {
        return Some(Some(cases[0].body));
    }
    None
}

/// Whether `keys` are exactly the variants of the enum the tag switch scrutinises.
fn variants_covered(
    core: &emela_core::Module,
    f: &Function,
    on: SwitchOn,
    keys: &[CaseKey],
) -> bool {
    let SwitchOn::Tag(v) = on else {
        return false;
    };
    let Type::Enum(id, _) = core.ty(f.vars[v].ty) else {
        return false;
    };
    let count = core.enums[*id].variants.len();
    keys.len() == count
        && keys
            .iter()
            .all(|key| matches!(key, CaseKey::Variant(k) if *k < count))
}

/// Whether two sequences are the same code: the same statements with the same operands and
/// variables, the same landings and suspension edges, the same nested sequences, and the same
/// terminator.
fn same_body(f: &Function, a: SeqId, b: SeqId) -> bool {
    same_seq(f, a, b, &mut FxHashSet::default())
}

fn same_seq(f: &Function, a: SeqId, b: SeqId, seen: &mut FxHashSet<(SeqId, SeqId)>) -> bool {
    if a == b {
        return true;
    }
    if !seen.insert((a, b)) {
        // Already being compared further up: the trees agree so far.
        return true;
    }
    let (sa, sb) = (&f.seqs[a], &f.seqs[b]);
    if sa.stmts.len() != sb.stmts.len() {
        return false;
    }
    sa.stmts
        .iter()
        .zip(&sb.stmts)
        .all(|(&x, &y)| same_stmt(f, x, y, seen))
        && sa.term == sb.term
}

fn same_stmt(
    f: &Function,
    a: crate::ir::StmtId,
    b: crate::ir::StmtId,
    seen: &mut FxHashSet<(SeqId, SeqId)>,
) -> bool {
    if a == b {
        return true;
    }
    match (&f.stmts[a], &f.stmts[b]) {
        (
            Stmt::Let {
                dst: d1,
                value: v1,
                on_defect: l1,
                on_suspend: s1,
            },
            Stmt::Let {
                dst: d2,
                value: v2,
                on_defect: l2,
                on_suspend: s2,
            },
        ) => {
            d1 == d2
                && v1 == v2
                && s1 == s2
                && match (l1, l2) {
                    (None, None) => true,
                    (Some(x), Some(y)) => same_seq(f, *x, *y, seen),
                    _ => false,
                }
        }
        (
            Stmt::If {
                dst: d1,
                cond: c1,
                then: t1,
                else_: e1,
            },
            Stmt::If {
                dst: d2,
                cond: c2,
                then: t2,
                else_: e2,
            },
        ) => d1 == d2 && c1 == c2 && same_seq(f, *t1, *t2, seen) && same_seq(f, *e1, *e2, seen),
        (
            Stmt::Switch {
                dst: d1,
                on: o1,
                cases: c1,
                default: f1,
            },
            Stmt::Switch {
                dst: d2,
                on: o2,
                cases: c2,
                default: f2,
            },
        ) => {
            d1 == d2
                && o1 == o2
                && c1.len() == c2.len()
                && c1
                    .iter()
                    .zip(c2)
                    .all(|(x, y)| x.keys == y.keys && same_seq(f, x.body, y.body, seen))
                && match (f1, f2) {
                    (None, None) => true,
                    (Some(x), Some(y)) => same_seq(f, *x, *y, seen),
                    _ => false,
                }
        }
        (
            Stmt::Loop {
                dst: d1,
                vars: v1,
                inits: i1,
                body: b1,
            },
            Stmt::Loop {
                dst: d2,
                vars: v2,
                inits: i2,
                body: b2,
            },
        ) => d1 == d2 && v1 == v2 && i1 == i2 && same_seq(f, *b1, *b2, seen),
        (
            Stmt::Block {
                dst: d1,
                label: l1,
                body: b1,
            },
            Stmt::Block {
                dst: d2,
                label: l2,
                body: b2,
            },
        ) => d1 == d2 && l1 == l2 && same_seq(f, *b1, *b2, seen),
        (
            Stmt::Protect {
                dst: d1,
                body: b1,
                cleanup: c1,
            },
            Stmt::Protect {
                dst: d2,
                body: b2,
                cleanup: c2,
            },
        ) => d1 == d2 && same_seq(f, *b1, *b2, seen) && same_seq(f, *c1, *c2, seen),
        (Stmt::Dup(x), Stmt::Dup(y)) | (Stmt::Drop(x), Stmt::Drop(y)) => x == y,
        _ => false,
    }
}

/// A total order on keys: variant numbers ascending, literal values ascending. A switch holds one
/// kind of key (the verifier checks it), so the order between a variant and a literal is only a
/// tie-break that keeps the pass total.
fn key_order(a: &CaseKey, b: &CaseKey) -> Ordering {
    match (a, b) {
        (CaseKey::Variant(x), CaseKey::Variant(y)) => x.cmp(y),
        (CaseKey::Lit(x), CaseKey::Lit(y)) => const_order(x, y),
        (CaseKey::Variant(_), CaseKey::Lit(_)) => Ordering::Less,
        (CaseKey::Lit(_), CaseKey::Variant(_)) => Ordering::Greater,
    }
}

fn const_order(a: &Const, b: &Const) -> Ordering {
    fn rank(c: &Const) -> u8 {
        match c {
            Const::Int(_) => 0,
            Const::Int64(_) => 1,
            Const::Bool(_) => 2,
            Const::Float(_) => 3,
            Const::String(_) => 4,
            Const::Unit => 5,
        }
    }
    match (a, b) {
        (Const::Int(x), Const::Int(y)) => x.cmp(y),
        (Const::Int64(x), Const::Int64(y)) => x.cmp(y),
        (Const::Bool(x), Const::Bool(y)) => x.cmp(y),
        (Const::Float(x), Const::Float(y)) => x.total_cmp(y),
        (Const::String(x), Const::String(y)) => x.cmp(y),
        _ => rank(a).cmp(&rank(b)),
    }
}

/// The variant keys of a tag switch, every key of every case, in the canonical case order.
///
/// `None` if a case holds a literal key (that is not a tag switch). Backends use this and
/// [`variant_span`] instead of collecting and indexing the keys again.
pub fn variant_keys(cases: &[Case]) -> Option<Vec<usize>> {
    let mut out = Vec::new();
    for case in cases {
        for key in &case.keys {
            match key {
                CaseKey::Variant(k) => out.push(*k),
                CaseKey::Lit(_) => return None,
            }
        }
    }
    Some(out)
}

/// One past the largest variant key of a tag switch (`0` when it has no case): the number of
/// slots a `br_table` covering every key needs. `None` if the switch is not a tag switch.
pub fn variant_span(cases: &[Case]) -> Option<usize> {
    let keys = variant_keys(cases)?;
    Some(keys.iter().max().map_or(0, |m| m.saturating_add(1)))
}

/// The maximal contiguous runs of `keys`, as `(start, length)` in ascending order. One run that
/// starts at 0 and has as many slots as there are keys says the key set is dense (no gaps); more
/// than one run says it is sparse.
pub fn contiguous_runs(keys: &[usize]) -> Vec<(usize, usize)> {
    let mut sorted = keys.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut runs = Vec::new();
    for &key in &sorted {
        match runs.last_mut() {
            Some((start, len)) if *start + *len == key => *len += 1,
            _ => runs.push((key, 1)),
        }
    }
    runs
}

/// Whether the variant keys of a tag switch are a contiguous run starting at 0.
pub fn is_dense(cases: &[Case]) -> Option<bool> {
    let keys = variant_keys(cases)?;
    let runs = contiguous_runs(&keys);
    Some(match keys.len() {
        0 => true,
        n => runs.len() == 1 && runs[0] == (0, n),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Suspend;
    use crate::{FnBuilder, Value, dump_function, verify};
    use emela_core::{EnumDef, Module as CoreModule, TypeId, VariantDef};

    /// A fresh core with one enum of `variants` unit constructors, and its type.
    fn colors(variants: &[&str]) -> (CoreModule, TypeId) {
        let mut core = CoreModule::new();
        let id = core.add_enum(EnumDef {
            name: "Color".into(),
            params: 0,
            variants: variants.iter().map(|name| VariantDef::unit(name)).collect(),
            error: false,
            external: false,
        });
        let ty = core.enum_type(id, Vec::new());
        (core, ty)
    }

    fn module(core: &CoreModule, f: crate::Function) -> (Module, FnId) {
        let mut m = Module::new(core.clone());
        let id = m.add(f);
        m.stage = Stage::RefCounted;
        (m, id)
    }

    fn yield_unit(b: &mut FnBuilder<'_>) {
        b.term(Term::Yield(None));
    }

    fn dump(m: &Module, id: FnId) -> String {
        dump_function(m, &m.functions[id])
    }

    /// A switch over an enum whose two variants share one arm becomes one case with two keys,
    /// while the third keeps its own body.
    #[test]
    fn cases_with_the_same_body_are_joined() {
        let (core, ty) = colors(&["Red", "Green", "Blue"]);
        let mut b = FnBuilder::new(&core, "f");
        let s = b.param("s", ty);
        let red = b.seq(yield_unit);
        let green = b.seq(|b| {
            b.let_(None, Value::Const(Const::Int(1)));
            b.term(Term::Yield(None));
        });
        let blue = b.seq(yield_unit);
        b.switch(
            None,
            SwitchOn::Tag(s),
            vec![
                (CaseKey::Variant(0), red),
                (CaseKey::Variant(1), green),
                (CaseKey::Variant(2), blue),
            ],
            None,
        );
        b.term(Term::Return(None));
        let (mut m, id) = module(&core, b.finish());

        let stats = run(&mut m);

        assert_eq!(stats.switch_cases_merged, 1, "{stats:?}");
        assert_eq!(stats.switch_direct, 0);
        let text = dump(&m, id);
        assert!(text.contains("case Color.Red, Color.Blue"), "{text}");
        assert!(text.contains("case Color.Green"), "{text}");
        assert!(verify(&m).is_ok(), "the rewritten module does not verify");
    }

    /// A tag switch whose single case covers every constructor has nothing to dispatch on: it
    /// becomes a block with that body.
    #[test]
    fn a_single_case_covering_every_constructor_is_direct() {
        let (core, ty) = colors(&["Only"]);
        let mut b = FnBuilder::new(&core, "f");
        let s = b.param("s", ty);
        let only = b.seq(yield_unit);
        b.switch(
            None,
            SwitchOn::Tag(s),
            vec![(CaseKey::Variant(0), only)],
            None,
        );
        b.term(Term::Return(None));
        let (mut m, id) = module(&core, b.finish());

        let stats = run(&mut m);

        assert_eq!(stats.switch_direct, 1, "{stats:?}");
        let text = dump(&m, id);
        assert!(!text.contains("switch tag"), "{text}");
        assert!(text.contains("block"), "{text}");
        assert!(verify(&m).is_ok(), "the rewritten module does not verify");
    }

    /// A switch whose every arm is the same body runs that body whatever the value is, even when
    /// it is a literal switch with a default.
    #[test]
    fn a_switch_whose_arms_all_agree_is_direct() {
        let core = CoreModule::new();
        let mut b = FnBuilder::new(&core, "f");
        let n = b.param("n", TypeId::INT);
        let case = b.seq(yield_unit);
        let default = b.seq(yield_unit);
        b.switch(
            None,
            SwitchOn::Lit(n),
            vec![(CaseKey::Lit(Const::Int(0)), case)],
            Some(default),
        );
        b.term(Term::Return(None));
        let (mut m, id) = module(&core, b.finish());

        let stats = run(&mut m);

        assert_eq!(stats.switch_direct, 1, "{stats:?}");
        assert!(!dump(&m, id).contains("switch tag"));
        assert!(verify(&m).is_ok());
    }

    /// A tag switch with one case and a default with another body must still dispatch.
    #[test]
    fn a_reachable_default_keeps_the_dispatch() {
        let (core, ty) = colors(&["Red", "Green"]);
        let mut b = FnBuilder::new(&core, "f");
        let s = b.param("s", ty);
        let red = b.seq(yield_unit);
        let default = b.seq(|b| {
            b.let_(None, Value::Const(Const::Int(1)));
            b.term(Term::Yield(None));
        });
        b.switch(
            None,
            SwitchOn::Tag(s),
            vec![(CaseKey::Variant(0), red)],
            Some(default),
        );
        b.term(Term::Return(None));
        let (mut m, id) = module(&core, b.finish());

        let stats = run(&mut m);

        assert_eq!(stats.switch_direct, 0, "{stats:?}");
        assert!(dump(&m, id).contains("switch"));
        assert!(verify(&m).is_ok());
    }

    /// Cases with different bodies are left as they are.
    #[test]
    fn different_bodies_are_not_merged() {
        let (core, ty) = colors(&["Red", "Green"]);
        let mut b = FnBuilder::new(&core, "f");
        let s = b.param("s", ty);
        let red = b.seq(yield_unit);
        let green = b.seq(|b| {
            b.let_(None, Value::Const(Const::Int(1)));
            b.term(Term::Yield(None));
        });
        b.switch(
            None,
            SwitchOn::Tag(s),
            vec![(CaseKey::Variant(0), red), (CaseKey::Variant(1), green)],
            None,
        );
        b.term(Term::Return(None));
        let (mut m, id) = module(&core, b.finish());
        let before = dump(&m, id);

        let stats = run(&mut m);

        assert_eq!(stats.switch_cases_merged, 0, "{stats:?}");
        assert_eq!(stats.switch_direct, 0);
        assert_eq!(dump(&m, id), before);
    }

    /// A duplicated key keeps its first case and loses the later one, so the result verifies.
    #[test]
    fn a_duplicate_key_is_dropped() {
        let (core, ty) = colors(&["Red", "Green"]);
        let mut b = FnBuilder::new(&core, "f");
        let s = b.param("s", ty);
        let first = b.seq(yield_unit);
        let duplicate = b.seq(|b| {
            b.let_(None, Value::Const(Const::Int(1)));
            b.term(Term::Yield(None));
        });
        let green = b.seq(|b| {
            b.let_(None, Value::Const(Const::Int(2)));
            b.term(Term::Yield(None));
        });
        b.switch(
            None,
            SwitchOn::Tag(s),
            vec![
                (CaseKey::Variant(0), first),
                (CaseKey::Variant(0), duplicate),
                (CaseKey::Variant(1), green),
            ],
            None,
        );
        b.term(Term::Return(None));
        let (mut m, id) = module(&core, b.finish());

        let stats = run(&mut m);

        assert_eq!(stats.switch_cases_merged, 1, "{stats:?}");
        let text = dump(&m, id);
        assert_eq!(text.matches("case Color.Red").count(), 1, "{text}");
        assert!(verify(&m).is_ok(), "the rewritten module does not verify");
    }

    /// The keys and their runs describe a dense set and a sparse one.
    #[test]
    fn dense_and_sparse_key_sets_are_recognised() {
        assert_eq!(contiguous_runs(&[0, 1, 2]), vec![(0, 3)]);
        assert_eq!(contiguous_runs(&[0, 2, 3]), vec![(0, 1), (2, 2)]);
        assert_eq!(contiguous_runs(&[]), Vec::new());

        let (core, ty) = colors(&["A", "B", "C", "D"]);
        let mut dense = FnBuilder::new(&core, "dense");
        let s = dense.param("s", ty);
        let bodies: Vec<SeqId> = (0..4).map(|_| dense.seq(yield_unit)).collect();
        dense.switch(
            None,
            SwitchOn::Tag(s),
            bodies
                .into_iter()
                .enumerate()
                .map(|(k, body)| (CaseKey::Variant(k), body))
                .collect(),
            None,
        );
        dense.term(Term::Return(None));

        let mut sparse = FnBuilder::new(&core, "sparse");
        let s = sparse.param("s", ty);
        let a = sparse.seq(yield_unit);
        let c = sparse.seq(|b| {
            b.let_(None, Value::Const(Const::Int(1)));
            b.term(Term::Yield(None));
        });
        let default = sparse.seq(yield_unit);
        sparse.switch(
            None,
            SwitchOn::Tag(s),
            vec![(CaseKey::Variant(0), a), (CaseKey::Variant(2), c)],
            Some(default),
        );
        sparse.term(Term::Return(None));

        let (dense_m, dense_id) = module(&core, dense.finish());
        let (sparse_m, sparse_id) = module(&core, sparse.finish());
        let dense_cases = switch_cases(&dense_m, dense_id);
        let sparse_cases = switch_cases(&sparse_m, sparse_id);

        assert_eq!(variant_keys(&dense_cases), Some(vec![0, 1, 2, 3]));
        assert_eq!(variant_span(&dense_cases), Some(4));
        assert_eq!(is_dense(&dense_cases), Some(true));
        assert_eq!(variant_keys(&sparse_cases), Some(vec![0, 2]));
        assert_eq!(variant_span(&sparse_cases), Some(3));
        assert_eq!(is_dense(&sparse_cases), Some(false));
    }

    fn switch_cases(m: &Module, id: FnId) -> Vec<Case> {
        m.functions[id]
            .stmts
            .iter()
            .find_map(|(_, stmt)| match stmt {
                Stmt::Switch { cases, .. } => Some(cases.clone()),
                _ => None,
            })
            .expect("no switch")
    }

    /// Constants compare by their bits: an arm that returns `-0.0` is not the arm that returns
    /// `0.0`, even though the language's `==` says the two numbers are equal.
    #[test]
    fn signed_zero_bodies_are_different() {
        assert_ne!(Const::Float(-0.0), Const::Float(0.0));
        let core = CoreModule::new();
        let mut b = FnBuilder::new(&core, "f");
        b.set_ret(TypeId::FLOAT);
        let n = b.param("n", TypeId::INT);
        let negative = b.seq(|b| b.term(Term::Return(Some(Const::Float(-0.0).into()))));
        let positive = b.seq(|b| b.term(Term::Return(Some(Const::Float(0.0).into()))));
        b.switch(
            None,
            SwitchOn::Lit(n),
            vec![(CaseKey::Lit(Const::Int(0)), negative)],
            Some(positive),
        );
        b.term(Term::Return(Some(Const::Float(1.0).into())));
        let (mut m, id) = module(&core, b.finish());

        let stats = run(&mut m);

        assert_eq!(stats.switch_direct, 0, "{stats:?}");
        assert_eq!(stats.switch_cases_merged, 0);
        assert!(dump(&m, id).contains("switch"), "{}", dump(&m, id));
        assert!(verify(&m).is_ok());
    }

    /// Bodies that differ in their landing or their suspension edge are not the same body.
    #[test]
    fn different_landings_and_suspensions_are_not_merged() {
        let core = CoreModule::new();
        let mut b = FnBuilder::new(&core, "f");
        let landing_a = b.seq(|b| b.term(Term::Return(None)));
        let landing_a2 = b.seq(|b| b.term(Term::Return(None)));
        let landing_b = b.seq(|b| {
            b.let_(None, Value::Const(Const::Int(2)));
            b.term(Term::Return(None));
        });
        let body_a = b.seq(|b| {
            b.stmt(Stmt::Let {
                dst: None,
                value: Value::Const(Const::Int(1)),
                on_defect: Some(landing_a),
                on_suspend: None,
            });
            b.term(Term::Return(None));
        });
        let body_a2 = b.seq(|b| {
            b.stmt(Stmt::Let {
                dst: None,
                value: Value::Const(Const::Int(1)),
                on_defect: Some(landing_a2),
                on_suspend: None,
            });
            b.term(Term::Return(None));
        });
        let body_b = b.seq(|b| {
            b.stmt(Stmt::Let {
                dst: None,
                value: Value::Const(Const::Int(1)),
                on_defect: Some(landing_b),
                on_suspend: None,
            });
            b.term(Term::Return(None));
        });
        let param = b.param("s", TypeId::INT);
        let body_c = b.seq(|b| {
            b.stmt(Stmt::Let {
                dst: None,
                value: Value::Const(Const::Int(1)),
                on_defect: None,
                on_suspend: Some(Suspend {
                    state: 0,
                    saved: Vec::new(),
                }),
            });
            b.term(Term::Return(None));
        });
        let body_d = b.seq(|b| {
            b.stmt(Stmt::Let {
                dst: None,
                value: Value::Const(Const::Int(1)),
                on_defect: None,
                on_suspend: Some(Suspend {
                    state: 0,
                    saved: vec![param],
                }),
            });
            b.term(Term::Return(None));
        });
        let f = b.finish();

        // The landing sequences are structurally equal but the bodies point at different ones, so
        // the comparison has to follow the edge.
        assert!(same_body(&f, body_a, body_a2));
        assert!(!same_body(&f, body_a, body_b));
        assert!(!same_body(&f, body_c, body_d));
    }

    /// A second run changes nothing.
    #[test]
    fn the_pass_is_idempotent() {
        let (core, ty) = colors(&["Red", "Green", "Blue"]);
        let mut b = FnBuilder::new(&core, "f");
        let s = b.param("s", ty);
        let red = b.seq(yield_unit);
        let green = b.seq(yield_unit);
        let blue = b.seq(|b| {
            b.let_(None, Value::Const(Const::Int(1)));
            b.term(Term::Yield(None));
        });
        b.switch(
            None,
            SwitchOn::Tag(s),
            vec![
                (CaseKey::Variant(0), red),
                (CaseKey::Variant(1), green),
                (CaseKey::Variant(2), blue),
            ],
            None,
        );
        b.term(Term::Return(None));
        let (mut m, id) = module(&core, b.finish());

        let first = run(&mut m);
        assert_eq!(first.switch_cases_merged, 1, "{first:?}");
        let text = dump(&m, id);
        let second = run(&mut m);
        assert_eq!(second.switch_cases_merged, 0, "{second:?}");
        assert_eq!(second.switch_direct, 0);
        assert_eq!(dump(&m, id), text);
    }

    /// With the flag off the stage does not touch the module or its counters.
    #[test]
    fn the_flag_off_leaves_the_stage_alone() {
        let (core, ty) = colors(&["Red", "Green"]);
        let mut b = FnBuilder::new(&core, "f");
        let s = b.param("s", ty);
        let red = b.seq(yield_unit);
        let green = b.seq(yield_unit);
        b.switch(
            None,
            SwitchOn::Tag(s),
            vec![(CaseKey::Variant(0), red), (CaseKey::Variant(1), green)],
            None,
        );
        b.term(Term::Return(None));
        let (mut m, id) = module(&core, b.finish());
        let before = dump(&m, id);

        let stats = crate::opt::run_heap_with(
            &mut m,
            crate::opt::Options {
                rules: crate::opt::LirRules::none(),
            },
        );

        assert_eq!(stats.switch_cases_merged, 0);
        assert_eq!(stats.switch_direct, 0);
        assert_eq!(dump(&m, id), before);
    }
}
