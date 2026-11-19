//! Closure scalar replacement (C5/PR-043, closure half): a closure record that is made and only
//! passed to its own direct calls becomes its captures.
//!
//! Closure conversion makes every function value a record `{function, captures…}` and calls it
//! indirectly. Closure devirtualisation ([`super::devirt`], E5/PR-037) turns a record that is made
//! and only called in the same function into a direct call of the lifted function, with the record
//! as the environment argument. The lifted function reads each capture from that environment
//! (`env_get`). When nothing can observe the record, both the record and the environment argument
//! are dead: the function is known, the captures are read by it and by nothing else, and the record
//! is used only as the environment of those calls.
//!
//! This pass removes the record and makes the captures direct: for a record `r = make_closure
//! F[captures]` every call `call F(r, args)` becomes `call F'(captures, args)`, where `F'` is a copy
//! of `F` with one parameter per capture in place of the environment. `F'` has no environment
//! (`Function::env` is `None`) and its body starts where the `env_get` reads did, reading the
//! parameters directly. The original `F` is left in the module unchanged.
//!
//! The rule is deliberately narrow. Only scalar captures (`i32`, `i64`, `f64`) are replaced, the
//! lifted function must be infallible, non-suspending and free of defects, and the function that
//! makes the record must not suspend. A reference capture
//! would move an owned reference from the record to the caller's argument: the record consumes its
//! captures when it is made, and the lifted function reads them from a borrowed environment; with
//! the record gone, the captures stay owned by the caller and are consumed by the calls, and borrow
//! inference decides later whether the callee borrows them. That moves a release from the record's
//! drop point to the end of the callee's body, which needs its own ownership proof (a type with a
//! finalizer can observe it). A failing or suspending call would likewise have to account for the
//! captures on every way the call comes back and for the variables its frames save. This slice
//! therefore changes no reference count, no drop point and no frame: it removes an allocation and a
//! pure read, and nothing else.
//!
//! [`super::sra`] is the record and tuple half of C5/PR-043.
//!
//! # Contract
//!
//! ```text
//! Rule ID: SRA closure (C5/PR-043)
//! Theory: scalar replacement of a closure record behind the use shape that closure
//!         devirtualisation leaves, with the lifted function specialised on its captures
//! Equation: let r = make_closure F[c1, …, ck]
//!           … call F(r, a1, …, an) …
//!           -> let c1' = c1, …, ck' = ck in      (the variables stay where they are)
//!              … call F'(c1, …, ck, a1, …, an) …
//!           where F' = F with the environment parameter replaced by k capture parameters and
//!           every `env_get env#i` read replaced by its parameter; no record is allocated
//! Preconditions: the module is at Stage::ClosuresConverted and its functions were allocated as
//!                closure conversion allocates them (a lifted function after the function that
//!                makes its record); the function that makes the record does not suspend; the
//!                record is defined by a `let` with a destination, without
//!                a defect landing or a suspension edge, and is used only as the first argument
//!                of direct calls of the same lifted function (at least one); the lifted function
//!                has a body, is not external, exported, constant, fallible, suspending or
//!                defect-raising; its environment lists exactly the record's captures with the
//!                same Core types; every capture is `i32`, `i64` or `f64`; every use of its
//!                environment parameter is exactly one plain `env_get env#i` read per capture,
//!                each with a destination and no landing or edge; every call site passes the
//!                record and arguments whose representations are the lifted function's parameters
//!                after the environment, suspends and fails like it (never), has no defect
//!                landing or suspension edge, and gives the lifted function's result
//!                representation
//! Postconditions: the record's construction, the record variable and every `env_get` of the
//!                 environment are gone; each call names a fresh `F'` with the captures first and
//!                 the old arguments after them; `F'` is a new function of the module with the
//!                 same result, parameters after the environment, body, failure and suspension
//!                 flags, and no environment; `F` itself is unchanged; the module is still at
//!                 Stage::ClosuresConverted and verifies
//! Preserves: evaluation order and effects (a `make_closure` and an `env_get` are pure reads, and
//!            the capture variables are read, never moved); failure, defect and suspension edges
//!            (there are none on a candidate, and neither function touched suspends, so no frame
//!            changes); the evidence tables (an environment table is a reference and deopts);
//!            drop points and reference counts (a capture is a scalar, so neither the record nor
//!            its removal owns or releases a reference); stdout, stderr, the exit code and test
//!            results. The value a call reads is the value the record was made with: a variable
//!            is bound once and a loop only rebinds its variables at `recur`, which leaves the
//!            sequence that defines the record.
//! Deopt: the record of
//!        - a function that may suspend (a frame could save the record or its captures);
//!        - any use other than the first argument of a direct call of its function, including an
//!          indirect call (the pass runs after E5), a second position, a capture, a container, a
//!          terminator, a `dup`/`drop`, a view or a `coerce`;
//!        - a `let` without a destination, with a defect landing, or with a suspension edge;
//!        - no use at all;
//!        and a lifted function that
//!        - has no body, is external, is exported, or is a constant;
//!        - fails, suspends, or may raise a defect;
//!        - has an environment parameter used anywhere but one `env_get` per capture (or an
//!          `env_get` without a destination or with a landing);
//!        - captures a reference (`ref`), whatever its type (a function, `String`, `List`,
//!          `Any`, a handler, an evidence table, a cell, a tuple or a record);
//!        - has a call site whose arguments, flags or result representation do not fit it.
//!        Nothing is guessed: a record that fails one of these is left exactly as it was.
//! Budget: one use scan of each function, one environment scan of the target per candidate, and
//!         one clone per candidate; O(statements) each. The functions are visited in reverse arena
//!         order, so a lifted function (allocated after the function that makes its record) is
//!         already rewritten when a clone copies it; a clone therefore contains no candidate, and
//!         a second run finds nothing. There is no fixpoint and no re-run. The number of rewrites
//!         is bounded by the number of `make_closure` statements, and each rewrite removes one of
//!         them.
//! Measures: [`Stats::sra_closures`] (records replaced) and [`Stats::sra_closure_deopts`]
//!           (records left alone, one per `make_closure` bound by a `let`).
//! ```
//!
//! The pass does not consult the escape analysis: the use shape already proves that the record
//! stays in its function. It could not use the intraprocedural set anyway, because after E5 the
//! record is an argument of a direct call and the set marks every call argument as escaping.

