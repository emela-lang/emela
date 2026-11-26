//! Escape analysis, intra- and interprocedural: which values of a function may leave it, or reach
//! a place that the function does not control, and which parameters a function may keep.
//!
//! The scalar replacement of aggregates (SRA) may decompose a tuple, record or closure record
//! only when the value stays inside the function: a value that escapes can be observed after the
//! function's frame is gone (or at a point whose order against the frame is observable), so its
//! allocation cannot be dropped.
//!
//! A value escapes when one of these holds, directly or through an aggregate it is put into:
//!
//! - it is a parameter (it comes from the caller), or its type has a finalizer or an untracked
//!   representation: `Any`, `FiberScope`, a handler value, an evidence table, a cell;
//! - it is stored into a container (`List`, `Array` / `Map` through a builtin, a `Fiber`
//!   operation), a handler or an evidence table, or it is captured by a closure;
//! - it is boxed or unboxed ([`Value::Coerce`](crate::ir::Value::Coerce)), which may cross `Any`;
//! - it is returned ([`Term::Return`](crate::ir::Term::Return)), failed with ([`Term::Fail`](crate::ir::Term::Fail)), or becomes a defect message;
//! - it is passed to a call (direct, indirect or through a handler) or to a builtin, whose body
//!   the analysis does not see; a direct call is included because SRA does not yet inline;
//! - it is live across a suspension point of a suspending function: the state machine moves it
//!   into the heap frame ([`Suspend::saved`](crate::ir::Suspend::saved)).
//!
//! Everything else does not escape: a tuple or record built and consumed in the same function and
//! read only through its fields, tags or a `switch`, and a closure record that is only called.
//!
//! The analysis is deliberately conservative: it never says that a value does not escape when it
//! might. SRA uses it as a deopt condition, so a wrong `true` loses an optimisation while a wrong
//! `false` changes behaviour.
//!
//! # Contract: the escaping variables
//!
//! ```text
//! Analysis ID: AnalysisId::Escape
//! Domain / lattice: for each function, a set of its variables. The top element is `Unknown`; the
//!                   bottom is the empty set.
//! Transfer: the rules above. A `tuple` / `make` keeps the fields that built it, so the fields of
//!           an escaping aggregate escape with it (a fixpoint over the aggregates of the body).
//!           This set does not follow a value through a compound statement's result: SRA deopts on
//!           a terminator use anyway, and the set is pinned to the answer it had before PR-022.
//!           The parameter summary follows the result, because a functional consumer needs it.
//! Soundness: a variable in a `Known` set may escape; the analysis never omits a value that does.
//! Deopt (unknown): a consumer that gets `Unknown` does not decompose or reuse the value. It must
//!                  not read `Unknown` as "does not escape".
//! Consumers: SRA (`crate::opt::sra`, C5/PR-043 for records and closures), reuse (C4/PR-042) and
//!            closure devirtualisation (E5/PR-037).
//! ```
//!
//! This analysis always answers `Known`: every function's body is walked on its own, so a function
//! without a body contributes an empty set. It is the parameter summary that needs the whole call
//! graph and can answer `Unknown`.
//!
//! # Contract: the parameter summary
//!
//! ```text
//! Analysis ID: AnalysisId::ParameterEscape
//! Domain / lattice: for each function, one boolean per parameter, in parameter order. `true`
//!                   means the value passed at that position may escape the callee. `Unknown` is
//!                   the top; the all-`false` vector is the bottom.
//! Transfer: T_f(summaries) walks f's body with the rules above, except that a parameter is not
//!           escaping just because it comes from the caller: summary[f][i] is "the value of
//!           parameter i takes part in an escaping value". A direct call to g makes argument i
//!           escaping when summary[g][i] is true; an indirect call, an operation call and a call
//!           to a function the module does not define (an external host function) make every
//!           argument escaping. The result of a compound statement (`if`, `switch`, `block`,
//!           `loop`, `protect`) contains the values its arms yield, break or recur with, so a
//!           parameter that reaches an escaping result escapes.
//! Soundness: the summary is the least fixed point of T over the call graph. T only adds escape
//!            facts (it never uses "does not escape" to conclude that something does not), so it
//!            is monotone and every intermediate summary is sound; the least fixed point is the
//!            most precise sound one. A parameter of a finalizer or untracked type escapes.
//! Deopt (unknown): a consumer that gets `Unknown` (the module is malformed: a function that is
//!                  neither defined nor external, which `verify` reports as `MissingBody`) treats
//!                  every argument of every call as escaping, or does not apply its rule.
//! Consumers: borrow inference (C2/PR-026), reuse (C4/PR-042) and SRA of records and closures
//!            (C5/PR-043). SRA's tuple replacement does not consult the summary yet: this
//!            analysis does not change any SRA decision (PR-022).
//! ```
//!
//! # Direction of the fixpoint
//!
//! The summary starts from "nothing escapes" and adds parameters until nothing changes (the least
//! fixed point). The transfer function is monotone: a callee's summary is only read to decide
//! that an argument escapes, never to decide that it does not, so every round can only turn a
//! `false` into a `true`. Starting from "everything escapes" and removing a parameter would
//! instead need a proof that no path reaches a sink; this transfer does not produce such a
//! proof. Recursion and mutual recursion are handled by the iteration: a parameter that reaches a
//! sink through a cycle is marked once the cycle has been walked, and a cycle that never observes
//! a value keeps `false` (which is sound: no such value leaves or is stored anywhere).

