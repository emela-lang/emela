//! CSE: the common subexpression elimination of a straight-line `let` chain.
//!
//! Two occurrences of the same pure computation in one straight-line chain are one computation.
//! The later occurrence reads the local the earlier one is bound to; when the earlier occurrence
//! is a subexpression rather than the whole right-hand side of a `let`, the pass inserts a `let`
//! for it at the earlier point and both occurrences read that local.
//!
//! The rule is deliberately narrow. It numbers only the values of the `let` chain at the head of a
//! body (`let x = v in let y = w in …`), and only the pure subexpressions of those values. It
//! never looks through an `if`, `match`, `loop`, `with`, `escape`, lambda or call, so the earlier
//! occurrence always dominates the later one on the only path it considers and no occurrence is
//! moved across a binder or an edge. Every candidate is re-evaluated only through a local that was
//! already bound, so nothing is evaluated later, earlier (except a pure subexpression hoisted
//! within its own statement), or a second time.
//!
//! # Contract
//!
//! ```text
//! Rule ID: cse
//! Theory: common subexpression elimination by local value numbering over a straight-line let
//!         chain
//! Equation: let x = e in C[let y = e] → let x = e in C[let y = x]
//!           let x = f(e) in C[e] → let t = e in let x = f(t) in C[t]   (first occurrence unbound)
//! Preconditions: e is a computation over literals, variables, constructors, tuples, fields and
//!                pure operators (`Add`, `Sub`, `Mul`, comparisons, `&&`, `||`, `Neg`, `Not`
//!                on Int, Int64, Float and Bool); no operand is a call, builtin, division,
//!                remainder or assertion; `analysis::effect::of_expr_in` gives e a summary with
//!                every "may" fact false (a missing callee summary deopts, it is not read as
//!                pure); e's type has no finalizer-bearing value (the set of `zeta`'s
//!                `has_finalizer`, inspected recursively through tuples, lists, arrays and enum
//!                fields); both occurrences are values of the same straight-line `let` chain,
//!                with no branch, loop, handler, escape, lambda or call between them; `e` fits
//!                the budget.
//! Postconditions: the later occurrence is a read of the local the earlier occurrence was bound
//!                 to (or of an inserted `let` for an unbound earlier subexpression). The earlier
//!                 occurrence evaluates once and every later one reads its result. The module
//!                 stays well typed, and a second run removes nothing.
//! Preserves: evaluation order, effects, failures, suspension points, the order and count of the
//!            releases of `with`, and drop points. A candidate is pure and defect-free, so
//!            removing its second evaluation removes no effect, failure or defect; its type has
//!            no finalizer, so merging its two values does not move a finalizer's last drop. A
//!            hoisted subexpression is pure and moved only within its own statement, before
//!            other expressions that have no effect, so no evaluation order changes observably;
//!            its allocation and deallocation may move, which plan §3.2 allows.
//! Deopt: an occurrence whose summary has any "may" fact set, or which is a call, builtin,
//!        division, remainder, assertion or array access; an occurrence whose type carries a
//!        finalizer (a handler, evidence table, cell, fiber scope, `Any`, or an aggregate that
//!        contains one); an occurrence across an `if`, `match`, `loop`, `with`, `escape`, lambda
//!        or call boundary; an operand whose value is not a local that is unique and immutable;
//!        and a function body over the budget. Each repeated ineligible occurrence after the
//!        first counts in `RuleStats::deopt`.
//! Budget: at most [`MAX_CSE_NODES`] keyable candidate nodes per function body. Going over leaves
//!         the whole body unchanged and counts one deopt; the counters never grow without a
//!         bound.
//! Measures: [`crate::opt::Stats::cse`].
//! ```
//!
//! # Why a straight-line chain is enough
//!
//! Dominance is the whole precondition of CSE: the earlier occurrence must have been evaluated on
//! every path that reaches the later one, and its operands must not have changed. Locals are
//! bound once and immutable (`crate::ir`), and the `let` spine is a single path, so both hold
//! without a dataflow analysis. A branch, loop, handler, escape or lambda introduces another path
//! or another binder, so the pass restarts its numbering there: a chain inside an arm is
//! optimised on its own, but nothing is merged across the arm.
//!
//! # Finalizers
//!
//! A value with a finalizer runs it at its last drop. Two values merged into one drop at one
//! point, so an expression whose type owns a finalizer stays bound, as in ζ and β. The check is
//! recursive through aggregates here, so a constructor holding a handler or an evidence table is
//! a deopt too. A value whose finalizers are only deallocations (a tuple of integers, a string)
//! may be shared: plan §3.2 allows memory use and deallocation to change.
//!
//! # Status
//!
//! A12/PR-041 implements the rule behind `Rules::cse`; it stays off in `Rules::default` until the
//! M5 integration.

