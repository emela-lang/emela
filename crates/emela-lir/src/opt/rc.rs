//! Reference counting cleanup: take back a reference that reference counting gives a value only to
//! give it back.
//!
//! Reference counting ([`crate::heap::rc`]) emits `dup v` before a use that does not consume the
//! last reference, and `drop v` at the last use. Most of those pairs are needed. One shape is not:
//! a variable that reads its value out of something else (a view: `env_get`, `field`,
//! `tuple_get`, …) gets a reference of its own right after the statement that defines it, so that
//! the base may be released before the view's last use, and gives that reference back at the last
//! use. When the base is a borrowed parameter (the environment of a lifted function, or a
//! parameter borrow inference marks borrowed), the value lives in the caller, which keeps it alive
//! for the whole call, so the reference buys nothing: the pairs are pure work in the hot path of
//! every function that reads a capture.
//!
//! The pass removes a `dup v` together with the `drop v` that gives the reference back, when
//!
//! - every use of the variable between the two only borrows it: no consuming use, no second `dup`,
//!   no frame that saves it;
//! - every path from the `dup` reaches a `drop v` before it consumes the variable or leaves for
//!   `recur`, `break`, `return`, `fail` or `unwind`. The walk follows the sequence and descends
//!   into every arm of a compound statement (`if`, `switch`, `block`, `loop`) and into the failure
//!   and defect landings of a statement; an arm that flows back into the sequence makes the walk
//!   go on after the statement, which is the form the counting pass emits when the last use is
//!   inside the arms;
//! - the value cannot die in between: either the variable owns a reference itself (a parameter, a
//!   `let` of a value that is not a view, a loop variable, a compound statement's result), which
//!   the region neither consumes nor drops, or the value lives in a borrowed parameter
//!   ([`Bases::borrows_from_a_borrowed`]).
//!
//! A view of anything else is left alone by default: the reference the `dup` takes may be the only
//! thing keeping its base alive.
//!
//! # A view of a live local base (`LirRules::rc_borrow`)
//!
//! With [`LirRules::rc_borrow`](super::LirRules::rc_borrow) the pass also removes the pair of a
//! counted view whose base chain ends at a local variable, when that root variable keeps the value
//! alive across the whole region:
//!
//! - the statement before the `dup` is the `let` that defines the view (the shape reference
//!   counting emits: a view that is live after its definition gets its reference right after it);
//! - the root owns the only reference to its value at that point (B6, [`ownership`]: the fact after
//!   the view's definition is `Known(true)`), and it is live after it (B4, [`liveness`]);
//! - the region neither takes, adds nor releases the root's reference: no `dup`, `drop`, consuming
//!   use, loop initial value, frame save or redefinition of the root on any path from the `dup` to
//!   the paired `drop`. A view reads its value out of its base, so a root that owns a reference and
//!   keeps it on every path keeps the value alive for the whole region.
//!
//! A call consumes its arguments under the calling convention, so a region that contains a call
//! that consumes the view (or the root) deopts: removing the `dup` would underflow the callee.
//! Call-site borrow elision is not part of this rule; it needs the calling convention to change
//! (C2/PR-026).
//!
//! # Contract
//!
//! ```text
//! Rule ID: rc-cleanup/borrow-local
//! Theory: Perceus reference counting. A view never owns the value it reads; it only needs a
//!         reference of its own because its base may be released before the view's last use.
//!         When the base is uniquely owned and not released in the region, the base's reference
//!         is enough, and the `dup`/`drop` pair is counting work with no keep-alive function.
//! Equation: dup v; …; drop v  ⇒  …            (v a view, base live and unique on every path)
//! Preconditions: the module is at Stage::RefCounted (`insert_rc` has run); the pass runs in
//!         `run_heap`. For a local-base pair (flag on): the analyses answer for the function and
//!         prove the root unique and live; the view's definition is the statement before the dup.
//! Postconditions: the module is still at Stage::RefCounted and passes the verifier: every
//!         remaining `dup` has its `drop` on every path, and the variables of a removed pair are
//!         still used and still owned by what owns them.
//! Preserves: evaluation order, effects, failure landings, defect landings, the evidence tables,
//!         drop points, `Suspend::saved`, and the order and count of the releases of `with` and of
//!         the remaining drops. The `drop` of a pair cannot be the one that frees the value,
//!         because the `dup` proved a reference is held elsewhere; no release (and so no
//!         finalizer or effect a release may run) moves or disappears. What changes is the count a
//!         value carries in the region (one lower), and so memory use, in-place `Array.set`, and
//!         when a program runs out of memory (implementation defined).
//! Deopt: a view whose base is not the borrowed environment unless the local-base proof above
//!         holds; a variable that is consumed (a call, `Array.set`, a container, a terminator) or
//!         duplicated inside the region; the root being dropped, duplicated, consumed or saved by
//!         a frame inside the region; a path that leaves the sequence without giving the reference
//!         back (`recur`, `break`, `return`, `fail`, `unwind`, or the end of the body); a statement
//!         the pass does not model (`protect`, which the Heap IR does not have); and `Unknown` from
//!         B4/B6, which is counted as `Stats::rc_borrow_deopts`.
//! Budget: one pass over the statements of each function, repeated until a round removes nothing;
//!         the two analyses run once per module when the flag is on.
//! Measures: Stats::rc_pairs (all pairs), Stats::rc_borrow_pairs (the local-base pairs),
//!         Stats::rc_borrow_deopts (pairs left alone because B4/B6 answered Unknown).
//! ```