use emela_core::{Type, TypeId};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::heap::live;
use crate::ir::{
    Callee, FnId, Function, LabelId, Module, OnFail, Operand, RuntimeOp, SeqId, Stmt, StmtId, Term,
    Value, VarId,
};

use super::{Analysis, AnalysisId, Fact};

/// The variables of a function that may escape it.
pub(crate) type Set = FxHashSet<VarId>;

/// The variables of each function that may escape it.
pub type EscapeSets = FxHashMap<FnId, FxHashSet<VarId>>;

/// For each function, the parameters that may escape the callee, in parameter order.
pub type ParameterSummary = FxHashMap<FnId, Vec<bool>>;

/// The escape analysis of the variables of each function (see the module documentation).
pub struct Escape;

impl Analysis for Escape {
    type Output = Fact<EscapeSets>;

    const ID: AnalysisId = AnalysisId::Escape;

    fn analyze(&self, m: &Module) -> Self::Output {
        let mut sets = EscapeSets::default();
        for (id, f) in m.functions.iter() {
            sets.insert(id, escaping(m, f));
        }
        Fact::Known(sets)
    }
}

/// The escaping variables of each function, or `Unknown` (see the module contract).
pub fn analyze(m: &Module) -> Fact<EscapeSets> {
    Escape.analyze(m)
}

/// The parameter summary of the module, or `Unknown` when it is malformed (see the module
/// contract).
pub struct ParameterEscape;

impl Analysis for ParameterEscape {
    type Output = Fact<ParameterSummary>;

    const ID: AnalysisId = AnalysisId::ParameterEscape;

    fn analyze(&self, m: &Module) -> Self::Output {
        parameter_summary(m)
    }
}

/// For each function, which of its parameters may escape it, or `Unknown` (see the module
/// contract).
///
/// The functions are visited in arena order, so the result does not depend on the iteration order
/// of a hash map. A function that is neither defined nor external makes the summary `Unknown`.
pub fn parameter_summary(m: &Module) -> Fact<ParameterSummary> {
    if m.functions
        .iter()
        .any(|(_, f)| f.body.is_none() && f.external.is_none())
    {
        return Fact::Unknown;
    }
    let functions: Vec<FnId> = m.functions.iter().map(|(id, _)| id).collect();
    let mut summaries = ParameterSummary::default();
    for &id in &functions {
        let f = &m.functions[id];
        // A host function keeps what it is given: the analysis does not see its body.
        let all = f.external.is_some();
        summaries.insert(id, vec![all; f.params.len()]);
    }
    let bound = summaries.values().map(Vec::len).sum::<usize>() + 1;
    let mut rounds = 0;
    loop {
        let mut changed = false;
        for &id in &functions {
            let f = &m.functions[id];
            if f.external.is_some() {
                continue;
            }
            let next = summarize(m, f, &summaries);
            if summaries[&id] != next {
                summaries.insert(id, next);
                changed = true;
            }
        }
        if !changed {
            break;
        }
        rounds += 1;
        debug_assert!(rounds <= bound, "the parameter summary is not monotone");
    }
    Fact::Known(summaries)
}

/// One round of the fixpoint: the parameters of `f` that escape, given the current summaries of
/// the functions it calls.
fn summarize(m: &Module, f: &Function, summaries: &ParameterSummary) -> Vec<bool> {
    let escaping = escape_set(m, f, Seed::FromBody, Calls::Summaries(summaries));
    f.params.iter().map(|p| escaping.contains(p)).collect()
}

/// The variables of `f` that may escape it.
///
/// This is the analysis SRA consumes: every parameter escapes because it comes from the caller.
pub(crate) fn escaping(m: &Module, f: &Function) -> Set {
    escape_set(m, f, Seed::EveryParameter, Calls::AllArguments)
}

/// Which parameters are escaping before the body is walked.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Seed {
    /// Every parameter escapes because it comes from the caller (the SRA rule).
    EveryParameter,
    /// A parameter escapes only through what the body does with it (the summary rule).
    FromBody,
}

