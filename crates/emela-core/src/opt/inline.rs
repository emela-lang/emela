//! Inlining of small, non-recursive functions.
//!
//! A direct call to a small function is replaced by a copy of its body, with its locals renamed to
//! fresh ones and its type arguments substituted. The copy is what later passes need: the
//! aggregates a function builds (a record, a tuple) and the code that consumes them end up in the
//! same function, where the escape analysis of the LIR can see that they do not escape.
//!
//! What is copied is deliberately narrow. A callee must be small, take few parameters, not fail,
//! not suspend, not be in a cycle of the call graph, and not be one of the bundled collection
//! functions, a trait function (whose impl monomorphization picks), a derived impl or a
//! compiler-generated helper. Its body must not raise or catch a failure, install a handler,
//! extend or look up an evidence table, call an operation, or contain a loop. Everything else is
//! left as a call.
//!
//! # Contract
//!
//! ```text
//! Preconditions: a direct call to a function that is small, non-recursive (no path back to
//!                itself in the call graph), neither failing nor suspending, and whose body has
//!                no Fail, Escape, With, Handler, Provide, Use, Op call, Fiber or Loop.
//! Postconditions: the call is the callee's body with fresh locals and the call's type arguments;
//!                 each argument is evaluated once, left to right, into the fresh local of its
//!                 parameter before the body runs. The callee stays in the module. A second run
//!                 of the pipeline (which repeats every pass to a fixed point) changes nothing.
//! Preserves: evaluation order, effects, failures, drop points and evidence. The arguments are
//!            evaluated where they were, once each; the body runs where the call was. Since the
//!            callee cannot fail and has no `with`, no release order can change.
//! Deopt: a call that is indirect, to an external function or a const, to a compiler-generated
//!        helper (a name with `$`), in a cycle, too large, to a bundled collection function or a
//!        derived impl, or whose copy would exceed the expansion budget is left as a call.
//! Measures: `nbody` (`zero_body`, `get_body`) and `fannkuch` (its tuple helpers), by bringing
//!           the record and tuple that escape only because they cross a call into one function
//!           for the LIR's scalar replacement of aggregates.
//! ```
//!
//! Inlining runs after the fusions: a fusion recognises the call the source wrote (`List.fold`,
//! `Option.or`), and inlining those functions first would dissolve it.

use rustc_hash::{FxHashMap, FxHashSet};

use crate::ir::{
    Arm, Assert, AssertCond, Callee, Expr, FnId, Local, Module, Pat, Release, StrPart, TypeId,
    WithBind,
};
use crate::opt::{CoreFns, PassStats};

/// The most parameters a callee may take. The evidence table, if there is one, counts.
const MAX_PARAMS: usize = 4;

/// The most nodes a callee's body may have. Small enough that a callsite stays small even when a
/// callee calls another candidate; large enough for a record literal or a short match.
const MAX_CALLEE_NODES: usize = 24;

/// The most nodes one call site may grow to. An expansion that would be larger is abandoned whole,
/// so that a later run of the pipeline (with the same, deterministic budget) abandons it again
/// and the pass is idempotent.
const MAX_EXPANDED_NODES: usize = 128;

/// Replaces calls to small, non-recursive functions by copies of their bodies.
///
/// See the module docs for the contract. `core_fns` identifies the bundled functions that the
/// fusions match or that are too costly to copy into every caller; none of them is inlined.
pub(super) fn inline(m: &mut Module, core_fns: &CoreFns) -> PassStats {
    let graph = call_graph(m);
    let cyclic = cyclic_functions(&graph);
    let mut candidates: FxHashSet<FnId> = FxHashSet::default();
    for (id, _) in m.functions.iter() {
        if is_candidate(m, id, core_fns, &cyclic) {
            candidates.insert(id);
        }
    }
    let mut changed = 0;
    crate::opt::each_body(m, &mut |m, body| {
        rewrite(m, body, &candidates, &mut changed)
    });
    PassStats {
        inline: changed,
        ..PassStats::default()
    }
}