use rustc_hash::{FxHashMap, FxHashSet};

use super::Stats;
use super::analysis::{Fact, liveness, ownership};
use crate::heap::compact;
use crate::heap::rc::{Bases, Mode, counted, value_uses};
use crate::ir::{FnId, Function, Module, Operand, SeqId, Stmt, StmtId, Term, Value, VarId};

/// Removes every `dup`/`drop` pair of every function that the contract allows.
///
/// The functions are visited in arena order and the pairs in statement order, so the result does
/// not depend on the iteration order of a hash map. Removing a pair can expose another one
/// (`dup x; dup x; drop x; drop x`), so a function is cleaned until nothing changes; the loop
/// terminates because every round removes at least one statement. A changed function is rebuilt
/// with `heap::compact`, which drops the removed statements and numbers what is left in the order
/// met.
///
/// `borrow` is [`LirRules::rc_borrow`](super::LirRules::rc_borrow): when it is off, only the pairs
/// the pass has always removed. When it is on, the analyses the local-base proof needs are computed
/// once for the module. A removed `dup`/`drop` pair is two fewer references; it can only remove a
/// "not unique" event, so a fact from the module before the round stays sound (it may be more
/// conservative than the rewritten function).
pub(crate) fn run(m: &mut Module, borrow: bool) -> Stats {
    let mut stats = Stats::default();
    let (live, own) = if borrow {
        (liveness::analyze(m).known(), ownership::analyze(m).known())
    } else {
        (None, None)
    };
    let functions: Vec<FnId> = m.functions.iter().map(|(id, _)| id).collect();
    for id in functions {
        if m.functions[id].body.is_none() {
            continue;
        }
        let ctx = Ctx {
            borrow,
            live: live.as_ref().and_then(|sets| sets.get(&id)),
            own: own.as_ref().and_then(|sets| sets.get(&id)),
        };
        let mut f = m.functions[id].clone();
        let mut removed = 0;
        let mut borrowed = 0;
        // The analysis was computed for the module before this function changed. A candidate that
        // survives a round survives every later round, so the deopts are counted once.
        let mut first = true;
        loop {
            let plan = plan(m, &f, ctx);
            if first {
                stats.rc_borrow_deopts += plan.deopts;
                first = false;
            }
            if plan.count == 0 {
                break;
            }
            plan.apply(&mut f);
            removed += plan.count;
            borrowed += plan.borrowed;
        }
        if removed > 0 {
            m.functions[id] = compact(&f);
            stats.rc_pairs += removed;
            stats.rc_borrow_pairs += borrowed;
        }
    }
    stats
}

/// The `dup`/`drop` pairs of one function.
#[derive(Default)]
struct Plan {
    /// The pairs removed, as the statements to unlink.
    remove: FxHashSet<StmtId>,
    /// Pairs, the counter's contribution.
    count: u32,
    /// The pairs that needed the local-base proof.
    borrowed: u32,
    /// Candidates the local-base proof left alone because B4 or B6 answered `Unknown`.
    deopts: u32,
}

/// What the pass knows about the function it plans.
#[derive(Clone, Copy, Default)]
struct Ctx<'a> {
    /// Whether to elide the reference of a view of a live local base.
    borrow: bool,
    /// The variables live after each `let` of the function (B4).
    live: Option<&'a FxHashMap<StmtId, FxHashSet<VarId>>>,
    /// The ownership facts after each `let` of the function (B6).
    own: Option<&'a FxHashMap<StmtId, ownership::StatementFacts>>,
}

/// The base a local view reads out of, as far as the flag's proof got.
enum Base {
    /// The counted root of the view chain, and its facts are known.
    Root(VarId),
    /// A rule of the proof failed: keep the pair.
    Unsafe,
    /// B4 or B6 has no answer: keep the pair and count a deopt.
    Unknown,
}

impl Plan {
    fn apply(&self, f: &mut Function) {
        for (_, seq) in f.seqs.iter_mut() {
            seq.stmts.retain(|s| !self.remove.contains(s));
        }
    }
}