/// How a call is charged to its arguments.
#[derive(Clone, Copy)]
enum Calls<'a> {
    /// Every argument of every call escapes (the SRA rule: a direct call is included because SRA
    /// does not inline).
    AllArguments,
    /// A direct call uses the callee's summary; an indirect or operation call, and a function the
    /// module does not define, makes every argument escape.
    Summaries(&'a ParameterSummary),
}

fn escape_set(m: &Module, f: &Function, seed: Seed, calls: Calls<'_>) -> Set {
    let follow_results = matches!(calls, Calls::Summaries(_));
    let mut walk = Walk {
        escaping: Set::default(),
        aggregates: Vec::new(),
        calls,
        follow_results,
    };
    if seed == Seed::EveryParameter {
        walk.escaping.extend(f.params.iter().copied());
    }
    for (id, v) in f.vars.iter() {
        if has_finalizer_or_untracked(m, v.ty) {
            walk.escaping.insert(id);
        }
    }
    if let Some(body) = f.body {
        walk.seq(f, body);
    }
    walk.suspension_points(f);
    walk.aggregates_escaping_with_their_fields();
    walk.escaping
}

/// Whether `ty` names a value that SRA must not decompose: a runtime object with a finalizer
/// (`fin`), an untracked `Any`, or a value whose lifetime the language does not keep in a variable.
fn has_finalizer_or_untracked(m: &Module, ty: TypeId) -> bool {
    matches!(
        m.core.ty(ty),
        Type::Any | Type::FiberScope | Type::Handler(_) | Type::Evidence | Type::Cell(_)
    )
}

struct Walk<'a> {
    escaping: Set,
    /// `dst = tuple(..)` / `dst = make(..)`: the fields stay with the aggregate, so they escape
    /// when it does.
    aggregates: Vec<(VarId, Vec<VarId>)>,
    calls: Calls<'a>,
    /// Whether a value given to a compound statement's result is followed back to the value the
    /// arm gives it. The summary needs it; SRA's set deliberately does not follow it, because SRA
    /// deopts on a terminator use anyway, and that set must stay exactly what it was.
    follow_results: bool,
}