/// Whether the call to `f` may be copied: the checks that do not depend on the call itself.
fn is_candidate(m: &Module, f: FnId, core_fns: &CoreFns, cyclic: &[bool]) -> bool {
    let func = &m.functions[f];
    if func.external.is_some() || func.constant || func.fails || func.suspends {
        return false;
    }
    // A function whose E or R still has a set type parameter is generic over effects; its evidence
    // and type arguments do not substitute with `TypeTable::subst` alone.
    if !func.set_params.is_empty() {
        return false;
    }
    if func.params.len() > MAX_PARAMS {
        return false;
    }
    // A compiler-generated function (`X$init`, `X$release`, a lifted local fn `outer$inner`, a
    // `$suspend` copy) is not a function the source wrote. `$` does not occur in Emela names
    // (`Lowerer::ir_name`), and copying these changes the shape handler lowering produces for no
    // gain.
    if func.name.contains('$') {
        return false;
    }
    if cyclic[index(f)] {
        return false;
    }
    // The collection functions are what the range fusion matches, and copying them into every
    // caller would grow the output for no gain.
    if core_fns.contains_key(&f) {
        return false;
    }
    // A trait function's body depends on the impl the type picks, which monomorphization
    // resolves (`Module::dispatch`). That includes the derivable traits (`Show.show`, `Eq.eq`),
    // whose impls are generated per type; copying the body here would copy the generic default.
    if m.dispatch.contains_key(&f) {
        return false;
    }
    if forbidden(&func.body) {
        return false;
    }
    node_count(&func.body) <= MAX_CALLEE_NODES
}

/// Whether `e` has a node the copy cannot reproduce faithfully.
fn forbidden(e: &Expr) -> bool {
    match e {
        Expr::Fail(_)
        | Expr::Escape { .. }
        | Expr::With { .. }
        | Expr::Handler { .. }
        | Expr::Provide { .. }
        | Expr::Use { .. }
        | Expr::Fiber { .. } => return true,
        Expr::Call {
            callee: Callee::Op { .. },
            ..
        } => return true,
        // A `Loop` is a loopified self tail call. Copying it at every call site would duplicate
        // the loop, and an aggregate builder that SRA can help with is not a loop.
        Expr::Loop { .. } => return true,
        _ => {}
    }
    let mut found = false;
    e.for_each_child(&mut |c| found |= forbidden(c));
    found
}

/// The number of expression nodes in `e`.
fn node_count(e: &Expr) -> usize {
    let mut count = 1;
    e.for_each_child(&mut |c| count += node_count(c));
    count
}

/// Rewrites the calls in `e`, children first, and then `e` itself if it is a call to a candidate.
fn rewrite(m: &mut Module, e: &mut Expr, candidates: &FxHashSet<FnId>, changed: &mut u32) {
    crate::tail::for_each_child_mut(e, &mut |c| rewrite(m, c, candidates, changed));
    let eligible = matches!(
        e,
        Expr::Call { callee: Callee::Direct { f, .. }, fails: false, suspends: false, .. }
            if candidates.contains(f)
    );
    if !eligible {
        return;
    }
    let mut budget = MAX_EXPANDED_NODES;
    if let Expansion::Done(expanded) = expand(m, e.clone(), candidates, &mut budget) {
        *e = expanded;
        *changed += 1;
    }
}

/// What [`expand`] decided about a call.
enum Expansion {
    /// The call was replaced.
    Done(Expr),
    /// The call is not one to expand (including a malformed one); it is returned unchanged.
    NotEligible(Expr),
    /// The expansion would exceed the budget; the whole call site is left alone.
    Exhausted,
}