/// The pairs of `f` that can be removed in one round.
fn plan(m: &Module, f: &Function, ctx: Ctx<'_>) -> Plan {
    let bases = Bases::new(f);
    let mut plan = Plan::default();
    for (seq, s) in f.seqs.iter() {
        for (i, &id) in s.stmts.iter().enumerate() {
            let Stmt::Dup(v) = f.stmts[id] else {
                continue;
            };
            // A view of the closure record, or a variable that keeps its value alive by itself,
            // needs no further proof. A view of anything else needs the local-base proof below.
            let guard = if bases.view_base(v).is_some() && !bases.borrows_from_a_borrowed(v) {
                if !ctx.borrow {
                    continue;
                }
                match local_base(f, &bases, seq, i, v, ctx) {
                    Base::Root(root) => Some(root),
                    Base::Unsafe => continue,
                    Base::Unknown => {
                        plan.deopts += 1;
                        continue;
                    }
                }
            } else {
                None
            };
            let mut drops = Vec::new();
            match region(m, f, seq, i + 1, v, guard, &mut drops) {
                Some(Ends::Closed) => {}
                // A path that reaches the end of the body without giving the reference back, or
                // that leaves through a terminator: not a pair.
                Some(Ends::Open) | None => continue,
            }
            // Two pairs never give the same reference back; a shared `drop` would be a counting
            // pass bug, not something to remove twice.
            if drops.iter().any(|d| plan.remove.contains(d)) {
                continue;
            }
            plan.remove.insert(id);
            plan.remove.extend(drops);
            plan.count += 1;
            if guard.is_some() {
                plan.borrowed += 1;
            }
        }
    }
    plan
}

/// The counted root of the view chain that starts at `v`, when the root keeps the value alive
/// across the region of the `dup` at `i`.
///
/// The chain is `v`'s base, its base, and so on. The root is the first variable that is not a
/// view; a borrowed parameter is not one (a pair that borrows from it is the pass's original
/// case). A view of a root that is not counted, or a `dup` that does not immediately follow the
/// view's definition, is a shape the proof does not cover.
fn local_base(f: &Function, bases: &Bases, seq: SeqId, i: usize, v: VarId, ctx: Ctx<'_>) -> Base {
    let mut root = v;
    let mut found = false;
    for _ in 0..=f.vars.len() {
        match bases.view_base(root) {
            Some(base) => {
                if bases.borrows_from_a_borrowed(root) {
                    return Base::Unsafe;
                }
                root = base;
            }
            None => {
                found = true;
                break;
            }
        }
    }
    if !found || root == v || !counted(f, root) {
        return Base::Unsafe;
    }
    let Some(prev) = i.checked_sub(1).map(|p| f.seqs[seq].stmts[p]) else {
        return Base::Unsafe;
    };
    if !matches!(f.stmts[prev], Stmt::Let { dst: Some(d), .. } if d == v) {
        return Base::Unsafe;
    }
    // B6: the root owns the only reference to its value after the view is defined.
    let Some(own) = ctx.own else {
        return Base::Unknown;
    };
    let Some(facts) = own.get(&prev) else {
        return Base::Unknown;
    };
    match facts.get(&root) {
        Some(Fact::Known(true)) => {}
        Some(Fact::Known(false)) => return Base::Unsafe,
        Some(Fact::Unknown) | None => return Base::Unknown,
    }
    // B4: the root is live where the region starts.
    let Some(live) = ctx.live else {
        return Base::Unknown;
    };
    let Some(after) = live.get(&prev) else {
        return Base::Unknown;
    };
    if !after.contains(&root) {
        return Base::Unsafe;
    }
    Base::Root(root)
}

/// What a statement may do with a variable's references.
enum Action {
    /// Gives a reference back: the end of the region.
    Drop,
    /// Takes or adds a reference (a consuming use, a `dup`, a frame that saves the variable).
    Consume,
    /// Leaves the references as they are.
    Borrow,
}

/// Whether the walk still has paths that must reach the `drop`.
enum Ends {
    /// Every path has reached its `drop`: the region ends before the rest of the sequence, which is
    /// only reachable without the reference being given back.
    Closed,
    /// A path flows on past the statement: the scan continues after it.
    Open,
}

fn action(m: &Module, f: &Function, id: StmtId, v: VarId) -> Action {
    let stmt = &f.stmts[id];
    match stmt {
        Stmt::Dup(x) if *x == v => Action::Consume,
        Stmt::Drop(x) if *x == v => Action::Drop,
        Stmt::Let {
            dst,
            value,
            on_suspend,
            ..
        } => {
            // A variable is defined once, so the region cannot reach its definition; a landing
            // that redefines it is a malformed function and a deopt.
            if *dst == Some(v) {
                return Action::Consume;
            }
            let mut consume = on_suspend.as_ref().is_some_and(|s| s.saved.contains(&v));
            if !consume {
                value_uses(m, f, value, &mut |x, mode| {
                    if x == v && mode == Mode::Consume {
                        consume = true;
                    }
                });
            }
            if consume {
                Action::Consume
            } else {
                Action::Borrow
            }
        }
        Stmt::Loop { inits, .. } => {
            if inits
                .iter()
                .any(|o| matches!(o, Operand::Var(x) if *x == v))
            {
                Action::Consume
            } else {
                Action::Borrow
            }
        }
        // Not in the Heap IR; never pair across one.
        Stmt::Protect { .. } => Action::Consume,
        Stmt::Dup(_)
        | Stmt::Drop(_)
        | Stmt::If { .. }
        | Stmt::Switch { .. }
        | Stmt::Block { .. } => Action::Borrow,
    }
}