use crate::intrinsics::Builtin;
use crate::ir::{
    BinOp, CtorRef, EffectId, Expr, FnId, Lit, Local, Module, OpTy, Type, TypeId, UnOp,
};
use crate::opt::analysis::effect::{self, EffectSummaries};
use crate::opt::{RuleStats, each_body};

use super::subst;

/// The candidate nodes one function body may contain before the rule deopts the whole body.
///
/// The pass only rewrites `let` chains, so this is generous for a real function and still bounds
/// the work per body. It counts every expression the rule can key (a candidate or a call that it
/// refuses), including nested ones.
const MAX_CSE_NODES: u32 = 256;

/// The depth at which the finalizer check of a recursive type gives up and answers "finalizer".
const MAX_TYPE_DEPTH: u32 = 32;

/// Merges pure common subexpressions of each straight-line `let` chain.
///
/// See the module documentation for the contract. The effect summaries are computed once for the
/// module; the pass changes only pure expressions, so they stay valid for the whole run.
pub(in crate::opt) fn cse(m: &mut Module) -> RuleStats {
    let summaries = effect::of_module(m);
    let mut stats = RuleStats::default();
    each_body(m, &mut |m, body| {
        rewrite(m, body, &summaries, &mut stats);
    });
    stats
}

/// Rewrites `e`: a `let` chain at the head is numbered and merged, and every other node is
/// descended into with a fresh numbering.
fn rewrite(m: &mut Module, e: &mut Expr, summaries: &EffectSummaries, stats: &mut RuleStats) {
    if matches!(e, Expr::Let { .. }) {
        rewrite_chain(m, e, summaries, stats);
        return;
    }
    crate::tail::for_each_child_mut(e, &mut |child| rewrite(m, child, summaries, stats));
}

/// The numbers of one straight-line chain: how often each expression occurs and whether it may be
/// reused.
struct Plan {
    entries: Vec<Entry>,
    /// The body has more candidate nodes than [`MAX_CSE_NODES`].
    overflow: bool,
}

struct Entry {
    key: Key,
    /// Occurrences of the key in the chain, in any position.
    count: u32,
    /// Whether every occurrence can be reused (pure, finalizer-free, supported).
    eligible: bool,
}

impl Plan {
    /// Counts the keys of every value of the chain and of its tail, without changing anything.
    fn build(
        m: &mut Module,
        summaries: &EffectSummaries,
        stmts: &[(Local, Expr)],
        tail: &Expr,
    ) -> Self {
        let mut plan = Plan {
            entries: Vec::new(),
            overflow: false,
        };
        let mut nodes = 0;
        for (_, value) in stmts {
            plan.count(m, summaries, value, &mut nodes);
            if plan.overflow {
                break;
            }
        }
        if !plan.overflow {
            plan.count(m, summaries, tail, &mut nodes);
        }
        plan
    }

    /// Adds `e` and its candidate children to the plan.
    fn count(&mut self, m: &mut Module, summaries: &EffectSummaries, e: &Expr, nodes: &mut u32) {
        let Some(key) = key_of(e) else {
            return;
        };
        *nodes += 1;
        if *nodes > MAX_CSE_NODES {
            self.overflow = true;
            return;
        }
        let eligible = is_candidate_node(e) && eligible_tree(m, summaries, e);
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.key == key) {
            entry.count += 1;
            // The same key is the same expression, so the eligibility cannot differ; keep the
            // first answer rather than recomputing it.
        } else {
            self.entries.push(Entry {
                key,
                count: 1,
                eligible,
            });
        }
        if is_candidate_tree(e) {
            e.for_each_child(&mut |child| self.count(m, summaries, child, nodes));
        }
    }

    /// The plan entry of `key`.
    fn entry(&self, key: &Key) -> Option<&Entry> {
        self.entries.iter().find(|entry| &entry.key == key)
    }
}