/// Replaces `call` by the body of its callee if it is a call to a candidate.
fn expand(
    m: &mut Module,
    call: Expr,
    candidates: &FxHashSet<FnId>,
    budget: &mut usize,
) -> Expansion {
    let eligible = match &call {
        Expr::Call {
            callee: Callee::Direct { f, .. },
            fails: false,
            suspends: false,
            ..
        } => candidates.contains(f),
        _ => false,
    };
    if !eligible {
        return Expansion::NotEligible(call);
    }
    if *budget == 0 {
        return Expansion::Exhausted;
    }
    let Expr::Call {
        callee,
        args,
        fails,
        suspends,
    } = call
    else {
        unreachable!()
    };
    let Callee::Direct { f, ty_args } = callee else {
        unreachable!()
    };
    let params = m.functions[f].params.clone();
    if params.len() != args.len() || ty_args.len() != m.functions[f].type_params {
        return Expansion::NotEligible(Expr::Call {
            callee: Callee::Direct { f, ty_args },
            args,
            fails,
            suspends,
        });
    }
    if *budget < args.len() {
        return Expansion::Exhausted;
    }
    *budget -= args.len();
    let body = m.functions[f].body.clone();
    let mut map = FxHashMap::default();
    let mut vars = Vec::with_capacity(params.len());
    for &p in &params {
        vars.push(copy_local(m, p, &ty_args, &mut map));
    }
    let Some(mut body) = copy_expr(m, &body, &ty_args, &mut map, candidates, budget) else {
        return Expansion::Exhausted;
    };
    // The arguments are bound left to right before the body: `let p0 = a0; let p1 = a1; body`.
    for (var, value) in vars.into_iter().zip(args).rev() {
        body = Expr::Let {
            var,
            value: Box::new(value),
            body: Box::new(body),
        };
    }
    Expansion::Done(body)
}

/// The local `old` becomes in a copy: the same hint, its type with `args` substituted.
fn copy_local(
    m: &mut Module,
    old: Local,
    args: &[TypeId],
    map: &mut FxHashMap<Local, Local>,
) -> Local {
    if let Some(&new) = map.get(&old) {
        return new;
    }
    let ty = copy_ty(m, m.locals[old].ty, args);
    let hint = m.locals[old].hint.clone();
    let new = m.local(&hint, ty);
    map.insert(old, new);
    new
}

/// The type `ty` with `args` substituted; unchanged when the callee is not generic.
fn copy_ty(m: &mut Module, ty: TypeId, args: &[TypeId]) -> TypeId {
    if args.is_empty() {
        ty
    } else {
        m.types.subst(ty, args)
    }
}

fn copy_tys(m: &mut Module, tys: &[TypeId], args: &[TypeId]) -> Vec<TypeId> {
    tys.iter().map(|&t| copy_ty(m, t, args)).collect()
}