/// Whether every path from `from` on in the sequence reaches a `drop v` before it does anything
/// else with the variable's references. The drops that end the region are collected in `drops`.
///
/// `guard` is the root whose reference must keep the value alive: no path may take, add or release
/// it before the region closes. `None` for a variable that keeps its value alive by itself.
fn region(
    m: &Module,
    f: &Function,
    seq: SeqId,
    from: usize,
    v: VarId,
    guard: Option<VarId>,
    drops: &mut Vec<StmtId>,
) -> Option<Ends> {
    for &id in &f.seqs[seq].stmts[from..] {
        if let Some(g) = guard
            && !matches!(action(m, f, id, g), Action::Borrow)
        {
            return None;
        }
        match action(m, f, id, v) {
            Action::Consume => return None,
            Action::Drop => {
                drops.push(id);
                return Some(Ends::Closed);
            }
            Action::Borrow => {
                if let Ends::Closed = statement(m, f, id, v, guard, drops)? {
                    // Every path through the statement ends at one of its own drops; what follows
                    // only runs for paths that gave the reference back.
                    return Some(Ends::Closed);
                }
            }
        }
    }
    match &f.seqs[seq].term {
        // A `yield` continues after the statement that holds this sequence, which is the scan of
        // the enclosing sequence: the path is still open there.
        Term::Yield(value) => {
            if let Some(g) = guard
                && value
                    .as_ref()
                    .is_some_and(|o| matches!(o, Operand::Var(x) if *x == g))
            {
                // The yielded value is consumed: the root no longer owns a reference.
                return None;
            }
            let consumed = value
                .as_ref()
                .is_some_and(|o| matches!(o, Operand::Var(x) if *x == v));
            (!consumed).then_some(Ends::Open)
        }
        // `recur`, `break`, `return`, `fail` and `unwind` leave this sequence without giving the
        // reference back: removing the `dup` would leave that path short one reference.
        // `unreachable` (after a call that does not return) and `defect` (Flow IR only) are not
        // paths to pair either.
        Term::Recur(_)
        | Term::Break { .. }
        | Term::Return(_)
        | Term::Fail(_)
        | Term::Unwind
        | Term::Unreachable
        | Term::Defect(..) => None,
    }
}