impl Walk<'_> {
    fn operands(&mut self, os: &[Operand]) {
        for o in os {
            if let Operand::Var(v) = o {
                self.escaping.insert(*v);
            }
        }
    }

    fn seq(&mut self, f: &Function, id: SeqId) {
        let seq = &f.seqs[id];
        for &s in &seq.stmts {
            self.stmt(f, s);
        }
        self.term(&seq.term);
    }

    fn term(&mut self, t: &Term) {
        match t {
            // Only the ways that leave the function (or become observable) move a value out.
            // `yield`, `break` and `recur` give values to a compound statement or a loop inside
            // the same function, so those values stay local. The summary follows them separately,
            // through the result of the compound statement ([`Walk::results`]).
            Term::Return(v) => {
                if let Some(v) = v {
                    self.operands(std::slice::from_ref(v));
                }
            }
            Term::Fail(v) | Term::Defect(v, _) => {
                self.operands(std::slice::from_ref(v));
            }
            Term::Yield(_)
            | Term::Break { .. }
            | Term::Recur(_)
            | Term::Unwind
            | Term::Unreachable => {}
        }
    }

    fn stmt(&mut self, f: &Function, id: StmtId) {
        match &f.stmts[id] {
            Stmt::Let {
                dst,
                value,
                on_defect,
                on_suspend,
            } => {
                self.value(f, *dst, value);
                if let Some(suspend) = on_suspend {
                    self.escaping.extend(suspend.saved.iter().copied());
                }
                if let Some(landing) = on_defect {
                    self.seq(f, *landing);
                }
            }
            Stmt::If {
                dst, then, else_, ..
            } => {
                // The condition is a scalar (an `i32`); reading it does not move a value.
                self.results(f, *dst, &[*then, *else_]);
                self.seq(f, *then);
                self.seq(f, *else_);
            }
            Stmt::Switch {
                dst,
                cases,
                default,
                ..
            } => {
                // The scrutinee is a tag or a scalar, read in place.
                let mut arms: Vec<SeqId> = cases.iter().map(|case| case.body).collect();
                arms.extend(*default);
                self.results(f, *dst, &arms);
                for case in cases {
                    self.seq(f, case.body);
                }
                if let Some(default) = default {
                    self.seq(f, *default);
                }
            }
            Stmt::Loop {
                dst,
                vars,
                inits,
                body,
            } => {
                // The loop variables are rebound by `recur`; their values stay local unless the
                // loop hands them on.
                if self.follow_results {
                    if let Some(dst) = dst {
                        let mut fields = Vec::new();
                        yielded(f, *body, &mut fields);
                        if !fields.is_empty() {
                            self.aggregates.push((*dst, fields));
                        }
                    }
                    // A loop variable holds its initial value and every value a `recur` gives it,
                    // so an escaping loop variable takes those with it. Every variable is charged
                    // with every initial and recurring value: a superset of the positional
                    // relation, which is all soundness needs.
                    let mut kept = var_operands(inits);
                    recurred_values(f, *body, &mut kept);
                    if !kept.is_empty() {
                        for var in vars {
                            self.aggregates.push((*var, kept.clone()));
                        }
                    }
                }
                self.seq(f, *body);
            }
            Stmt::Block {
                dst, label, body, ..
            } => {
                self.results(f, *dst, &[*body]);
                if self.follow_results && dst.is_some() {
                    let mut fields = Vec::new();
                    break_values(f, *body, *label, &mut fields);
                    if let Some(dst) = dst {
                        self.aggregates.push((*dst, fields));
                    }
                }
                self.seq(f, *body);
            }
            Stmt::Protect {
                dst, body, cleanup, ..
            } => {
                self.results(f, *dst, &[*body]);
                self.seq(f, *body);
                self.seq(f, *cleanup);
            }
            Stmt::Dup(_) | Stmt::Drop(_) => {}
        }
    }

    /// Adds the edges from a compound statement's result to the values its arms give it.
    fn results(&mut self, f: &Function, dst: Option<VarId>, arms: &[SeqId]) {
        let Some(dst) = dst else { return };
        if !self.follow_results {
            return;
        }
        let mut fields = Vec::new();
        for &arm in arms {
            yielded(f, arm, &mut fields);
        }
        if !fields.is_empty() {
            self.aggregates.push((dst, fields));
        }
    }

    fn value(&mut self, f: &Function, dst: Option<VarId>, value: &Value) {
        match value {
            Value::Const(_) | Value::FnRef(_) => {}
            // A primitive operation reads its operands; it neither stores nor returns them.
            Value::Prim { .. } => {}
            Value::Call {
                callee,
                args,
                on_fail,
                ..
            } => {
                self.call(callee, args);
                // A failure landing is part of the function. It cannot occur before
                // `Stage::DefectsExplicit`, where SRA no longer runs, so this does not change the
                // set SRA sees; the summary may be asked for at any stage.
                if let Some(OnFail::Handle { body, .. }) = on_fail {
                    self.seq(f, *body);
                }
            }
            // A builtin may store its arguments (`Array.set`, `Cell.set`, `Map.insert`, …). SRA
            // does not know the shape of each one, so every argument is treated as escaping.
            Value::Builtin { args, .. } => self.operands(args),
            Value::Make { args, .. } | Value::Tuple(args) => {
                if let Some(dst) = dst {
                    let fields = var_operands(args);
                    self.aggregates.push((dst, fields));
                }
            }
            // Reads: the base stays in its variable and does not escape by being read.
            Value::Field { .. }
            | Value::Tag(_)
            | Value::TupleGet { .. }
            | Value::ListIsEmpty(_)
            | Value::ListHead(_)
            | Value::ListTail(_)
            | Value::EvLookup { .. }
            | Value::IsError { .. }
            | Value::AsError { .. }
            | Value::EnvGet { .. } => {}
            // A list is a container: its elements (and tail) are stored in cons cells.
            Value::List { elems, tail } => {
                self.operands(elems);
                if let Some(tail) = tail {
                    self.operands(std::slice::from_ref(tail));
                }
            }
            // A closure body is a function of its own; only its captures leave this function.
            Value::Closure(closure) => {
                self.escaping.extend(closure.captures.iter().copied());
            }
            Value::MakeClosure { captures, .. } => {
                self.escaping.extend(captures.iter().copied());
            }
            // The base of an extension is read; the bindings are stored in the new table.
            Value::EvExtend { base, bindings } => {
                if let Some(base) = base {
                    self.escaping.insert(*base);
                }
                for (_, v) in bindings {
                    self.escaping.insert(*v);
                }
            }
            // The handler keeps its operations.
            Value::Handler { ops, .. } => self.escaping.extend(ops.iter().copied()),
            // A concurrency operation stores its arguments in runtime objects, and its value is a
            // runtime object with a finalizer (`fin`).
            Value::Fiber { args, .. } => {
                self.operands(args);
                if let Some(dst) = dst {
                    self.escaping.insert(dst);
                }
            }
            // A runtime operation takes and returns references the runtime owns (a pending defect,
            // a held value). They are not plain locals. `concat` is the exception: it reads its
            // parts and its value is an ordinary String, which escapes only where any String
            // does.
            Value::Runtime { op, args } => {
                self.operands(args);
                if let Some(dst) = dst
                    && *op != RuntimeOp::Concat
                {
                    self.escaping.insert(dst);
                }
            }
            // Boxing or unboxing: the representation changes, so the value is not a plain local.
            Value::Coerce(v) => {
                self.escaping.insert(*v);
            }
        }
    }

    /// Charges a call to its arguments: all of them, or the ones the callee's summary says may
    /// escape it.
    fn call(&mut self, callee: &Callee, args: &[Operand]) {
        match (self.calls, callee) {
            (Calls::AllArguments, _) => self.operands(args),
            (Calls::Summaries(summary), Callee::Direct(g)) => {
                let params = summary.get(g);
                for (i, o) in args.iter().enumerate() {
                    let escapes = params.is_none_or(|p| p.get(i).copied().unwrap_or(true));
                    if escapes && let Operand::Var(v) = o {
                        self.escaping.insert(*v);
                    }
                }
            }
            (Calls::Summaries(_), Callee::Indirect(_) | Callee::Op { .. }) => {
                self.operands(args);
            }
        }
    }

    /// Marks the values live across a suspension point: the state machine saves them into the
    /// heap frame.
    fn suspension_points(&mut self, f: &Function) {
        if !f.suspends {
            return;
        }
        let live = live::live_on_lets(f);
        for (id, after) in live {
            if may_suspend(&f.stmts[id]) {
                self.escaping.extend(after);
            }
        }
    }

    /// A tuple or record that escapes keeps its fields alive, so the aggregates it holds escape
    /// too. Iterate to a fixpoint for nested aggregates.
    fn aggregates_escaping_with_their_fields(&mut self) {
        loop {
            let mut changed = false;
            for (dst, fields) in &self.aggregates {
                if self.escaping.contains(dst) {
                    for field in fields {
                        changed |= self.escaping.insert(*field);
                    }
                }
            }
            if !changed {
                break;
            }
        }
    }
}