/// Rewrites one straight-line chain.
///
/// The chain is taken apart, numbered, rewritten in order, and put back together with the `let`s
/// the rewrites inserted. A chain inside a value is handled by [`rewrite`] before the value takes
/// part in this chain.
fn rewrite_chain(m: &mut Module, e: &mut Expr, summaries: &EffectSummaries, stats: &mut RuleStats) {
    let mut stmts: Vec<(Local, Expr)> = Vec::new();
    let mut current = std::mem::replace(e, Expr::Lit(Lit::Unit));
    while let Expr::Let { var, value, body } = current {
        let mut value = *value;
        rewrite(m, &mut value, summaries, stats);
        stmts.push((var, value));
        current = *body;
    }
    let mut tail = current;
    rewrite(m, &mut tail, summaries, stats);

    let plan = Plan::build(m, summaries, &stmts, &tail);
    if plan.overflow {
        // The chain was not changed; put the parts back and report the refused body once.
        *e = rebuild(stmts, Vec::new(), tail);
        stats.deopt += 1;
        return;
    }

    let mut env: Vec<(Key, Local)> = Vec::new();
    let mut seen_blocked: Vec<Key> = Vec::new();
    let mut inserted: Vec<Vec<(Local, Expr)>> = Vec::with_capacity(stmts.len());
    for (var, value) in stmts.iter_mut() {
        let mut before = Vec::new();
        visit_value(
            m,
            value,
            *var,
            &plan,
            &mut env,
            &mut seen_blocked,
            &mut before,
            stats,
        );
        inserted.push(before);
    }
    // The tail is part of the same straight-line path, so a later occurrence there reads the
    // earlier local too. A `let` inserted for an unbound tail subexpression wraps the tail.
    let mut tail_inserted = Vec::new();
    visit_child(
        m,
        &mut tail,
        &plan,
        &mut env,
        &mut seen_blocked,
        &mut tail_inserted,
        stats,
    );
    for (local, value) in tail_inserted.into_iter().rev() {
        tail = Expr::Let {
            var: local,
            value: Box::new(value),
            body: Box::new(tail),
        };
    }
    *e = rebuild(stmts, inserted, tail);
}

/// Rewrites the value of one `let`, considering its root (which the `let` binds) and then its
/// candidate subexpressions (which need an inserted `let` on their first occurrence).
#[allow(clippy::too_many_arguments)]
fn visit_value(
    m: &mut Module,
    value: &mut Expr,
    var: Local,
    plan: &Plan,
    env: &mut Vec<(Key, Local)>,
    seen_blocked: &mut Vec<Key>,
    inserted: &mut Vec<(Local, Expr)>,
    stats: &mut RuleStats,
) {
    let mut register = None;
    if let Some(key) = key_of(value)
        && let Some(entry) = plan.entry(&key)
        && entry.count >= 2
    {
        if entry.eligible {
            if let Some(previous) = lookup(env, &key) {
                *value = Expr::Var(previous);
                stats.applied += 1;
                return;
            }
            register = Some(key);
        } else {
            note_blocked(&key, seen_blocked, stats);
        }
    }
    if is_candidate_tree(value) {
        crate::tail::for_each_child_mut(value, &mut |child| {
            visit_child(m, child, plan, env, seen_blocked, inserted, stats);
        });
    }
    if let Some(key) = register
        && lookup(env, &key).is_none()
    {
        // `let var = value` binds the expression itself; later occurrences read `var`.
        env.push((key, var));
    }
}

/// Rewrites one non-root candidate: a repeated expression becomes a read of the earlier local,
/// and the first occurrence of a repeated expression that no local binds gets a `let` of its own.
fn visit_child(
    m: &mut Module,
    child: &mut Expr,
    plan: &Plan,
    env: &mut Vec<(Key, Local)>,
    seen_blocked: &mut Vec<Key>,
    inserted: &mut Vec<(Local, Expr)>,
    stats: &mut RuleStats,
) {
    if let Some(key) = key_of(child)
        && let Some(entry) = plan.entry(&key)
        && entry.count >= 2
    {
        if entry.eligible {
            if let Some(previous) = lookup(env, &key) {
                *child = Expr::Var(previous);
                stats.applied += 1;
                return;
            }
            // The first occurrence is not bound. Hoist it into a `let` before the statement; the
            // expression is pure and finalizer-free, so only the timing of memory use changes.
            // The value is evaluated once, at the first occurrence.
            let ty = expr_type(m, child);
            let local = subst::fresh(m, "cse", ty);
            let original = child.clone();
            *child = Expr::Var(local);
            inserted.push((local, original));
            env.push((key, local));
            return;
        }
        note_blocked(&key, seen_blocked, stats);
    }
    if is_candidate_tree(child) {
        crate::tail::for_each_child_mut(child, &mut |inner| {
            visit_child(m, inner, plan, env, seen_blocked, inserted, stats);
        });
    }
}