use rustc_hash::{FxHashMap, FxHashSet};

use super::Stats;
use crate::heap::{Copier, compact};
use crate::ir::{
    Callee, FnId, Function, Module, Operand, Stage, Stmt, StmtId, SwitchOn, Term, Value, VarData,
    VarId,
};
use crate::repr::{Repr, repr_of};
use emela_core::TypeId;

/// Replaces every removable closure record of every function by its captures.
///
/// Preconditions: the module is at [`Stage::ClosuresConverted`] in the pipeline
/// ([`super::run_mid_with`] calls it after [`super::devirt`]). The functions are visited in
/// reverse arena order so that the clones copy bodies this pass has already rewritten.
pub fn run(m: &mut Module) -> Stats {
    assert_eq!(
        m.stage,
        Stage::ClosuresConverted,
        "closure scalar replacement runs after closure conversion and before the state machines"
    );
    let mut stats = Stats::default();
    let functions: Vec<FnId> = m.functions.iter().map(|(id, _)| id).collect();
    for id in functions.into_iter().rev() {
        let f = &m.functions[id];
        if f.body.is_none() {
            continue;
        }
        let uses = uses(f);
        let (candidates, deopts) = plan(m, f, &uses);
        stats.sra_closure_deopts += deopts;
        if candidates.is_empty() {
            continue;
        }
        let clones: Vec<FnId> = candidates.iter().map(|c| clone(m, c)).collect();
        let f = &mut m.functions[id];
        apply(f, &candidates, &clones);
        stats.sra_closures += candidates.len() as u32;
    }
    stats
}