fn var_operands(os: &[Operand]) -> Vec<VarId> {
    os.iter()
        .filter_map(|o| match o {
            Operand::Var(v) => Some(*v),
            Operand::Const(_) => None,
        })
        .collect()
}

/// The variables a sequence gives to the compound statement it belongs to.
fn yielded(f: &Function, seq: SeqId, out: &mut Vec<VarId>) {
    if let Term::Yield(Some(Operand::Var(v))) = f.seqs[seq].term {
        out.push(v);
    }
}

/// The variables a `break` for `label` carries out of a sequence, at any depth below it.
fn break_values(f: &Function, seq: SeqId, label: LabelId, out: &mut Vec<VarId>) {
    let s = &f.seqs[seq];
    if let Term::Break {
        label: target,
        value: Some(Operand::Var(v)),
    } = s.term
        && target == label
    {
        out.push(v);
    }
    for &id in &s.stmts {
        match &f.stmts[id] {
            Stmt::Let {
                value, on_defect, ..
            } => {
                if let Value::Call {
                    on_fail: Some(OnFail::Handle { body, .. }),
                    ..
                } = value
                {
                    break_values(f, *body, label, out);
                }
                if let Some(landing) = on_defect {
                    break_values(f, *landing, label, out);
                }
            }
            Stmt::If { then, else_, .. } => {
                break_values(f, *then, label, out);
                break_values(f, *else_, label, out);
            }
            Stmt::Switch { cases, default, .. } => {
                for case in cases {
                    break_values(f, case.body, label, out);
                }
                if let Some(default) = default {
                    break_values(f, *default, label, out);
                }
            }
            // A `break` leaves the enclosing block, so it may cross these.
            Stmt::Loop { body, .. } | Stmt::Block { body, .. } => {
                break_values(f, *body, label, out);
            }
            Stmt::Protect { body, cleanup, .. } => {
                break_values(f, *body, label, out);
                break_values(f, *cleanup, label, out);
            }
            Stmt::Dup(_) | Stmt::Drop(_) => {}
        }
    }
}

/// The variables a `recur` of the innermost loop carries, at any depth below its body. A `recur`
/// cannot cross a `protect` or a closure, and a nested loop has its own.
fn recurred_values(f: &Function, seq: SeqId, out: &mut Vec<VarId>) {
    let s = &f.seqs[seq];
    if let Term::Recur(args) = &s.term {
        out.extend(args.iter().filter_map(|o| {
            if let Operand::Var(v) = o {
                Some(*v)
            } else {
                None
            }
        }));
    }
    for &id in &s.stmts {
        match &f.stmts[id] {
            Stmt::If { then, else_, .. } => {
                recurred_values(f, *then, out);
                recurred_values(f, *else_, out);
            }
            Stmt::Switch { cases, default, .. } => {
                for case in cases {
                    recurred_values(f, case.body, out);
                }
                if let Some(default) = default {
                    recurred_values(f, *default, out);
                }
            }
            Stmt::Block { body, .. } => recurred_values(f, *body, out),
            Stmt::Loop { .. }
            | Stmt::Protect { .. }
            | Stmt::Let { .. }
            | Stmt::Dup(_)
            | Stmt::Drop(_) => {}
        }
    }
}