/// Copies `e` with its locals renamed and its type arguments substituted, expanding the calls to
/// candidates in it. Returns `None` if the copy would exceed `budget`.
fn copy_expr(
    m: &mut Module,
    e: &Expr,
    args: &[TypeId],
    map: &mut FxHashMap<Local, Local>,
    candidates: &FxHashSet<FnId>,
    budget: &mut usize,
) -> Option<Expr> {
    if *budget == 0 {
        return None;
    }
    *budget -= 1;
    let copied = match e {
        Expr::Lit(lit) => Expr::Lit(lit.clone()),
        Expr::Var(l) => Expr::Var(copy_local(m, *l, args, map)),
        Expr::Fn { f, ty_args } => Expr::Fn {
            f: *f,
            ty_args: copy_tys(m, ty_args, args),
        },
        Expr::Let { var, value, body } => Expr::Let {
            var: copy_local(m, *var, args, map),
            value: Box::new(copy_expr(m, value, args, map, candidates, budget)?),
            body: Box::new(copy_expr(m, body, args, map, candidates, budget)?),
        },
        Expr::Call {
            callee,
            args: call_args,
            fails,
            suspends,
        } => Expr::Call {
            callee: match callee {
                Callee::Direct { f, ty_args } => Callee::Direct {
                    f: *f,
                    ty_args: copy_tys(m, ty_args, args),
                },
                Callee::Indirect(c) => {
                    Callee::Indirect(Box::new(copy_expr(m, c, args, map, candidates, budget)?))
                }
                Callee::Op {
                    handler,
                    effect,
                    op,
                    ty_args,
                } => Callee::Op {
                    handler: Box::new(copy_expr(m, handler, args, map, candidates, budget)?),
                    effect: *effect,
                    op: *op,
                    ty_args: copy_tys(m, ty_args, args),
                },
            },
            args: copy_exprs(m, call_args, args, map, candidates, budget)?,
            fails: *fails,
            suspends: *suspends,
        },
        Expr::Builtin {
            op,
            ty_args,
            args: bs,
        } => Expr::Builtin {
            op: *op,
            ty_args: copy_tys(m, ty_args, args),
            args: copy_exprs(m, bs, args, map, candidates, budget)?,
        },
        Expr::Ctor {
            ctor,
            ty_args,
            args: cs,
        } => Expr::Ctor {
            ctor: *ctor,
            ty_args: copy_tys(m, ty_args, args),
            args: copy_exprs(m, cs, args, map, candidates, budget)?,
        },
        Expr::Field { base, ctor, index } => Expr::Field {
            base: Box::new(copy_expr(m, base, args, map, candidates, budget)?),
            ctor: *ctor,
            index: *index,
        },
        Expr::Tuple(es) => Expr::Tuple(copy_exprs(m, es, args, map, candidates, budget)?),
        Expr::List { elem, elems, tail } => Expr::List {
            elem: copy_ty(m, *elem, args),
            elems: copy_exprs(m, elems, args, map, candidates, budget)?,
            tail: match tail {
                Some(t) => Some(Box::new(copy_expr(m, t, args, map, candidates, budget)?)),
                None => None,
            },
        },
        Expr::If { cond, then, else_ } => Expr::If {
            cond: Box::new(copy_expr(m, cond, args, map, candidates, budget)?),
            then: Box::new(copy_expr(m, then, args, map, candidates, budget)?),
            else_: Box::new(copy_expr(m, else_, args, map, candidates, budget)?),
        },
        Expr::Match { scrutinee, arms } => Expr::Match {
            scrutinee: Box::new(copy_expr(m, scrutinee, args, map, candidates, budget)?),
            arms: copy_arms(m, arms, args, map, candidates, budget)?,
        },
        Expr::Binary { op, ty, lhs, rhs } => Expr::Binary {
            op: *op,
            ty: *ty,
            lhs: Box::new(copy_expr(m, lhs, args, map, candidates, budget)?),
            rhs: Box::new(copy_expr(m, rhs, args, map, candidates, budget)?),
        },
        Expr::Unary { op, ty, operand } => Expr::Unary {
            op: *op,
            ty: *ty,
            operand: Box::new(copy_expr(m, operand, args, map, candidates, budget)?),
        },
        Expr::Lambda {
            params,
            ty,
            body,
            suspends,
        } => Expr::Lambda {
            params: params
                .iter()
                .map(|&p| copy_local(m, p, args, map))
                .collect(),
            ty: copy_ty(m, *ty, args),
            body: Box::new(copy_expr(m, body, args, map, candidates, budget)?),
            suspends: *suspends,
        },
        Expr::Concat(parts) => {
            let mut copied = Vec::with_capacity(parts.len());
            for part in parts {
                copied.push(match part {
                    StrPart::Lit(text) => StrPart::Lit(text.clone()),
                    StrPart::Value(value, ty) => StrPart::Value(
                        copy_expr(m, value, args, map, candidates, budget)?,
                        copy_ty(m, *ty, args),
                    ),
                });
            }
            Expr::Concat(copied)
        }
        Expr::Loop { vars, inits, body } => Expr::Loop {
            vars: vars.iter().map(|&v| copy_local(m, v, args, map)).collect(),
            inits: copy_exprs(m, inits, args, map, candidates, budget)?,
            body: Box::new(copy_expr(m, body, args, map, candidates, budget)?),
        },
        Expr::Recur(es) => Expr::Recur(copy_exprs(m, es, args, map, candidates, budget)?),
        Expr::Assert(a) => {
            Expr::Assert(Box::new(copy_assert(m, a, args, map, candidates, budget)?))
        }
        Expr::Fail(inner) => Expr::Fail(Box::new(copy_expr(
            m, inner, args, map, candidates, budget,
        )?)),
        Expr::Escape { body, arms } => Expr::Escape {
            body: Box::new(copy_expr(m, body, args, map, candidates, budget)?),
            arms: copy_arms(m, arms, args, map, candidates, budget)?,
        },
        Expr::Use { ctx, effect } => Expr::Use {
            ctx: Box::new(copy_expr(m, ctx, args, map, candidates, budget)?),
            effect: *effect,
        },
        Expr::Provide { base, bindings } => Expr::Provide {
            base: match base {
                Some(base) => Some(Box::new(copy_expr(m, base, args, map, candidates, budget)?)),
                None => None,
            },
            bindings: {
                let mut copied = Vec::with_capacity(bindings.len());
                for (effect, value) in bindings {
                    copied.push((*effect, copy_expr(m, value, args, map, candidates, budget)?));
                }
                copied
            },
        },
        Expr::Handler { name, effect, ops } => Expr::Handler {
            name: name.clone(),
            effect: *effect,
            ops: copy_exprs(m, ops, args, map, candidates, budget)?,
        },
        Expr::With { binds, body } => Expr::With {
            binds: {
                let mut copied = Vec::with_capacity(binds.len());
                for bind in binds {
                    copied.push(WithBind {
                        var: copy_local(m, bind.var, args, map),
                        value: copy_expr(m, &bind.value, args, map, candidates, budget)?,
                        release: match &bind.release {
                            Some(release) => Some(Release {
                                handler: release.handler.clone(),
                                body: copy_expr(m, &release.body, args, map, candidates, budget)?,
                            }),
                            None => None,
                        },
                    });
                }
                copied
            },
            body: Box::new(copy_expr(m, body, args, map, candidates, budget)?),
        },
        Expr::Fiber { op, args: fs } => Expr::Fiber {
            op: *op,
            args: copy_exprs(m, fs, args, map, candidates, budget)?,
        },
    };
    if matches!(&copied, Expr::Call { .. }) {
        return match expand(m, copied, candidates, budget) {
            Expansion::Done(expanded) => Some(expanded),
            Expansion::NotEligible(call) => Some(call),
            Expansion::Exhausted => None,
        };
    }
    Some(copied)
}