/// Rebuilds a chain with the inserted `let`s in front of each statement.
fn rebuild(
    mut stmts: Vec<(Local, Expr)>,
    mut inserted: Vec<Vec<(Local, Expr)>>,
    mut tail: Expr,
) -> Expr {
    if inserted.is_empty() {
        inserted = vec![Vec::new(); stmts.len()];
    }
    for (i, (var, value)) in stmts.iter_mut().enumerate().rev() {
        tail = Expr::Let {
            var: *var,
            value: Box::new(std::mem::replace(value, Expr::Lit(Lit::Unit))),
            body: Box::new(tail),
        };
        for (local, value) in inserted[i].drain(..).rev() {
            tail = Expr::Let {
                var: local,
                value: Box::new(value),
                body: Box::new(tail),
            };
        }
    }
    tail
}

/// The local `key` is bound to, if any.
fn lookup(env: &[(Key, Local)], key: &Key) -> Option<Local> {
    env.iter()
        .find(|(bound, _)| bound == key)
        .map(|(_, local)| *local)
}

/// Counts a repeated occurrence the plan refused, once for each occurrence after the first.
fn note_blocked(key: &Key, seen: &mut Vec<Key>, stats: &mut RuleStats) {
    if seen.iter().any(|blocked| blocked == key) {
        stats.deopt += 1;
    } else {
        seen.push(key.clone());
    }
}

/// Whether `e` is a computation whose result CSE may reuse.
///
/// A literal, variable or function reference is a leaf, not a computation: it is not keyed as a
/// candidate, though it can be an operand. A call or builtin is never a candidate, whatever the
/// effect summary says; the summary is still read so that an unknown callee is a deopt rather than
/// an assumption.
fn is_candidate_node(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Tuple(_)
            | Expr::Ctor { .. }
            | Expr::Field { .. }
            | Expr::Binary { .. }
            | Expr::Unary { .. }
    ) && is_candidate_tree(e)
}

/// Whether `e` and every computation inside it may be reused as one value.
///
/// A leaf is free. A computation must be pure and finalizer-free, and so must each computation
/// inside it: sharing `(e, 1).1` would still merge the two tuples that hold `e`, even though the
/// field read itself yields a plain `Int`. This is the whole-expression check, not only the type
/// of the result.
fn eligible_tree(m: &mut Module, summaries: &EffectSummaries, e: &Expr) -> bool {
    if matches!(e, Expr::Lit(_) | Expr::Var(_) | Expr::Fn { .. }) {
        return true;
    }
    if !is_candidate_tree(e) {
        return false;
    }
    if !is_candidate_node(e) || !effect::of_expr_in(m, e, summaries).is_pure() {
        return false;
    }
    if !finalizer_free(m, e) {
        return false;
    }
    let mut children_ok = true;
    e.for_each_child(&mut |child| {
        children_ok = children_ok && eligible_tree(m, summaries, child);
    });
    children_ok
}

/// Whether every node of `e` is a supported candidate or a leaf that cannot have an effect.
///
/// This is the boundary the pass will not cross: a call, builtin, assertion, `if`, `match`,
/// lambda or other non-value form makes the whole tree ineligible, and the pass does not key or
/// descend into it as a candidate.
fn is_candidate_tree(e: &Expr) -> bool {
    match e {
        Expr::Lit(_) | Expr::Var(_) | Expr::Fn { .. } => true,
        Expr::Tuple(es) | Expr::Ctor { args: es, .. } => es.iter().all(is_candidate_tree),
        Expr::Field { base, .. } => is_candidate_tree(base),
        Expr::Binary { op, ty, lhs, rhs } => {
            is_pure_operator(*op, *ty) && is_candidate_tree(lhs) && is_candidate_tree(rhs)
        }
        Expr::Unary { ty, operand, .. } => {
            !matches!(ty, OpTy::Structural | OpTy::String) && is_candidate_tree(operand)
        }
        _ => false,
    }
}

/// Whether an operator and its operand type are total and pure.
///
/// A division or remainder may raise a defect on a zero divisor. `Structural` equality may become
/// a call of an `Eq` impl in monomorphization. A String operation allocates a string whose
/// contents the IR does not bound.
fn is_pure_operator(op: BinOp, ty: OpTy) -> bool {
    !matches!(op, BinOp::Div | BinOp::Rem) && !matches!(ty, OpTy::Structural | OpTy::String)
}