/// Whether the value of a `let` may come back suspended (a suspension point of its function).
fn may_suspend(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Let { value, .. } => match value {
            Value::Call { suspends, .. } => *suspends,
            Value::Fiber { op, .. } => op.suspends(),
            _ => false,
        },
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::FnBuilder;
    use crate::ir::{Const, FnId};
    use emela_core::{External, FnType, TypeId};

    fn int(n: i32) -> Operand {
        Operand::Const(Const::Int(n))
    }

    /// A Core module with the definitions the tests use, plus a builder that adds a function.
    struct Fixture {
        m: Module,
        pair: TypeId,
        list_pair: TypeId,
        thunk: TypeId,
    }

    impl Fixture {
        fn new() -> Self {
            let mut core = emela_core::Module::new();
            let pair = core.tuple(vec![TypeId::INT, TypeId::INT]);
            let list_pair = core.list(pair);
            let thunk = core.func(FnType {
                params: vec![],
                ret: TypeId::INT,
                fails: false,
                suspends: false,
                ev: false,
                uses: false,
                sets: None,
            });
            Fixture {
                m: Module::new(core),
                pair,
                list_pair,
                thunk,
            }
        }

        fn build<R>(&mut self, name: &str, f: impl FnOnce(&mut FnBuilder<'_>) -> R) -> (FnId, R) {
            let mut b = FnBuilder::new(&self.m.core, name);
            let r = f(&mut b);
            let function = b.finish();
            (self.m.add(function), r)
        }

        /// Reserves the id of a function that a function built earlier already calls (mutual
        /// recursion), to be defined with [`Self::define`].
        fn declare(&mut self, name: &str) -> FnId {
            self.m.declare(name)
        }

        fn define<R>(&mut self, id: FnId, f: impl FnOnce(&mut FnBuilder<'_>) -> R) -> R {
            let name = self.m.functions[id].name.clone();
            let mut b = FnBuilder::new(&self.m.core, &name);
            let r = f(&mut b);
            self.m.define(id, b.finish());
            r
        }

        fn external(&mut self, name: &str, params: &[TypeId]) -> FnId {
            let mut b = FnBuilder::new(&self.m.core, name);
            for (i, &ty) in params.iter().enumerate() {
                b.param(&format!("p{i}"), ty);
            }
            let external = External {
                bindings: Vec::new(),
            };
            let function = b.finish_external(external);
            self.m.add(function)
        }

        fn summary(&self) -> ParameterSummary {
            parameter_summary(&self.m)
                .known()
                .expect("the fixture module has a summary")
        }

        fn params(&self, id: FnId) -> Vec<bool> {
            self.summary()[&id].clone()
        }
    }

    #[test]
    fn a_returned_parameter_escapes() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let (id, _) = fx.build("id", |b| {
            b.set_ret(pair);
            let x = b.param("x", pair);
            b.term(Term::Return(Some(x.into())));
        });
        assert_eq!(fx.params(id), vec![true]);
    }

    #[test]
    fn a_parameter_read_field_by_field_and_returned_as_a_scalar_does_not_escape() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let (id, _) = fx.build("first", |b| {
            b.set_ret(TypeId::INT);
            let x = b.param("x", pair);
            let a = b.assign("a", TypeId::INT, Value::TupleGet { base: x, index: 0 });
            b.term(Term::Return(Some(a.into())));
        });
        assert_eq!(fx.params(id), vec![false]);
    }

    #[test]
    fn a_parameter_stored_in_a_list_escapes() {
        let mut fx = Fixture::new();
        let (pair, list) = (fx.pair, fx.list_pair);
        let (id, _) = fx.build("store", |b| {
            b.set_ret(list);
            let x = b.param("x", pair);
            let xs = b.assign(
                "xs",
                list,
                Value::List {
                    elems: vec![x.into()],
                    tail: None,
                },
            );
            b.term(Term::Return(Some(xs.into())));
        });
        assert_eq!(fx.params(id), vec![true]);
    }

    #[test]
    fn a_parameter_captured_by_a_closure_escapes() {
        let mut fx = Fixture::new();
        let (pair, thunk) = (fx.pair, fx.thunk);
        let (g, ()) = fx.build("g", |b| {
            b.set_ret(TypeId::INT);
            b.term(Term::Return(Some(int(0))));
        });
        let (id, _) = fx.build("capture", |b| {
            let x = b.param("x", pair);
            b.assign(
                "closure",
                thunk,
                Value::MakeClosure {
                    function: g,
                    captures: vec![x],
                },
            );
            b.term(Term::Return(None));
        });
        assert_eq!(fx.params(id), vec![true]);
    }

    #[test]
    fn a_parameter_of_a_finalizer_type_escapes() {
        let mut fx = Fixture::new();
        let (id, _) = fx.build("any", |b| {
            b.param("x", TypeId::ANY);
            b.term(Term::Return(None));
        });
        assert_eq!(fx.params(id), vec![true]);
    }

    #[test]
    fn a_parameter_passed_to_an_escaping_callee_escapes() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let (id, _) = fx.build("id", |b| {
            b.set_ret(pair);
            let x = b.param("x", pair);
            b.term(Term::Return(Some(x.into())));
        });
        let (caller, _) = fx.build("caller", |b| {
            let x = b.param("x", pair);
            b.let_(
                None,
                Value::Call {
                    callee: Callee::Direct(id),
                    args: vec![x.into()],
                    suspends: false,
                    on_fail: None,
                },
            );
            b.term(Term::Return(None));
        });
        assert_eq!(fx.params(caller), vec![true]);
    }

    #[test]
    fn a_parameter_passed_to_a_callee_that_ignores_it_does_not_escape() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let (id, _) = fx.build("ignore", |b| {
            b.set_ret(TypeId::INT);
            b.param("x", pair);
            b.term(Term::Return(Some(int(0))));
        });
        let (caller, _) = fx.build("caller", |b| {
            let x = b.param("x", pair);
            b.let_(
                None,
                Value::Call {
                    callee: Callee::Direct(id),
                    args: vec![x.into()],
                    suspends: false,
                    on_fail: None,
                },
            );
            b.term(Term::Return(None));
        });
        assert_eq!(fx.params(caller), vec![false]);
    }

    #[test]
    fn an_indirect_call_makes_every_argument_escape() {
        let mut fx = Fixture::new();
        let (pair, thunk) = (fx.pair, fx.thunk);
        let (id, _) = fx.build("apply", |b| {
            let callee = b.param("callee", thunk);
            let x = b.param("x", pair);
            b.let_(
                None,
                Value::Call {
                    callee: Callee::Indirect(callee),
                    args: vec![x.into()],
                    suspends: false,
                    on_fail: None,
                },
            );
            b.term(Term::Return(None));
        });
        assert_eq!(fx.params(id), vec![false, true]);
    }

    #[test]
    fn an_external_function_summarises_every_parameter_as_escaping() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let host = fx.external("host", &[pair, pair]);
        assert_eq!(fx.params(host), vec![true, true]);

        let (caller, _) = fx.build("caller", |b| {
            let x = b.param("x", pair);
            b.let_(
                None,
                Value::Call {
                    callee: Callee::Direct(host),
                    args: vec![x.into()],
                    suspends: false,
                    on_fail: None,
                },
            );
            b.term(Term::Return(None));
        });
        assert_eq!(fx.params(caller), vec![true]);
    }

    #[test]
    fn mutual_recursion_reaches_the_sink() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let ping = fx.declare("ping");
        let pong = fx.declare("pong");
        fx.define(ping, |b| {
            b.set_ret(pair);
            let x = b.param("x", pair);
            let r = b.assign(
                "r",
                pair,
                Value::Call {
                    callee: Callee::Direct(pong),
                    args: vec![x.into()],
                    suspends: false,
                    on_fail: None,
                },
            );
            b.term(Term::Return(Some(r.into())));
        });
        fx.define(pong, |b| {
            b.set_ret(pair);
            let y = b.param("y", pair);
            b.term(Term::Return(Some(y.into())));
        });
        assert_eq!(fx.params(ping), vec![true]);
        assert_eq!(fx.params(pong), vec![true]);
    }

    #[test]
    fn a_cycle_that_never_observes_a_value_keeps_it_local() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        // `a` calls `b` and `b` calls `a`; neither stores, returns or suspends with the value.
        let a = fx.declare("a");
        let b_id = fx.declare("b");
        fx.define(a, |b| {
            let x = b.param("x", pair);
            b.let_(
                None,
                Value::Call {
                    callee: Callee::Direct(b_id),
                    args: vec![x.into()],
                    suspends: false,
                    on_fail: None,
                },
            );
            b.term(Term::Return(None));
        });
        fx.define(b_id, |b| {
            let y = b.param("y", pair);
            b.let_(
                None,
                Value::Call {
                    callee: Callee::Direct(a),
                    args: vec![y.into()],
                    suspends: false,
                    on_fail: None,
                },
            );
            b.term(Term::Return(None));
        });
        assert_eq!(fx.params(a), vec![false]);
        assert_eq!(fx.params(b_id), vec![false]);
    }

    #[test]
    fn a_parameter_given_to_a_returned_if_result_escapes() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let (id, _) = fx.build("choose", |b| {
            b.set_ret(pair);
            let x = b.param("x", pair);
            let y = b.var("y", pair);
            b.if_(
                Some(y),
                int(1),
                |b| b.term(Term::Yield(Some(x.into()))),
                |b| b.term(Term::Yield(Some(x.into()))),
            );
            b.term(Term::Return(Some(y.into())));
        });
        assert_eq!(fx.params(id), vec![true]);
    }

    #[test]
    fn a_parameter_broken_out_of_a_returned_block_escapes() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let (id, _) = fx.build("escape", |b| {
            b.set_ret(pair);
            let x = b.param("x", pair);
            let y = b.var("y", pair);
            b.block(Some(y), "done", |b, label| {
                b.term(Term::Break {
                    label,
                    value: Some(x.into()),
                });
            });
            b.term(Term::Return(Some(y.into())));
        });
        assert_eq!(fx.params(id), vec![true]);
    }

    #[test]
    fn a_parameter_carried_out_of_a_returned_loop_escapes() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let (id, _) = fx.build("carry", |b| {
            b.set_ret(pair);
            let x = b.param("x", pair);
            let v = b.var("v", pair);
            let y = b.var("y", pair);
            b.loop_(Some(y), vec![(v, x.into())], |b| {
                b.term(Term::Yield(Some(v.into())));
            });
            b.term(Term::Return(Some(y.into())));
        });
        assert_eq!(fx.params(id), vec![true]);
    }

    #[test]
    fn a_parameter_live_across_a_suspension_escapes() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let (g, ()) = fx.build("suspend_once", |b| {
            b.set_ret(TypeId::INT);
            b.term(Term::Return(Some(int(0))));
        });
        let (id, _) = fx.build("live", |b| {
            b.set_suspends();
            b.set_ret(pair);
            let x = b.param("x", pair);
            b.let_(
                None,
                Value::Call {
                    callee: Callee::Direct(g),
                    args: vec![],
                    suspends: true,
                    on_fail: None,
                },
            );
            b.term(Term::Return(Some(x.into())));
        });
        assert_eq!(fx.params(id), vec![true]);
    }

    #[test]
    fn a_parameter_read_before_a_suspension_does_not_escape() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let (g, ()) = fx.build("suspend_again", |b| {
            b.set_ret(TypeId::INT);
            b.term(Term::Return(Some(int(0))));
        });
        let (id, _) = fx.build("dead", |b| {
            b.set_suspends();
            b.set_ret(TypeId::INT);
            let x = b.param("x", pair);
            let a = b.assign("a", TypeId::INT, Value::TupleGet { base: x, index: 0 });
            b.let_(
                None,
                Value::Call {
                    callee: Callee::Direct(g),
                    args: vec![],
                    suspends: true,
                    on_fail: None,
                },
            );
            b.term(Term::Return(Some(a.into())));
        });
        assert_eq!(fx.params(id), vec![false]);
    }

    #[test]
    fn the_summary_is_deterministic() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        fx.build("id", |b| {
            b.set_ret(pair);
            let x = b.param("x", pair);
            b.term(Term::Return(Some(x.into())));
        });
        let first = fx.summary();
        let second = fx.summary();
        let first: Vec<_> = first.iter().map(|(id, v)| (*id, v.clone())).collect();
        let second: Vec<_> = second.iter().map(|(id, v)| (*id, v.clone())).collect();
        assert_eq!(first, second);
    }

    #[test]
    fn an_empty_module_has_an_empty_summary() {
        let m = Module::new(emela_core::Module::new());
        let summary = parameter_summary(&m)
            .known()
            .expect("an empty module has a summary");
        assert!(summary.is_empty());
    }

    #[test]
    fn a_module_with_a_function_that_is_not_defined_is_unknown() {
        let mut m = Module::new(emela_core::Module::new());
        m.declare("declared_but_never_defined");
        assert!(parameter_summary(&m).is_unknown());
    }

    #[test]
    fn the_escape_set_of_the_api_is_the_one_sra_reads() {
        let mut fx = Fixture::new();
        let pair = fx.pair;
        let (id, t) = fx.build("ret", |b| {
            b.set_ret(pair);
            let t = b.assign("t", pair, Value::Tuple(vec![int(1), int(2)]));
            b.term(Term::Return(Some(t.into())));
            t
        });
        let sets = analyze(&fx.m).known().expect("the analysis answers");
        assert_eq!(sets[&id], escaping(&fx.m, &fx.m.functions[id]));
        assert!(sets[&id].contains(&t));
    }
}