/// What the paths inside a statement do with the variable. A `let` continues on its own path after
/// the statement, whatever its landings do; the arms of an `if`, a `switch`, a `block` or a `loop`
/// are its paths, and they must agree (`None` when one of them closes the region and another flows
/// on, which the counting pass never emits: the references of a join point match).
fn statement(
    m: &Module,
    f: &Function,
    id: StmtId,
    v: VarId,
    guard: Option<VarId>,
    drops: &mut Vec<StmtId>,
) -> Option<Ends> {
    let mut seqs: Vec<SeqId> = Vec::new();
    let landing = match &f.stmts[id] {
        Stmt::Let {
            value, on_defect, ..
        } => {
            if let Value::Call {
                on_fail: Some(on_fail),
                ..
            }
            | Value::Fiber {
                on_fail: Some(on_fail),
                ..
            } = value
                && let crate::ir::OnFail::Handle { body, .. } = on_fail
            {
                seqs.push(*body);
            }
            seqs.extend(on_defect.iter().copied());
            true
        }
        Stmt::If { then, else_, .. } => {
            seqs.extend([*then, *else_]);
            false
        }
        Stmt::Switch { cases, default, .. } => {
            seqs.extend(cases.iter().map(|c| c.body));
            seqs.extend(default.iter().copied());
            false
        }
        Stmt::Block { body, .. } | Stmt::Loop { body, .. } => {
            seqs.push(*body);
            false
        }
        Stmt::Protect { .. } => return None,
        Stmt::Dup(_) | Stmt::Drop(_) => return Some(Ends::Open),
    };
    if landing {
        // A landing leaves the function, the enclosing block or a loop, so it must give the
        // reference back before it does: the variable is in its scope with a reference that the
        // `dup` added.
        for s in seqs {
            match region(m, f, s, 0, v, guard, drops)? {
                Ends::Closed => {}
                Ends::Open => return None,
            }
        }
        return Some(Ends::Open);
    }
    if seqs.is_empty() {
        return Some(Ends::Open);
    }
    let mut all_closed = true;
    let mut all_open = true;
    for s in seqs {
        match region(m, f, s, 0, v, guard, drops)? {
            Ends::Closed => all_open = false,
            Ends::Open => all_closed = false,
        }
    }
    if all_closed {
        Some(Ends::Closed)
    } else if all_open {
        Some(Ends::Open)
    } else {
        // One arm closes the region and another flows on: their join point would not carry the
        // same references after the removal, so the counting pass cannot have emitted this.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::FnBuilder;
    use crate::ir::{Const, Stage};
    use crate::verify;
    use emela_core::{FnType, TypeId};

    /// A module with the types the tests use.
    struct Fixture {
        m: Module,
        /// `List[Int]`.
        list: TypeId,
        /// `fn() -> Int`, the type of the closure record a lifted function takes as its
        /// environment.
        thunk: TypeId,
    }

    impl Fixture {
        fn new() -> Self {
            let mut core = emela_core::Module::new();
            let list = core.list(TypeId::INT);
            let thunk = core.func(FnType {
                params: vec![],
                ret: TypeId::INT,
                fails: false,
                suspends: false,
                ev: false,
                sets: None,
            });
            Fixture {
                m: Module::new(core),
                list,
                thunk,
            }
        }

        fn build(&mut self, name: &str, f: impl FnOnce(&mut FnBuilder<'_>)) -> FnId {
            let mut b = FnBuilder::new(&self.m.core, name);
            f(&mut b);
            let function = b.finish();
            self.m.add(function)
        }

        /// Runs the pass at the stage of its precondition and checks the result.
        fn run(&mut self) -> Stats {
            self.run_with(false)
        }

        /// [`Self::run`] with [`LirRules::rc_borrow`](super::super::LirRules::rc_borrow) on or off.
        fn run_with(&mut self, borrow: bool) -> Stats {
            self.m.stage = Stage::RefCounted;
            let stats = super::run(&mut self.m, borrow);
            if let Err(errors) = verify(&self.m) {
                let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
                panic!("rc_cleanup produced invalid IR:\n{}", lines.join("\n"));
            }
            stats
        }

        fn dump(&self, id: FnId) -> String {
            crate::dump_function(&self.m, &self.m.functions[id])
        }

        /// The pairs the pass would remove from `function` (a unit of the analysis, without
        /// needing a module the verifier accepts).
        fn pairs(&self, function: &Function) -> u32 {
            plan(&self.m, function, Ctx::default()).count
        }

        /// The plan of `function` under `ctx`, without applying it.
        fn plan_of(&self, function: &Function, ctx: Ctx<'_>) -> Plan {
            plan(&self.m, function, ctx)
        }
    }

    /// A `dup`/`drop` pair of a variable that owns a reference itself is removed: the reference is
    /// an extra one while the region runs, and nothing in the region consumes it.
    #[test]
    fn a_pair_of_an_owned_variable_is_removed() {
        let mut fx = Fixture::new();
        let list = fx.list;
        let id = fx.build("owned", |b| {
            b.set_ret(TypeId::INT);
            let p = b.param("p", list);
            // `dup p; a = list_head p; drop p; b = list_head p; drop p; return a` — the shape the
            // counting pass emits when a variable is used more than once and given back at the
            // last use.
            b.stmt(Stmt::Dup(p));
            let a = b.assign("a", TypeId::INT, Value::ListHead(p));
            b.stmt(Stmt::Drop(p));
            let _b = b.assign("b", TypeId::INT, Value::ListHead(p));
            b.stmt(Stmt::Drop(p));
            b.term(Term::Return(Some(a.into())));
        });
        assert_eq!(fx.run().rc_pairs, 1);
        let text = fx.dump(id);
        assert_eq!(text.matches("dup").count(), 0, "{text}");
        assert_eq!(text.matches("drop").count(), 1, "{text}");
    }

    /// The same for a view of the borrowed environment parameter: the value lives in the closure
    /// record, which the caller keeps alive for the whole call, so the reference buys nothing.
    #[test]
    fn a_pair_of_a_view_of_the_environment_is_removed() {
        let mut fx = Fixture::new();
        let (list, thunk) = (fx.list, fx.thunk);
        let mut b = FnBuilder::new(&fx.m.core, "lifted");
        b.set_ret(TypeId::INT);
        let env = b.param("env", thunk);
        let d = b.assign("d", list, Value::EnvGet { env, index: 0 });
        b.stmt(Stmt::Dup(d));
        let a = b.assign("a", TypeId::INT, Value::ListHead(d));
        b.stmt(Stmt::Drop(d));
        b.term(Term::Return(Some(a.into())));
        let mut f = b.finish();
        f.env = Some(vec![list]);
        let id = fx.m.add(f);
        assert_eq!(fx.run().rc_pairs, 1);
        let text = fx.dump(id);
        assert_eq!(text.matches("dup").count(), 0, "{text}");
        assert_eq!(text.matches("drop").count(), 0, "{text}");
    }

    /// A view of anything else is left alone: the reference the `dup` takes may be the only thing
    /// keeping its base alive.
    #[test]
    fn a_pair_of_a_view_of_a_local_is_left_alone() {
        let mut fx = Fixture::new();
        let list = fx.list;
        let id = fx.build("local", |b| {
            b.set_ret(TypeId::INT);
            let p = b.param("p", list);
            let t = b.assign("t", list, Value::ListTail(p));
            b.stmt(Stmt::Dup(t));
            b.stmt(Stmt::Drop(p));
            let h = b.assign("h", TypeId::INT, Value::ListHead(t));
            b.stmt(Stmt::Drop(t));
            b.term(Term::Return(Some(h.into())));
        });
        let f = fx.m.functions[id].clone();
        assert_eq!(fx.pairs(&f), 0, "a view of a local lost its reference");
        assert_eq!(fx.run().rc_pairs, 0);
        let text = fx.dump(id);
        assert!(text.contains("dup"), "{text}");
    }

    /// A consuming use inside the region needs the reference: not a pair.
    #[test]
    fn a_consuming_use_inside_the_region_is_left_alone() {
        let mut fx = Fixture::new();
        let list = fx.list;
        let g = fx.build("g", |b| {
            b.set_ret(TypeId::INT);
            let _p = b.param("p", list);
            b.term(Term::Return(Some(Const::Int(0).into())));
        });
        let id = fx.build("consume", |b| {
            b.set_ret(TypeId::INT);
            let p = b.param("p", list);
            b.stmt(Stmt::Dup(p));
            let n = b.assign(
                "n",
                TypeId::INT,
                Value::Call {
                    callee: crate::ir::Callee::Direct(g),
                    args: vec![p.into()],
                    suspends: false,
                    on_fail: None,
                },
            );
            b.stmt(Stmt::Drop(p));
            b.term(Term::Return(Some(n.into())));
        });
        let f = fx.m.functions[id].clone();
        assert_eq!(
            fx.pairs(&f),
            0,
            "a consuming use inside the region was ignored"
        );
    }

    /// The drop must come on every path out of the region: an `if` whose arms disagree is left
    /// alone.
    #[test]
    fn a_drop_on_one_path_only_is_left_alone() {
        let mut fx = Fixture::new();
        let list = fx.list;
        let id = fx.build("branch", |b| {
            b.set_ret(TypeId::INT);
            let p = b.param("p", list);
            let c = b.assign(
                "c",
                TypeId::BOOL,
                Value::Prim {
                    op: crate::ir::PrimOp::Eq,
                    ty: crate::ir::PrimTy::I32,
                    args: vec![Const::Int(1).into(), Const::Int(1).into()],
                },
            );
            b.stmt(Stmt::Dup(p));
            let r = b.var("r", TypeId::INT);
            b.if_(
                Some(r),
                c,
                |b| {
                    b.stmt(Stmt::Drop(p));
                    b.term(Term::Yield(Some(Const::Int(0).into())));
                },
                |b| b.term(Term::Yield(Some(Const::Int(1).into()))),
            );
            b.stmt(Stmt::Drop(p));
            b.term(Term::Return(Some(r.into())));
        });
        let f = fx.m.functions[id].clone();
        assert_eq!(fx.pairs(&f), 0, "a drop on one path only was paired");
    }

    /// A drop in each arm is a pair: the region ends inside the arms, and what follows only runs
    /// for paths that gave the reference back.
    #[test]
    fn a_drop_in_every_arm_is_a_pair() {
        let mut fx = Fixture::new();
        let list = fx.list;
        let id = fx.build("arms", |b| {
            b.set_ret(TypeId::INT);
            let p = b.param("p", list);
            let c = b.assign(
                "c",
                TypeId::BOOL,
                Value::Prim {
                    op: crate::ir::PrimOp::Eq,
                    ty: crate::ir::PrimTy::I32,
                    args: vec![Const::Int(1).into(), Const::Int(1).into()],
                },
            );
            b.stmt(Stmt::Dup(p));
            let a = b.assign("a", TypeId::INT, Value::ListHead(p));
            let r = b.var("r", TypeId::INT);
            b.if_(
                Some(r),
                c,
                |b| {
                    b.stmt(Stmt::Drop(p));
                    b.term(Term::Yield(Some(a.into())));
                },
                |b| {
                    b.stmt(Stmt::Drop(p));
                    b.term(Term::Yield(Some(Const::Int(1).into())));
                },
            );
            b.stmt(Stmt::Drop(p));
            b.term(Term::Return(Some(r.into())));
        });
        assert_eq!(fx.run().rc_pairs, 1);
        let text = fx.dump(id);
        assert_eq!(text.matches("dup").count(), 0, "{text}");
        // The two arm drops and the one before the return became one: the pair's drops and the
        // variable's own last release.
        assert_eq!(text.matches("drop").count(), 1, "{text}");
    }

    /// The releases the pass does not remove stay where they were, in the same order.
    #[test]
    fn other_drops_keep_their_order() {
        let mut fx = Fixture::new();
        let (list, thunk) = (fx.list, fx.thunk);
        let mut b = FnBuilder::new(&fx.m.core, "order");
        b.set_ret(TypeId::INT);
        let env = b.param("env", thunk);
        let d = b.assign("d", list, Value::EnvGet { env, index: 0 });
        let q = b.param("q", list);
        let r = b.param("r", list);
        b.stmt(Stmt::Dup(d));
        let a = b.assign("a", TypeId::INT, Value::ListHead(d));
        b.stmt(Stmt::Drop(d));
        b.stmt(Stmt::Drop(q));
        b.stmt(Stmt::Drop(r));
        b.term(Term::Return(Some(a.into())));
        let mut f = b.finish();
        f.env = Some(vec![list]);
        let id = fx.m.add(f);
        let before = fx.dump(id);
        assert_eq!(fx.run().rc_pairs, 1);
        let after = fx.dump(id);
        // The pair of `d` is gone; the releases of `q` and `r` come in the same order as before,
        // and nothing else moved.
        let order = |text: &str| {
            text.lines()
                .filter(|l| l.trim_start().starts_with("drop"))
                .map(|l| l.trim().split('%').next().unwrap().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(order(&before), ["drop d", "drop q", "drop r"], "{before}");
        assert_eq!(order(&after), ["drop q", "drop r"], "{after}");
    }

    /// A defect landing that drops the reference is part of the region: the `dup` is given back on
    /// that path too, which is the shape the counting pass emits for a view that is live across a
    /// call that may raise a defect.
    #[test]
    fn a_landing_that_drops_the_reference_is_a_pair() {
        let mut fx = Fixture::new();
        let (list, thunk) = (fx.list, fx.thunk);
        let h = fx.build("h", |b| {
            b.set_ret(TypeId::INT);
            b.term(Term::Return(Some(Const::Int(0).into())));
        });
        fx.m.functions[h].defects = true;
        let mut b = FnBuilder::new(&fx.m.core, "landing");
        b.set_ret(TypeId::INT);
        let env = b.param("env", thunk);
        let d = b.assign("d", list, Value::EnvGet { env, index: 0 });
        b.stmt(Stmt::Dup(d));
        let n = b.var("n", TypeId::INT);
        let landing = b.seq(|b| {
            b.stmt(Stmt::Drop(d));
            b.term(Term::Unwind);
        });
        b.stmt(Stmt::Let {
            dst: Some(n),
            value: Value::Call {
                callee: crate::ir::Callee::Direct(h),
                args: vec![],
                suspends: false,
                on_fail: None,
            },
            on_defect: Some(landing),
            on_suspend: None,
        });
        let _a = b.assign("a", TypeId::INT, Value::ListHead(d));
        b.stmt(Stmt::Drop(d));
        b.term(Term::Return(Some(n.into())));
        let mut f = b.finish();
        f.env = Some(vec![list]);
        f.defects = true;
        let id = fx.m.add(f);
        assert_eq!(fx.run().rc_pairs, 1);
        let text = fx.dump(id);
        assert!(!text.contains("dup"), "{text}");
        assert!(!text.contains("drop"), "{text}");
    }

    /// A landing that leaves without giving the reference back keeps the pair: the counting pass
    /// would have dropped the view there too.
    #[test]
    fn a_landing_that_keeps_the_reference_is_left_alone() {
        let mut fx = Fixture::new();
        let (list, thunk) = (fx.list, fx.thunk);
        let h = fx.build("h2", |b| {
            b.set_ret(TypeId::INT);
            b.term(Term::Return(Some(Const::Int(0).into())));
        });
        fx.m.functions[h].defects = true;
        let mut b = FnBuilder::new(&fx.m.core, "kept");
        b.set_ret(TypeId::INT);
        let env = b.param("env", thunk);
        let d = b.assign("d", list, Value::EnvGet { env, index: 0 });
        b.stmt(Stmt::Dup(d));
        let n = b.var("n", TypeId::INT);
        let landing = b.seq(|b| {
            let _s = b.assign("s", TypeId::INT, Value::ListHead(d));
            b.term(Term::Unwind);
        });
        b.stmt(Stmt::Let {
            dst: Some(n),
            value: Value::Call {
                callee: crate::ir::Callee::Direct(h),
                args: vec![],
                suspends: false,
                on_fail: None,
            },
            on_defect: Some(landing),
            on_suspend: None,
        });
        let _a = b.assign("a", TypeId::INT, Value::ListHead(d));
        b.stmt(Stmt::Drop(d));
        b.term(Term::Return(Some(n.into())));
        let mut f = b.finish();
        f.env = Some(vec![list]);
        f.defects = true;
        assert_eq!(plan(&fx.m, &f, Ctx::default()).count, 0);
    }

    /// A frame that may save the environment parameter makes it owned (`heap::rc` dup's it at the
    /// start), so a view read out of it needs a reference of its own again: the pass leaves it
    /// alone, and the verifier keeps checking those borrows.
    #[test]
    fn a_view_of_an_environment_a_frame_saves_is_left_alone() {
        let fx = Fixture::new();
        let (list, thunk) = (fx.list, fx.thunk);
        let mut b = FnBuilder::new(&fx.m.core, "saved");
        b.set_ret(TypeId::INT);
        let env = b.param("env", thunk);
        let d = b.var("d", list);
        b.stmt(Stmt::Let {
            dst: Some(d),
            value: Value::EnvGet { env, index: 0 },
            on_defect: None,
            on_suspend: Some(crate::ir::Suspend {
                state: 0,
                saved: vec![env],
            }),
        });
        b.stmt(Stmt::Dup(d));
        let a = b.assign("a", TypeId::INT, Value::ListHead(d));
        b.stmt(Stmt::Drop(d));
        b.term(Term::Return(Some(a.into())));
        let mut f = b.finish();
        f.suspends = true;
        f.env = Some(vec![list]);
        let bases = Bases::new(&f);
        assert!(
            !bases.borrows_from_a_borrowed(env),
            "the environment is not owned"
        );
        assert!(
            !bases.borrows_from_a_borrowed(d),
            "a view of an owned environment is not borrowed"
        );
        assert_eq!(plan(&fx.m, &f, Ctx::default()).count, 0);
    }

    /// A second run changes nothing.
    #[test]
    fn the_pass_is_idempotent() {
        let mut fx = Fixture::new();
        let list = fx.list;
        fx.build("idem", |b| {
            b.set_ret(TypeId::INT);
            let p = b.param("p", list);
            b.stmt(Stmt::Dup(p));
            let a = b.assign("a", TypeId::INT, Value::ListHead(p));
            b.stmt(Stmt::Drop(p));
            let _b = b.assign("b", TypeId::INT, Value::ListHead(p));
            b.stmt(Stmt::Drop(p));
            b.term(Term::Return(Some(a.into())));
        });
        assert_eq!(fx.run().rc_pairs, 1);
        let after_first = crate::dump(&fx.m);
        assert_eq!(fx.run().rc_pairs, 0);
        assert_eq!(crate::dump(&fx.m), after_first);
    }

    /// A fresh list whose tail is read into a view, used once, with the list used again before its
    /// drop: the view's base is uniquely owned and alive across the region, so the flag removes the
    /// pair. This is the shape reference counting emits for a local base.
    #[test]
    fn a_pair_of_a_view_of_a_live_local_is_removed_with_the_flag() {
        let mut fx = Fixture::new();
        let list = fx.list;
        let id = fx.build("local", |b| {
            b.set_ret(TypeId::INT);
            let root = b.assign(
                "root",
                list,
                Value::List {
                    elems: vec![Const::Int(1).into(), Const::Int(2).into()],
                    tail: None,
                },
            );
            let t = b.assign("t", list, Value::ListTail(root));
            b.stmt(Stmt::Dup(t));
            let h = b.assign("h", TypeId::INT, Value::ListHead(t));
            b.stmt(Stmt::Drop(t));
            let _e = b.assign("e", TypeId::BOOL, Value::ListIsEmpty(root));
            b.stmt(Stmt::Drop(root));
            b.term(Term::Return(Some(h.into())));
        });
        assert_eq!(fx.run_with(false).rc_borrow_pairs, 0);
        assert_eq!(fx.run_with(true).rc_borrow_pairs, 1);
        let text = fx.dump(id);
        assert_eq!(text.matches("dup").count(), 0, "{text}");
        assert_eq!(text.matches("drop").count(), 1, "{text}");
    }

    /// The facts the local-base proof needs may be missing: the pair is left alone and counted as
    /// a deopt, not read as "safe".
    #[test]
    fn an_unknown_ownership_fact_deopts_a_local_view() {
        let mut fx = Fixture::new();
        let list = fx.list;
        let id = fx.build("unknown", |b| {
            b.set_ret(TypeId::INT);
            let root = b.assign(
                "root",
                list,
                Value::List {
                    elems: vec![Const::Int(1).into(), Const::Int(2).into()],
                    tail: None,
                },
            );
            let t = b.assign("t", list, Value::ListTail(root));
            b.stmt(Stmt::Dup(t));
            let h = b.assign("h", TypeId::INT, Value::ListHead(t));
            b.stmt(Stmt::Drop(t));
            let _e = b.assign("e", TypeId::BOOL, Value::ListIsEmpty(root));
            b.stmt(Stmt::Drop(root));
            b.term(Term::Return(Some(h.into())));
        });
        fx.m.stage = Stage::RefCounted;
        let f = fx.m.functions[id].clone();
        let plan = fx.plan_of(
            &f,
            Ctx {
                borrow: true,
                live: None,
                own: None,
            },
        );
        assert_eq!(plan.count, 0, "an unknown fact removed a pair");
        assert_eq!(plan.deopts, 1);
    }
}