fn copy_exprs(
    m: &mut Module,
    es: &[Expr],
    args: &[TypeId],
    map: &mut FxHashMap<Local, Local>,
    candidates: &FxHashSet<FnId>,
    budget: &mut usize,
) -> Option<Vec<Expr>> {
    let mut copied = Vec::with_capacity(es.len());
    for e in es {
        copied.push(copy_expr(m, e, args, map, candidates, budget)?);
    }
    Some(copied)
}

fn copy_arms(
    m: &mut Module,
    arms: &[Arm],
    args: &[TypeId],
    map: &mut FxHashMap<Local, Local>,
    candidates: &FxHashSet<FnId>,
    budget: &mut usize,
) -> Option<Vec<Arm>> {
    let mut copied = Vec::with_capacity(arms.len());
    for arm in arms {
        copied.push(Arm {
            pat: copy_pat(m, &arm.pat, args, map),
            guard: match &arm.guard {
                Some(guard) => Some(copy_expr(m, guard, args, map, candidates, budget)?),
                None => None,
            },
            body: copy_expr(m, &arm.body, args, map, candidates, budget)?,
        });
    }
    Some(copied)
}

fn copy_pat(m: &mut Module, p: &Pat, args: &[TypeId], map: &mut FxHashMap<Local, Local>) -> Pat {
    match p {
        Pat::Wild => Pat::Wild,
        Pat::Bind(l) => Pat::Bind(copy_local(m, *l, args, map)),
        Pat::Lit(lit) => Pat::Lit(lit.clone()),
        Pat::Ctor { ctor, fields } => Pat::Ctor {
            ctor: *ctor,
            fields: fields.iter().map(|f| copy_pat(m, f, args, map)).collect(),
        },
        Pat::Tuple(ps) => Pat::Tuple(ps.iter().map(|p| copy_pat(m, p, args, map)).collect()),
        Pat::List { elems, rest } => Pat::List {
            elems: elems.iter().map(|p| copy_pat(m, p, args, map)).collect(),
            rest: rest.as_ref().map(|r| Box::new(copy_pat(m, r, args, map))),
        },
    }
}