/// A closure record to replace: its definition, its captures, its call sites and the lifted
/// function to specialise.
struct Candidate {
    /// The `let` that makes the record.
    definition: StmtId,
    /// The captured variables, in environment order.
    captures: Vec<VarId>,
    /// The direct calls that take the record as the environment, in statement order.
    sites: Vec<StmtId>,
    /// The lifted function the record names.
    target: FnId,
    /// The Core types of `captures`, from the target's environment.
    capture_types: Vec<TypeId>,
    /// The destination of the `env_get` read of each capture, in environment order.
    reads: Vec<VarId>,
}

/// What the function does with each variable: the direct calls that take it as their first
/// argument, and every other use.
#[derive(Default)]
struct Uses {
    /// `(the let, its callee)` for every direct call `call f(v, …)` whose first argument is the
    /// variable.
    first_args: Vec<(StmtId, FnId)>,
    /// Uses other than a first argument of a direct call: an operand, a capture, a callee, a
    /// handler, a `dup`/`drop`, a switch scrutinee, a terminator, a saved frame slot.
    other: u32,
}

/// Classifies every use in the function.
///
/// Every statement is in some sequence, so walking the sequence arena (and not only the body) sees
/// them all, the failure and defect landings included; the result is conservative for a statement
/// an earlier pass left unreachable.
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
                // A direct call's first argument is the environment of a lifted function. It is
                // recorded as a possible call site; every other argument is an ordinary use.
                let mut start = 0;
                if let Value::Call {
                    callee: Callee::Direct(g),
                    args,
                    ..
                } = value
                    && let Some(Operand::Var(v)) = args.first()
                {
                    entry(uses, *v).first_args.push((id, *g));
                    start = 1;
                }
                match value {
                    Value::Call { callee, args, .. } => {
                        match callee {
                            Callee::Direct(_) => {}
                            Callee::Indirect(v) | Callee::Op { handler: v, .. } => {
                                entry(uses, *v).other += 1;
                            }
                        }
                        for o in &args[start..] {
                            operand(uses, o);
                        }
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

/// The records of the function that may be replaced, and how many records were left alone.
fn plan(m: &Module, f: &Function, uses: &FxHashMap<VarId, Uses>) -> (Vec<Candidate>, u32) {
    let mut candidates = Vec::new();
    let mut deopts = 0;
    for (_, seq) in f.seqs.iter() {
        for &id in &seq.stmts {
            let Stmt::Let {
                dst: Some(record),
                value: Value::MakeClosure { function, captures },
                on_defect,
                on_suspend,
            } = &f.stmts[id]
            else {
                continue;
            };
            if on_defect.is_some() || on_suspend.is_some() {
                deopts += 1;
                continue;
            }
            let Some(used) = uses.get(record) else {
                deopts += 1;
                continue;
            };
            // Every use must be the environment of a direct call of the same lifted function.
            // The creating function must not suspend: the record is live between its creation and
            // its calls, so a suspension point there would save it; after the rewrite the captures
            // would be saved instead, and the frame of the creating function would change. A
            // narrow slice that changes no frame keeps every function it touches non-suspending.
            if f.suspends
                || used.other > 0
                || used.first_args.is_empty()
                || used.first_args.iter().any(|&(_, g)| g != *function)
            {
                deopts += 1;
                continue;
            }
            let Some((capture_types, reads)) = fits_function(m, f, *function, captures) else {
                deopts += 1;
                continue;
            };
            if used
                .first_args
                .iter()
                .any(|&(site, _)| !fits_call(m, f, site, *record, *function))
            {
                deopts += 1;
                continue;
            }
            candidates.push(Candidate {
                definition: id,
                captures: captures.clone(),
                sites: used.first_args.iter().map(|&(site, _)| site).collect(),
                target: *function,
                capture_types,
                reads,
            });
        }
    }
    (candidates, deopts)
}

/// Whether the lifted function `target` can be specialised on the record's `captures`: the
/// environment types, the scalar restriction and the environment reads.
///
/// Answers the environment's Core types and the destination of each `env_get` read, in environment
/// order. A `None` is a deopt.
fn fits_function(
    m: &Module,
    f: &Function,
    target: FnId,
    captures: &[VarId],
) -> Option<(Vec<TypeId>, Vec<VarId>)> {
    let t = &m.functions[target];
    if t.body.is_none()
        || t.external.is_some()
        || t.exported
        || t.constant
        || t.fails
        || t.suspends
        || t.defects
    {
        return None;
    }
    let types = t.env.as_ref()?;
    if types.len() != captures.len() {
        return None;
    }
    for (&ty, &capture) in types.iter().zip(captures) {
        if f.vars[capture].ty != ty {
            return None;
        }
        // A reference capture would move ownership; see the module documentation.
        if !matches!(repr_of(&m.core, ty), Repr::I32 | Repr::I64 | Repr::F64) {
            return None;
        }
    }
    let reads = capture_reads(t)?;
    Some((types.clone(), reads))
}

/// The destination of each `env_get` read of the environment, in environment order, or `None` when
/// the environment is used in any other way.
fn capture_reads(f: &Function) -> Option<Vec<VarId>> {
    let env = *f.params.first()?;
    let types = f.env.as_ref()?;
    let mut reads: Vec<Option<VarId>> = vec![None; types.len()];
    let mut other = false;
    for (_, seq) in f.seqs.iter() {
        for &id in &seq.stmts {
            match &f.stmts[id] {
                Stmt::Let {
                    dst,
                    value,
                    on_defect,
                    on_suspend,
                } => {
                    match value {
                        Value::EnvGet { env: e, index } => {
                            if *e != env {
                                return None;
                            }
                            match (dst, on_defect, on_suspend) {
                                (Some(d), None, None)
                                    if *index < reads.len() && reads[*index].is_none() =>
                                {
                                    reads[*index] = Some(*d);
                                }
                                _ => other = true,
                            }
                        }
                        other_value if value_uses(other_value, env) => other = true,
                        _ => {}
                    }
                    if let Some(suspend) = on_suspend
                        && suspend.saved.contains(&env)
                    {
                        other = true;
                    }
                }
                Stmt::If { cond, .. } => other |= operand_uses(cond, env),
                Stmt::Switch { on, .. } => {
                    let (SwitchOn::Tag(v) | SwitchOn::Lit(v)) = on;
                    other |= *v == env;
                }
                Stmt::Loop { inits, .. } => {
                    other |= inits.iter().any(|o| operand_uses(o, env));
                }
                Stmt::Block { .. } | Stmt::Protect { .. } => {}
                Stmt::Dup(v) | Stmt::Drop(v) => other |= *v == env,
            }
        }
        other |= term_uses(&seq.term, env);
    }
    if other {
        return None;
    }
    let mut out = Vec::with_capacity(types.len());
    for (index, read) in reads.into_iter().enumerate() {
        let read = read?;
        if f.vars[read].ty != types[index] {
            return None;
        }
        out.push(read);
    }
    Some(out)
}

/// Whether the value uses the variable (`EnvGet` included).
fn value_uses(value: &Value, v: VarId) -> bool {
    let mut used = false;
    value.for_each_var(|x| used |= x == v);
    used
}

fn operand_uses(o: &Operand, v: VarId) -> bool {
    matches!(o, Operand::Var(x) if *x == v)
}

fn term_uses(t: &Term, v: VarId) -> bool {
    match t {
        Term::Yield(o) | Term::Return(o) | Term::Break { value: o, .. } => {
            o.as_ref().is_some_and(|o| operand_uses(o, v))
        }
        Term::Fail(o) | Term::Defect(o, _) => operand_uses(o, v),
        Term::Recur(args) => args.iter().any(|o| operand_uses(o, v)),
        Term::Unwind | Term::Unreachable => false,
    }
}

/// Whether the call at `site` passes `record` as the environment of `target` and fits it.
///
/// The record becomes the captures; the call's arguments follow them. The flags and the result must
/// agree with the target, because a direct call is checked against the target's body.
fn fits_call(m: &Module, f: &Function, site: StmtId, record: VarId, target: FnId) -> bool {
    let t = &m.functions[target];
    let Stmt::Let {
        dst,
        value:
            Value::Call {
                callee: Callee::Direct(g),
                args,
                suspends,
                on_fail,
            },
        on_defect,
        on_suspend,
    } = &f.stmts[site]
    else {
        return false;
    };
    if *g != target || *suspends || on_fail.is_some() || on_defect.is_some() || on_suspend.is_some()
    {
        return false;
    }
    if !matches!(args.first(), Some(Operand::Var(v)) if *v == record) {
        return false;
    }
    // The environment comes first, then the arguments the record's function type lists.
    if t.params.len() != args.len() {
        return false;
    }
    let operands = &t.params[1..];
    if operands
        .iter()
        .zip(args.iter().skip(1))
        .any(|(&p, a)| t.vars[p].repr != operand_repr(f, a))
    {
        return false;
    }
    let result = repr_of(&m.core, t.ret);
    dst.is_none_or(|d| f.vars[d].repr == result)
}

/// The representation of an operand, as the verifier checks it.
fn operand_repr(f: &Function, o: &Operand) -> Repr {
    match o {
        Operand::Var(v) => f.vars[*v].repr,
        Operand::Const(c) => c.repr(),
    }
}

/// A copy of the target with the environment parameter replaced by one parameter per capture.
///
/// The `env_get` reads are removed from the body and their destinations are the new parameters, so
/// every use of a read is a use of the parameter.
fn clone(m: &mut Module, candidate: &Candidate) -> FnId {
    let mut src = m.functions[candidate.target].clone();
    let env = src.params[0];
    let reads: FxHashSet<StmtId> = src
        .stmts
        .iter()
        .filter(|(_, stmt)| {
            matches!(
                stmt,
                Stmt::Let {
                    value: Value::EnvGet { env: e, .. },
                    ..
                } if *e == env
            )
        })
        .map(|(id, _)| id)
        .collect();
    for (_, seq) in src.seqs.iter_mut() {
        seq.stmts.retain(|s| !reads.contains(s));
    }

    let name = unique_name(m, &format!("{}$sra", src.name));
    let mut out = Function::new(&name);
    out.ret = src.ret;
    out.fails = src.fails;
    out.suspends = src.suspends;
    out.constant = src.constant;
    out.defects = src.defects;
    out.env = None;
    let mut copier = Copier::new(&src);
    let mut params = Vec::with_capacity(candidate.capture_types.len() + src.params.len() - 1);
    for (index, &ty) in candidate.capture_types.iter().enumerate() {
        let read = candidate.reads[index];
        let param = out.vars.alloc(VarData {
            hint: src.vars[read].hint.clone(),
            repr: repr_of(&m.core, ty),
            ty,
        });
        params.push(param);
        copier.vars.insert(read, param);
    }
    for &param in &src.params[1..] {
        params.push(copier.fresh(&mut out, param));
    }
    out.params = params;
    out.body = src.body.map(|body| copier.seq(&mut out, body));
    m.functions.alloc(out)
}

/// A name not used by another function of the module.
fn unique_name(m: &Module, base: &str) -> String {
    let taken = |n: &str| m.functions.iter().any(|(_, f)| f.name == n);
    if !taken(base) {
        return base.to_owned();
    }
    (1..)
        .map(|i| format!("{base}${i}"))
        .find(|n| !taken(n))
        .expect("a free name")
}

/// Removes the records and rewrites their call sites, then rebuilds the function.
fn apply(f: &mut Function, candidates: &[Candidate], clones: &[FnId]) {
    for (candidate, &clone) in candidates.iter().zip(clones) {
        for &site in &candidate.sites {
            let Stmt::Let {
                value: Value::Call { callee, args, .. },
                ..
            } = &mut f.stmts[site]
            else {
                unreachable!("a closure SRA site is a call");
            };
            *callee = Callee::Direct(clone);
            let rest = args.split_off(1);
            let mut new_args: Vec<Operand> = candidate
                .captures
                .iter()
                .map(|&v| Operand::Var(v))
                .collect();
            new_args.extend(rest);
            *args = new_args;
        }
    }
    let definitions: FxHashSet<StmtId> = candidates.iter().map(|c| c.definition).collect();
    for (_, seq) in f.seqs.iter_mut() {
        seq.stmts.retain(|s| !definitions.contains(s));
    }
    let rebuilt = compact(f);
    *f = rebuilt;
}