/// A canonical key: two expressions with equal keys are the same expression, with the same
/// operands and the same types.
///
/// Floats are compared by their bits, not by `==`, so `-0.0` and `0.0` (equal as floats, different
/// as values) are different keys. Locals and types are interned, so their IDs compare by value.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Key {
    LitInt(i32),
    LitInt64(i64),
    LitFloat(u64),
    LitBool(bool),
    LitString(String),
    LitUnit,
    Var(Local),
    Fn(FnId, Vec<TypeId>),
    Tuple(Vec<Key>),
    Ctor(CtorRef, Vec<TypeId>, Vec<Key>),
    Field(Box<Key>, CtorRef, usize),
    Binary(BinOp, OpTy, Box<Key>, Box<Key>),
    Unary(UnOp, OpTy, Box<Key>),
    CallDirect(FnId, Vec<TypeId>, Vec<Key>, bool, bool),
    CallIndirect(Box<Key>, Vec<Key>, bool, bool),
    CallOp(Box<Key>, EffectId, usize, Vec<TypeId>, Vec<Key>, bool, bool),
    Builtin(Builtin, Vec<TypeId>, Vec<Key>),
}

/// The canonical key of `e`, if every part of it has one.
fn key_of(e: &Expr) -> Option<Key> {
    match e {
        Expr::Lit(Lit::Int(v)) => Some(Key::LitInt(*v)),
        Expr::Lit(Lit::Int64(v)) => Some(Key::LitInt64(*v)),
        Expr::Lit(Lit::Float(v)) => Some(Key::LitFloat(v.to_bits())),
        Expr::Lit(Lit::Bool(v)) => Some(Key::LitBool(*v)),
        Expr::Lit(Lit::String(v)) => Some(Key::LitString(v.clone())),
        Expr::Lit(Lit::Unit) => Some(Key::LitUnit),
        Expr::Var(local) => Some(Key::Var(*local)),
        Expr::Fn { f, ty_args } => Some(Key::Fn(*f, ty_args.clone())),
        Expr::Tuple(es) => Some(Key::Tuple(keys(es)?)),
        Expr::Ctor {
            ctor,
            ty_args,
            args,
        } => Some(Key::Ctor(*ctor, ty_args.clone(), keys(args)?)),
        Expr::Field { base, ctor, index } => {
            Some(Key::Field(Box::new(key_of(base)?), *ctor, *index))
        }
        Expr::Binary { op, ty, lhs, rhs } => Some(Key::Binary(
            *op,
            *ty,
            Box::new(key_of(lhs)?),
            Box::new(key_of(rhs)?),
        )),
        Expr::Unary { op, ty, operand } => Some(Key::Unary(*op, *ty, Box::new(key_of(operand)?))),
        Expr::Call {
            callee,
            args,
            fails,
            suspends,
        } => {
            let args = keys(args)?;
            let key = match callee {
                crate::ir::Callee::Direct { f, ty_args } => {
                    Key::CallDirect(*f, ty_args.clone(), args, *fails, *suspends)
                }
                crate::ir::Callee::Indirect(callee) => {
                    Key::CallIndirect(Box::new(key_of(callee)?), args, *fails, *suspends)
                }
                crate::ir::Callee::Op {
                    handler,
                    effect,
                    op,
                    ty_args,
                } => Key::CallOp(
                    Box::new(key_of(handler)?),
                    *effect,
                    *op,
                    ty_args.clone(),
                    args,
                    *fails,
                    *suspends,
                ),
            };
            Some(key)
        }
        Expr::Builtin { op, ty_args, args } => {
            Some(Key::Builtin(*op, ty_args.clone(), keys(args)?))
        }
        _ => None,
    }
}

/// The keys of `es`, or `None` if one of them has none.
fn keys(es: &[Expr]) -> Option<Vec<Key>> {
    es.iter().map(key_of).collect()
}

/// Whether a value of `ty` owns a finalizer, or an untracked representation.
///
/// The set is ζ's [`has_finalizer`](super::zeta), inspected recursively through the type's
/// aggregates: a constructor or tuple that contains a handler or an evidence table is a deopt as
/// well. A type parameter that cannot be resolved is conservatively finalizer-bearing, and so is
/// a type deeper than [`MAX_TYPE_DEPTH`].
fn type_has_finalizer(m: &Module, ty: TypeId) -> bool {
    framed_has_finalizer(m, ty, &[], 0)
}