fn copy_assert(
    m: &mut Module,
    a: &Assert,
    args: &[TypeId],
    map: &mut FxHashMap<Local, Local>,
    candidates: &FxHashSet<FnId>,
    budget: &mut usize,
) -> Option<Assert> {
    let cond = match &a.cond {
        AssertCond::Compare {
            op,
            op_ty,
            ty,
            lhs,
            rhs,
        } => AssertCond::Compare {
            op: *op,
            op_ty: *op_ty,
            ty: copy_ty(m, *ty, args),
            lhs: copy_expr(m, lhs, args, map, candidates, budget)?,
            rhs: copy_expr(m, rhs, args, map, candidates, budget)?,
        },
        AssertCond::Shown {
            cond,
            op,
            ty,
            lhs,
            rhs,
        } => AssertCond::Shown {
            cond: copy_expr(m, cond, args, map, candidates, budget)?,
            op: *op,
            ty: copy_ty(m, *ty, args),
            lhs: copy_expr(m, lhs, args, map, candidates, budget)?,
            rhs: copy_expr(m, rhs, args, map, candidates, budget)?,
        },
        AssertCond::Bool(cond) => {
            AssertCond::Bool(copy_expr(m, cond, args, map, candidates, budget)?)
        }
    };
    Some(Assert {
        cond,
        text: a.text.clone(),
        location: a.location.clone(),
    })
}

/// The direct calls in every function body, by the index of the caller and the callee.
fn call_graph(m: &Module) -> Vec<Vec<usize>> {
    let mut graph = vec![Vec::new(); m.functions.len()];
    for (id, f) in m.functions.iter() {
        let mut calls = Vec::new();
        collect_calls(&f.body, &mut calls);
        calls.sort_unstable();
        calls.dedup();
        graph[index(id)] = calls;
    }
    graph
}

fn collect_calls(e: &Expr, out: &mut Vec<usize>) {
    if let Expr::Call {
        callee: Callee::Direct { f, .. },
        ..
    } = e
    {
        out.push(index(*f));
    }
    e.for_each_child(&mut |c| collect_calls(c, out));
}

/// Which functions are in a cycle of `graph`: a component of more than one function, or a self
/// call. A self tail call is a `Loop` by then, so it does not count as a self call.
fn cyclic_functions(graph: &[Vec<usize>]) -> Vec<bool> {
    let n = graph.len();
    let mut order = Vec::with_capacity(n);
    let mut visited = vec![false; n];
    for start in 0..n {
        if visited[start] {
            continue;
        }
        visited[start] = true;
        // (node, next edge to visit), so the postorder is built without recursion.
        let mut stack = vec![(start, 0usize)];
        while let Some((v, next)) = stack.pop() {
            if next < graph[v].len() {
                stack.push((v, next + 1));
                let w = graph[v][next];
                if !visited[w] {
                    visited[w] = true;
                    stack.push((w, 0));
                }
            } else {
                order.push(v);
            }
        }
    }
    let mut reverse = vec![Vec::new(); n];
    for (v, edges) in graph.iter().enumerate() {
        for &w in edges {
            reverse[w].push(v);
        }
    }
    let mut component = vec![usize::MAX; n];
    let mut size = 0;
    for &start in order.iter().rev() {
        if component[start] != usize::MAX {
            continue;
        }
        component[start] = size;
        let mut stack = vec![start];
        while let Some(v) = stack.pop() {
            for &w in &reverse[v] {
                if component[w] == usize::MAX {
                    component[w] = size;
                    stack.push(w);
                }
            }
        }
        size += 1;
    }
    let mut sizes = vec![0usize; size];
    for &c in &component {
        sizes[c] += 1;
    }
    (0..n)
        .map(|v| sizes[component[v]] > 1 || graph[v].contains(&v))
        .collect()
}