fn framed_has_finalizer(m: &Module, ty: TypeId, frames: &[&[TypeId]], depth: u32) -> bool {
    if depth > MAX_TYPE_DEPTH {
        return true;
    }
    let ty = resolve_param(m, ty, frames);
    match m.ty(ty) {
        Type::Handler(_) | Type::Evidence | Type::Cell(_) | Type::FiberScope | Type::Any => true,
        // An unresolved parameter has no known representation.
        Type::Param(_) => true,
        Type::Tuple(ts) => ts
            .iter()
            .any(|&ty| framed_has_finalizer(m, ty, frames, depth + 1)),
        Type::List(ty) | Type::Array(ty) => framed_has_finalizer(m, *ty, frames, depth + 1),
        Type::Enum(id, args) => {
            let mut inner: Vec<&[TypeId]> = frames.to_vec();
            inner.push(args.as_slice());
            m.enums[*id].variants.iter().any(|variant| {
                variant
                    .tys
                    .iter()
                    .any(|&ty| framed_has_finalizer(m, ty, &inner, depth + 1))
            })
        }
        _ => false,
    }
}

/// Resolves the type parameters of `ty` through the innermost frame first, then outward.
fn resolve_param(m: &Module, ty: TypeId, frames: &[&[TypeId]]) -> TypeId {
    let mut ty = ty;
    let mut frames = frames;
    for _ in 0..MAX_TYPE_DEPTH {
        let Type::Param(index) = m.ty(ty) else {
            return ty;
        };
        let Some((frame, rest)) = frames.split_last() else {
            return ty;
        };
        let Some(&next) = frame.get(*index) else {
            return ty;
        };
        ty = next;
        frames = rest;
    }
    ty
}

/// Whether the type of `e` has no finalizer-bearing value.
fn finalizer_free(m: &mut Module, e: &Expr) -> bool {
    let ty = expr_type(m, e);
    !type_has_finalizer(m, ty)
}

/// The type of `e`, keeping the types the typer had to intern.
fn expr_type(m: &mut Module, e: &Expr) -> TypeId {
    let mut typer = crate::Typer::new(m);
    let ty = typer.type_of(e);
    if let Some(types) = typer.into_types() {
        m.types = types;
    }
    ty
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int(v: i32) -> Expr {
        Expr::Lit(Lit::Int(v))
    }

    /// Floats are keyed by bits: `0.0` and `-0.0` are equal as floats but are different values,
    /// and NaN is not equal to itself.
    #[test]
    fn float_keys_compare_bits() {
        let zero = Expr::Lit(Lit::Float(0.0));
        let negative_zero = Expr::Lit(Lit::Float(-0.0));
        let nan = Expr::Lit(Lit::Float(f64::NAN));
        assert_ne!(key_of(&zero), key_of(&negative_zero));
        assert_eq!(key_of(&nan), key_of(&nan));
    }

    /// A division is not a pure operator: it may raise a defect, so neither it nor a tree that
    /// contains it is a candidate.
    #[test]
    fn a_division_is_not_a_candidate() {
        let division = Expr::Binary {
            op: BinOp::Div,
            ty: OpTy::Int,
            lhs: Box::new(int(1)),
            rhs: Box::new(int(0)),
        };
        assert!(!is_candidate_tree(&division));
        assert!(!is_candidate_node(&division));
        let tuple = Expr::Tuple(vec![division]);
        assert!(!is_candidate_tree(&tuple));
    }

    /// A call is not a candidate even when its arguments are pure, but it still has a key, so a
    /// repeated call is a deopt rather than an unnoticed miss.
    #[test]
    fn a_call_is_keyed_but_not_a_candidate() {
        let call = Expr::Call {
            callee: crate::ir::Callee::Indirect(Box::new(Expr::Fn {
                f: crate::ir::Module::new().declare("f", false),
                ty_args: Vec::new(),
            })),
            args: vec![int(1)],
            fails: false,
            suspends: false,
        };
        assert!(key_of(&call).is_some());
        assert!(!is_candidate_node(&call));
    }

    /// A pure tuple of integers is a candidate; a tuple holding a handler is not finalizer-free.
    #[test]
    fn finalizer_free_types() {
        let mut m = Module::new();
        assert!(finalizer_free(&mut m, &Expr::Tuple(vec![int(1), int(2)])));
        let evidence = Expr::Var(m.local("e", Type::Evidence));
        assert!(!finalizer_free(&mut m, &Expr::Tuple(vec![evidence])));
    }
}