fn index(id: FnId) -> usize {
    u32::from(id.into_raw()) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{DeriveOp, Derived, Dispatch, Lit};

    /// A module with one function taking `params` `Int`s whose body is built by `body`, and the
    /// id of the function with its parameters.
    fn module_with(
        name: &str,
        params: usize,
        body: impl FnOnce(&mut Module, &[Local]) -> Expr,
    ) -> (Module, FnId) {
        let mut m = Module::new();
        let f = m.declare(name, false);
        let ps: Vec<Local> = (0..params)
            .map(|i| m.local(&format!("p{i}"), TypeId::INT))
            .collect();
        let e = body(&mut m, &ps);
        m.define(f, ps, TypeId::INT, e);
        (m, f)
    }

    fn candidate(m: &Module, f: FnId) -> bool {
        let graph = call_graph(m);
        let cyclic = cyclic_functions(&graph);
        is_candidate(m, f, &CoreFns::default(), &cyclic)
    }

    #[test]
    fn a_plain_function_is_a_candidate() {
        let (m, f) = module_with("f", 1, |_, ps| Expr::Var(ps[0]));
        assert!(candidate(&m, f));
    }

    #[test]
    fn a_derived_impl_is_not_a_candidate() {
        let (mut m, f) = module_with("User: Show.show", 1, |_, _| Expr::Lit(Lit::Unit));
        m.dispatch.insert(
            f,
            Dispatch {
                impls: Vec::new(),
                default: false,
                structural: Vec::new(),
                derived: Some(Derived {
                    op: DeriveOp::Show,
                    enums: Vec::new(),
                }),
            },
        );
        assert!(!candidate(&m, f));
    }

    #[test]
    fn a_failing_or_suspending_function_is_not_a_candidate() {
        let (mut m, f) = module_with("f", 0, |_, _| Expr::Lit(Lit::Unit));
        m.functions[f].fails = true;
        assert!(!candidate(&m, f));
        m.functions[f].fails = false;
        m.functions[f].suspends = true;
        assert!(!candidate(&m, f));
    }

    #[test]
    fn a_function_with_too_many_parameters_is_not_a_candidate() {
        let (m, f) = module_with("f", MAX_PARAMS + 1, |_, _| Expr::Lit(Lit::Unit));
        assert!(!candidate(&m, f));
    }

    #[test]
    fn a_body_with_a_handler_is_not_a_candidate() {
        let (m, f) = module_with("f", 0, |_, _| Expr::Escape {
            body: Box::new(Expr::Lit(Lit::Unit)),
            arms: Vec::new(),
        });
        assert!(!candidate(&m, f));
    }

    #[test]
    fn a_loop_is_not_a_candidate() {
        let (m, f) = module_with("f", 0, |m, _| {
            let v = m.local("v", TypeId::INT);
            Expr::Loop {
                vars: vec![v],
                inits: vec![Expr::Lit(Lit::Int(0))],
                body: Box::new(Expr::Var(v)),
            }
        });
        assert!(!candidate(&m, f));
    }

    #[test]
    fn a_compiler_generated_helper_is_not_a_candidate() {
        let (m, f) = module_with("Type$init", 0, |_, _| Expr::Lit(Lit::Unit));
        assert!(!candidate(&m, f));
    }
}
