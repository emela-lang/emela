//! Substitution: replacing a local by a value expression, and copying a tree with fresh binders.
//!
//! Substitution is shared by the let-level rules (`zeta`, `beta`, `iota`, `delta`). It is not a
//! rule of its own: it has no counter and never walks a whole module by itself.
//!
//! # Contract
//!
//! ```text
//! Rule ID: subst (helper, not a rule)
//! Theory: substitution in the let calculus, with capture avoidance and fresh locals
//! Equation: e[x := v]
//! Preconditions: v is trivial (a literal, a variable read, a function reference or a lambda) and
//!                binds no local.
//! Postconditions: every free occurrence of x in e is v, and no other local changed.
//! Preserves: evaluation order, effects, failures, suspension points, drop points.
//! Deopt: v is not trivial, so replacing an occurrence could move, duplicate or drop its
//!        evaluation; or v binds a local, which a copy would bind a second time.
//! Budget: the size of the expression substituted, so a rule can refuse an unbounded copy.
//! Measures: none of its own; the calling rule counts what it rewrote.
//! ```
//!
//! # Capture avoidance
//!
//! A `Local` is bound at most once in the whole module ([`crate::ir`]). A module-wide unique name
//! cannot be shadowed, so replacing reads of `x` in `e` can never be captured by a binder inside
//! `e`: such a binder would be a second binding site for a local that already has one. Substitution
//! needs no alpha-renaming of `e` itself, and [`Subst`] never allocates.
//!
//! Copying a tree *into another context* (β, δ, the range fusion) is the other direction: the
//! copy's binders must not be the same locals as the original's, or the module would again have two
//! binding sites. [`freshen`] is that mechanism: one old→new map, because uniqueness means there is
//! no shadowing to track. [`subst`] refuses a value that binds a local instead of duplicating it.
//!
//! # Status
//!
//! A1/PR-010 completes the engine: [`use_count`], [`size`], [`fresh`]/[`fresh_like`], [`rename`],
//! [`freshen`] and the exact [`is_trivial`] boundaries. The rules that use it land one at a time
//! (A2/PR-011 to A8/PR-017); nothing in the pipeline calls these helpers yet, so the output of the
//! compiler is unchanged.

use rustc_hash::FxHashMap;

use crate::ir::{Expr, Local, Module, Pat, StrPart, TypeId};

/// What one [`subst`] call changed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Subst {
    /// Occurrences of the variable replaced.
    pub replaced: u32,
    /// Fresh locals allocated for copied binders. Always zero: [`subst`] allocates nothing, and a
    /// caller that copies binders calls [`freshen`].
    pub fresh: u32,
    /// Whether the call was abandoned, leaving the expression unchanged.
    pub deopt: bool,
}

/// Whether `e` may be evaluated more than once without changing the observable behaviour.
///
/// A literal, a variable read, a function reference and making a lambda have no effect, cannot
/// fail, cannot suspend and cannot raise a defect: at most they allocate. So may a tuple, list or
/// constructor whose elements all are, a field read of such a base, and a string concatenation of
/// literal text only.
///
/// Everything else is not trivial. A call, builtin, assertion, `fail`, `if`, `match`, `escape`,
/// `with`, handler, `provide`, `use`, fiber operation, `loop` and `recur` can have an effect, fail,
/// suspend or diverge. An operator can raise a defect (`Div` or `Rem` by zero) or short-circuit its
/// right side, so every `Binary` and `Unary` stays non-trivial even when its operands are literals.
/// Dropping or duplicating any of them changes what the program does, not only its memory use.
pub fn is_trivial(e: &Expr) -> bool {
    match e {
        Expr::Lit(_) | Expr::Var(_) | Expr::Fn { .. } | Expr::Lambda { .. } => true,
        Expr::Tuple(es) | Expr::Ctor { args: es, .. } => es.iter().all(is_trivial),
        Expr::List { elems, tail, .. } => {
            elems.iter().all(is_trivial) && tail.as_deref().is_none_or(is_trivial)
        }
        Expr::Field { base, .. } => is_trivial(base),
        Expr::Concat(parts) => parts.iter().all(|p| matches!(p, StrPart::Lit(_))),
        _ => false,
    }
}

/// The number of expression nodes in `e` (patterns are not counted).
///
/// This is the size measure the β and δ budgets read, and the non-increase assertion of the rules
/// that must not grow the program.
pub fn size(e: &Expr) -> u32 {
    let mut count = 1;
    e.for_each_child(&mut |c| count += size(c));
    count
}

/// The number of free occurrences of `var` in `e`.
///
/// Locals are unique in a module (`crate::ir`), so a binder of the same [`Local`] cannot occur
/// inside `e`; every occurrence is free. Binder positions (`let`, `loop`, lambda parameters,
/// pattern binds, `with`) are not `Expr::Var` nodes, so they are not counted. The count is the
/// use analysis's ([`crate::opt::analysis::uses::of_expr`]), which the reductions build on.
pub fn use_count(e: &Expr, var: Local) -> u32 {
    crate::opt::analysis::uses::of_expr(e, var)
}

/// A new local with the given hint and type.
pub fn fresh(m: &mut Module, hint: &str, ty: TypeId) -> Local {
    m.local(hint, ty)
}

/// A new local with the hint and type of `old`.
///
/// A copy of a tree keeps the shape and the types; only the identity of its binders changes, so
/// this is what [`freshen`] allocates for them.
pub fn fresh_like(m: &mut Module, old: Local) -> Local {
    let data = &m.locals[old];
    let (hint, ty) = (data.hint.clone(), data.ty);
    m.local(&hint, ty)
}

/// Renames the reads in `e` that the map lists; binders and unlisted locals are left alone.
///
/// This is the tree half of an alpha-renaming. [`freshen`] uses the same walk and renames the
/// binder positions too; a caller that renames a range of locals (the range fusion) only has reads
/// to change.
pub fn rename(e: &mut Expr, map: &FxHashMap<Local, Local>) {
    if let Expr::Var(l) = e {
        if let Some(&new) = map.get(l) {
            *l = new;
        }
        return;
    }
    crate::tail::for_each_child_mut(e, &mut |child| rename(child, map));
}

/// Gives every binder in `e` a fresh local and renames every read of it, so that the result can be
/// copied into another context without capturing or rebinding anything.
///
/// The binder forms are every one the Core IR has: `let`, lambda parameters, loop variables,
/// `match` and `escape` arm patterns (including nested constructor, tuple and list patterns), and
/// `with` bindings. Free locals (reads of a local not bound in `e`) stay as they are: they are still
/// in scope wherever `e` is copied, and uniqueness means no binder at the destination is the same
/// local.
///
/// Locals are globally unique, so there is nothing to shadow: one old→new map collected in a first
/// walk is the whole renaming, and a local bound inside `e` cannot occur free outside it. The walk
/// allocates one local per binder, in pre-order, and does not change a type. Returns the number of
/// fresh locals allocated.
pub fn freshen(m: &mut Module, e: &mut Expr) -> u32 {
    let mut map = FxHashMap::default();
    collect_binders(m, e, &mut map);
    if map.is_empty() {
        return 0;
    }
    freshen_tree(e, &map);
    map.len() as u32
}

fn collect_binders(m: &mut Module, e: &Expr, map: &mut FxHashMap<Local, Local>) {
    match e {
        Expr::Let { var, .. } => bind(m, map, *var),
        Expr::Lambda { params, .. } => params.iter().for_each(|&p| bind(m, map, p)),
        Expr::Loop { vars, .. } => vars.iter().for_each(|&v| bind(m, map, v)),
        Expr::With { binds, .. } => binds.iter().for_each(|b| bind(m, map, b.var)),
        Expr::Match { arms, .. } | Expr::Escape { arms, .. } => {
            for arm in arms {
                collect_pat_binders(m, &arm.pat, map);
            }
        }
        _ => {}
    }
    e.for_each_child(&mut |child| collect_binders(m, child, map));
}

fn collect_pat_binders(m: &mut Module, p: &Pat, map: &mut FxHashMap<Local, Local>) {
    match p {
        Pat::Bind(l) => bind(m, map, *l),
        Pat::Ctor { fields, .. } => fields.iter().for_each(|f| collect_pat_binders(m, f, map)),
        Pat::Tuple(ps) => ps.iter().for_each(|p| collect_pat_binders(m, p, map)),
        Pat::List { elems, rest } => {
            elems.iter().for_each(|p| collect_pat_binders(m, p, map));
            if let Some(rest) = rest {
                collect_pat_binders(m, rest, map);
            }
        }
        Pat::Wild | Pat::Lit(_) => {}
    }
}

/// Records the fresh local of `old`.
///
/// A local is bound at most once in a module, so a second entry is a broken module rather than a
/// shadowing that the renaming would have to resolve.
fn bind(m: &mut Module, map: &mut FxHashMap<Local, Local>, old: Local) {
    let new = fresh_like(m, old);
    let previous = map.insert(old, new);
    debug_assert!(
        previous.is_none(),
        "a local is bound at most once in a module"
    );
}

fn freshen_tree(e: &mut Expr, map: &FxHashMap<Local, Local>) {
    match e {
        Expr::Var(l) => {
            if let Some(&new) = map.get(&*l) {
                *l = new;
            }
            return;
        }
        Expr::Let { var, .. } => rename_binder(var, map),
        Expr::Lambda { params, .. } => params.iter_mut().for_each(|p| rename_binder(p, map)),
        Expr::Loop { vars, .. } => vars.iter_mut().for_each(|v| rename_binder(v, map)),
        Expr::With { binds, .. } => binds
            .iter_mut()
            .for_each(|b| rename_binder(&mut b.var, map)),
        Expr::Match { arms, .. } | Expr::Escape { arms, .. } => {
            for arm in arms {
                freshen_pat(&mut arm.pat, map);
            }
        }
        _ => {}
    }
    crate::tail::for_each_child_mut(e, &mut |child| freshen_tree(child, map));
}

fn rename_binder(l: &mut Local, map: &FxHashMap<Local, Local>) {
    if let Some(&new) = map.get(&*l) {
        *l = new;
    }
}

fn freshen_pat(p: &mut Pat, map: &FxHashMap<Local, Local>) {
    match p {
        Pat::Bind(l) => rename_binder(l, map),
        Pat::Ctor { fields, .. } => fields.iter_mut().for_each(|f| freshen_pat(f, map)),
        Pat::Tuple(ps) => ps.iter_mut().for_each(|p| freshen_pat(p, map)),
        Pat::List { elems, rest } => {
            elems.iter_mut().for_each(|p| freshen_pat(p, map));
            if let Some(rest) = rest {
                freshen_pat(rest, map);
            }
        }
        Pat::Wild | Pat::Lit(_) => {}
    }
}

/// Replaces every occurrence of `var` in `e` by `value`, when that is conservative and sound.
///
/// The module that owns the locals is taken but not changed: substitution allocates nothing
/// (`Subst::fresh` stays zero), and a caller that copies a tree with binders calls [`freshen`] on
/// the copy.
///
/// The call deopts (`Subst::deopt`, expression unchanged) when `value` is not [`is_trivial`], and
/// when it binds a local: copying it to a use would bind that [`Local`] a second time, which breaks
/// the invariant that makes substitution capture-free. A trivial value is a literal, a variable
/// read, a function reference or a lambda, so moving it to an occurrence never moves, duplicates or
/// drops an evaluation. A variable with several uses is handled; with no occurrence the call
/// changes nothing and does not deopt, because whether the binding may be removed (the dead-let
/// half of ζ) is the calling rule's decision.
pub fn subst(_m: &mut Module, e: &mut Expr, var: Local, value: &Expr) -> Subst {
    if !is_trivial(value) {
        return Subst {
            deopt: true,
            ..Subst::default()
        };
    }
    if use_count(e, var) == 0 {
        return Subst::default();
    }
    if binds_local(value) {
        return Subst {
            deopt: true,
            ..Subst::default()
        };
    }
    let mut replaced = 0;
    replace(e, var, value, &mut replaced);
    Subst {
        replaced,
        fresh: 0,
        deopt: false,
    }
}

fn replace(e: &mut Expr, var: Local, value: &Expr, replaced: &mut u32) {
    if matches!(e, Expr::Var(v) if *v == var) {
        *e = value.clone();
        *replaced += 1;
        return;
    }
    crate::tail::for_each_child_mut(e, &mut |child| replace(child, var, value, replaced));
}

/// Whether `e` binds a local anywhere, so a copy of it would bind that local twice.
///
/// Only a lambda can appear inside a trivial value, but every binder form is checked so that this
/// stays correct if [`is_trivial`] grows.
fn binds_local(e: &Expr) -> bool {
    match e {
        Expr::Let { .. } | Expr::Lambda { .. } | Expr::Loop { .. } | Expr::With { .. } => {
            return true;
        }
        Expr::Match { arms, .. } | Expr::Escape { arms, .. }
            if arms.iter().any(|a| pat_binds(&a.pat)) =>
        {
            return true;
        }
        _ => {}
    }
    let mut found = false;
    e.for_each_child(&mut |child| found |= binds_local(child));
    found
}

fn pat_binds(p: &Pat) -> bool {
    match p {
        Pat::Bind(_) => true,
        Pat::Ctor { fields, .. } => fields.iter().any(pat_binds),
        Pat::Tuple(ps) => ps.iter().any(pat_binds),
        Pat::List { elems, rest } => {
            elems.iter().any(pat_binds) || rest.as_deref().is_some_and(pat_binds)
        }
        Pat::Wild | Pat::Lit(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intrinsics::Builtin;
    use crate::ir::{Arm, BinOp, Callee, Lit, OpTy};

    /// `let var = value; body`.
    fn let_(var: Local, value: Expr, body: Expr) -> Expr {
        Expr::Let {
            var,
            value: Box::new(value),
            body: Box::new(body),
        }
    }

    /// `lhs + rhs`, an operator that is not trivial.
    fn add(lhs: Expr, rhs: Expr) -> Expr {
        Expr::Binary {
            op: BinOp::Add,
            ty: OpTy::Int,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        }
    }

    /// A call to a fresh function, which may have an effect or fail.
    fn call(m: &mut Module) -> Expr {
        let f = m.declare("f", false);
        Expr::Call {
            callee: Callee::Direct {
                f,
                ty_args: Vec::new(),
            },
            args: Vec::new(),
            fails: false,
            suspends: false,
        }
    }

    /// `uses` counts reads, and a binder is not one: the variable of a `let` is a definition.
    #[test]
    fn use_count_counts_reads_and_not_binders() {
        let mut m = Module::new();
        let x = m.local("x", TypeId::INT);
        let y = m.local("y", TypeId::INT);
        let other = m.local("other", TypeId::INT);
        let e = let_(
            y,
            Expr::Var(x),
            Expr::Tuple(vec![Expr::Var(x), Expr::Var(y)]),
        );
        assert_eq!(use_count(&e, x), 2, "the value and the first element");
        assert_eq!(use_count(&e, y), 1, "only the read, not the binder");
        assert_eq!(use_count(&e, other), 0);
    }

    /// `subst` replaces the reads and leaves the binding alone: locals are unique, so a read is the
    /// only place the replacement can go.
    #[test]
    fn subst_replaces_reads_only() {
        let mut m = Module::new();
        let x = m.local("x", TypeId::INT);
        let y = m.local("y", TypeId::INT);
        let mut e = add(Expr::Var(x), Expr::Var(x));
        let done = subst(&mut m, &mut e, x, &Expr::Var(y));
        assert_eq!(
            done,
            Subst {
                replaced: 2,
                fresh: 0,
                deopt: false
            }
        );
        assert_eq!(use_count(&e, x), 0);
        assert_eq!(use_count(&e, y), 2);

        // The binder is not a read, so substituting the bound variable cannot capture the binder.
        let mut binder = let_(x, Expr::Lit(Lit::Int(0)), Expr::Var(x));
        let done = subst(&mut m, &mut binder, x, &Expr::Lit(Lit::Int(1)));
        assert_eq!(done.replaced, 1);
        let Expr::Let { var, value, body } = &binder else {
            panic!("the binding stayed a binding");
        };
        assert_eq!(*var, x);
        assert!(matches!(**value, Expr::Lit(Lit::Int(0))));
        assert!(matches!(**body, Expr::Lit(Lit::Int(1))));
    }

    /// A call is not trivial, whatever the callee is: the expression stays as it was.
    #[test]
    fn a_non_trivial_value_deopts_without_changing_the_expression() {
        let mut m = Module::new();
        let x = m.local("x", TypeId::INT);
        let mut e = Expr::Var(x);
        let before = e.clone();
        let value = call(&mut m);
        let done = subst(&mut m, &mut e, x, &value);
        assert!(done.deopt);
        assert_eq!(done.replaced, 0);
        assert_eq!(done.fresh, 0);
        assert_eq!(e, before);
    }

    /// A builtin may have an effect, fail or suspend, so it is not a value to copy either.
    #[test]
    fn a_builtin_value_deopts() {
        let mut m = Module::new();
        let x = m.local("x", TypeId::INT);
        let mut e = Expr::Var(x);
        let before = e.clone();
        let value = Expr::Builtin {
            op: Builtin::StringLength,
            ty_args: Vec::new(),
            args: vec![Expr::Lit(Lit::String("a".to_owned()))],
        };
        let done = subst(&mut m, &mut e, x, &value);
        assert!(done.deopt);
        assert_eq!(e, before);
    }

    /// A `fail` ends the computation, so it is not trivial and is never moved to a use.
    #[test]
    fn a_fail_value_deopts() {
        let mut m = Module::new();
        let x = m.local("x", TypeId::INT);
        let mut e = Expr::Var(x);
        let before = e.clone();
        let value = Expr::Fail(Box::new(Expr::Lit(Lit::Int(1))));
        let done = subst(&mut m, &mut e, x, &value);
        assert!(done.deopt);
        assert_eq!(e, before);
    }

    /// Making a lambda allocates only, so it is trivial; copying it would duplicate its binder,
    /// which `subst` refuses: `freshen` is the copying mechanism.
    #[test]
    fn a_lambda_value_that_binds_a_local_deopts() {
        let mut m = Module::new();
        let x = m.local("x", TypeId::INT);
        let p = m.local("p", TypeId::INT);
        let ty = m.fn_type(Vec::new(), TypeId::INT);
        let mut e = Expr::Var(x);
        let before = e.clone();
        let value = Expr::Lambda {
            params: vec![p],
            ty,
            body: Box::new(Expr::Lit(Lit::Int(1))),
            suspends: false,
        };
        assert!(is_trivial(&value));
        let done = subst(&mut m, &mut e, x, &value);
        assert!(done.deopt);
        assert_eq!(e, before);
    }

    /// No occurrence is not a deopt: the caller decides whether the binding may go.
    #[test]
    fn a_missing_variable_changes_nothing() {
        let mut m = Module::new();
        let x = m.local("x", TypeId::INT);
        let y = m.local("y", TypeId::INT);
        let mut e = Expr::Var(y);
        let done = subst(&mut m, &mut e, x, &Expr::Lit(Lit::Unit));
        assert_eq!(done, Subst::default());
        assert_eq!(e, Expr::Var(y));
    }

    /// The boundaries `is_trivial` is trusted for.
    #[test]
    fn is_trivial_boundaries() {
        let mut m = Module::new();
        assert!(!is_trivial(&call(&mut m)), "a call");
        assert!(
            is_trivial(&Expr::Ctor {
                ctor: m.some_ctor(),
                ty_args: vec![TypeId::INT],
                args: vec![Expr::Lit(Lit::Int(1))],
            }),
            "a constructor of a trivial element"
        );
        assert!(
            !is_trivial(&Expr::Binary {
                op: BinOp::Div,
                ty: OpTy::Int,
                lhs: Box::new(Expr::Lit(Lit::Int(1))),
                rhs: Box::new(Expr::Lit(Lit::Int(0))),
            }),
            "a division"
        );
        let ty = m.fn_type(Vec::new(), TypeId::INT);
        assert!(
            is_trivial(&Expr::Lambda {
                params: Vec::new(),
                ty,
                body: Box::new(Expr::Lit(Lit::Int(1))),
                suspends: false,
            }),
            "a lambda"
        );
        let base = m.local("x", TypeId::INT);
        assert!(
            is_trivial(&Expr::Field {
                base: Box::new(Expr::Var(base)),
                ctor: m.some_ctor(),
                index: 0,
            }),
            "a field of a trivial base"
        );
        assert!(
            is_trivial(&Expr::Concat(vec![
                StrPart::Lit("a".to_owned()),
                StrPart::Lit("b".to_owned()),
            ])),
            "a concatenation of literals"
        );
        assert!(
            !is_trivial(&Expr::Concat(vec![
                StrPart::Lit("a".to_owned()),
                StrPart::Value(Expr::Var(base), TypeId::INT),
            ])),
            "a concatenation with a value"
        );
    }

    /// `size` counts expression nodes: a leaf is one, and each operand adds its own tree.
    #[test]
    fn size_counts_nodes() {
        let mut m = Module::new();
        let x = m.local("x", TypeId::INT);
        assert_eq!(size(&Expr::Lit(Lit::Int(1))), 1);
        assert_eq!(size(&add(Expr::Var(x), Expr::Lit(Lit::Int(1)))), 3);
        assert_eq!(
            size(&let_(x, Expr::Lit(Lit::Int(1)), Expr::Var(x))),
            3,
            "the binding, its value and the body"
        );
    }

    /// A rewrite that removes a node makes the size smaller, which is what the non-increase
    /// assertions rely on.
    #[test]
    fn a_rewrite_that_removes_a_node_decreases_the_size() {
        let mut m = Module::new();
        let x = m.local("x", TypeId::INT);
        // `let x = 1 in x + 2`, and the body alone after ζ removes the binding.
        let e = let_(
            x,
            Expr::Lit(Lit::Int(1)),
            add(Expr::Var(x), Expr::Lit(Lit::Int(2))),
        );
        let before = size(&e);
        let Expr::Let { body, .. } = &e else { panic!() };
        assert!(size(body) < before);
    }

    /// A body with every binder form the Core IR has: a `let`, a lambda parameter, a loop variable
    /// and a `match` pattern bind, nested so that all of them are in scope where they are read.
    fn body_with_every_binder(m: &mut Module) -> (Vec<Local>, Expr) {
        let x = m.local("x", TypeId::INT);
        let y = m.local("y", TypeId::INT);
        let int_fn = m.fn_type(vec![TypeId::INT], TypeId::INT);
        let h = m.local("h", int_fn);
        let p = m.local("p", TypeId::INT);
        let i = m.local("i", TypeId::INT);
        let some = m.some_ctor();
        let none = m.none_ctor();

        // `let h = |p| p + x in h(y)`
        let some_arm = Arm {
            pat: Pat::Ctor {
                ctor: some,
                fields: vec![Pat::Bind(y)],
            },
            guard: None,
            body: let_(
                h,
                Expr::Lambda {
                    params: vec![p],
                    ty: int_fn,
                    body: Box::new(add(Expr::Var(p), Expr::Var(x))),
                    suspends: false,
                },
                Expr::Call {
                    callee: Callee::Indirect(Box::new(Expr::Var(h))),
                    args: vec![Expr::Var(y)],
                    fails: false,
                    suspends: false,
                },
            ),
        };
        // `loop i = x { i }`
        let none_arm = Arm {
            pat: Pat::Ctor {
                ctor: none,
                fields: Vec::new(),
            },
            guard: None,
            body: Expr::Loop {
                vars: vec![i],
                inits: vec![Expr::Var(x)],
                body: Box::new(Expr::Var(i)),
            },
        };
        let body = let_(
            x,
            Expr::Lit(Lit::Int(0)),
            Expr::Match {
                scrutinee: Box::new(Expr::Ctor {
                    ctor: some,
                    ty_args: vec![TypeId::INT],
                    args: vec![Expr::Var(x)],
                }),
                arms: vec![some_arm, none_arm],
            },
        );
        (vec![x, y, h, p, i], body)
    }

    /// Every local the expression mentions, whether it binds or reads it.
    fn mentioned(e: &Expr) -> std::collections::HashSet<Local> {
        let mut locals = Vec::new();
        collect_mentioned(e, &mut locals);
        locals.into_iter().collect()
    }

    /// The locals the expression binds.
    fn binders(e: &Expr) -> Vec<Local> {
        let mut locals = Vec::new();
        collect_binders_in(e, &mut locals);
        locals
    }

    fn collect_binders_in(e: &Expr, locals: &mut Vec<Local>) {
        match e {
            Expr::Let { var, .. } => locals.push(*var),
            Expr::Lambda { params, .. } => locals.extend(params),
            Expr::Loop { vars, .. } => locals.extend(vars),
            Expr::With { binds, .. } => locals.extend(binds.iter().map(|b| b.var)),
            Expr::Match { arms, .. } | Expr::Escape { arms, .. } => {
                for arm in arms {
                    collect_pat_mentioned(&arm.pat, locals);
                }
            }
            _ => {}
        }
        e.for_each_child(&mut |child| collect_binders_in(child, locals));
    }

    fn collect_mentioned(e: &Expr, locals: &mut Vec<Local>) {
        match e {
            Expr::Var(l) => locals.push(*l),
            Expr::Let { var, .. } => locals.push(*var),
            Expr::Lambda { params, .. } => locals.extend(params),
            Expr::Loop { vars, .. } => locals.extend(vars),
            Expr::With { binds, .. } => locals.extend(binds.iter().map(|b| b.var)),
            Expr::Match { arms, .. } | Expr::Escape { arms, .. } => {
                for arm in arms {
                    collect_pat_mentioned(&arm.pat, locals);
                }
            }
            _ => {}
        }
        e.for_each_child(&mut |child| collect_mentioned(child, locals));
    }

    fn collect_pat_mentioned(p: &Pat, locals: &mut Vec<Local>) {
        match p {
            Pat::Bind(l) => locals.push(*l),
            Pat::Ctor { fields, .. } => {
                fields.iter().for_each(|f| collect_pat_mentioned(f, locals))
            }
            Pat::Tuple(ps) => ps.iter().for_each(|p| collect_pat_mentioned(p, locals)),
            Pat::List { elems, rest } => {
                elems.iter().for_each(|p| collect_pat_mentioned(p, locals));
                if let Some(rest) = rest {
                    collect_pat_mentioned(rest, locals);
                }
            }
            Pat::Wild | Pat::Lit(_) => {}
        }
    }

    /// Every binder gets its own fresh local, and every read of it is renamed.
    #[test]
    fn freshen_gives_every_binder_a_distinct_local() {
        let mut m = Module::new();
        let (old, mut body) = body_with_every_binder(&mut m);
        let fresh_count = freshen(&mut m, &mut body);
        assert_eq!(fresh_count, old.len() as u32, "one fresh local per binder");

        let new = binders(&body);
        assert_eq!(new.len(), old.len());
        let distinct: std::collections::HashSet<Local> = new.iter().copied().collect();
        assert_eq!(distinct.len(), new.len(), "the fresh binders are distinct");
        assert!(
            new.iter().all(|l| !old.contains(l)),
            "no binder kept its old local"
        );
        let mentioned = mentioned(&body);
        assert!(
            old.iter().all(|l| !mentioned.contains(l)),
            "no read still mentions an old local"
        );
    }

    /// Each read of a binder follows its renamed binder: the loop body still reads its loop again.
    #[test]
    fn freshen_renames_reads_with_their_binder() {
        let mut m = Module::new();
        let x = m.local("x", TypeId::INT);
        let mut body = Expr::Loop {
            vars: vec![x],
            inits: vec![Expr::Lit(Lit::Int(0))],
            body: Box::new(add(Expr::Var(x), Expr::Lit(Lit::Int(1)))),
        };
        assert_eq!(freshen(&mut m, &mut body), 1);
        let Expr::Loop { vars, body, .. } = &body else {
            panic!()
        };
        assert_ne!(vars[0], x);
        assert_eq!(use_count(body, vars[0]), 1);
        assert_eq!(use_count(body, x), 0);
    }

    /// Two copies of the same body share no binder, so neither can rebind the other's reads.
    #[test]
    fn two_freshenings_share_no_binder() {
        let mut m = Module::new();
        let (old, body) = body_with_every_binder(&mut m);
        let mut first = body.clone();
        let mut second = body;
        freshen(&mut m, &mut first);
        freshen(&mut m, &mut second);
        let first_locals = mentioned(&first);
        let second_locals = mentioned(&second);
        assert!(first_locals.is_disjoint(&second_locals));
        assert!(
            old.iter()
                .all(|l| !first_locals.contains(l) && !second_locals.contains(l))
        );
    }

    /// A freshened body is still well typed, so the copy may be placed in a module.
    #[test]
    fn a_freshened_body_is_well_typed() {
        let mut m = Module::new();
        let (_, mut body) = body_with_every_binder(&mut m);
        freshen(&mut m, &mut body);
        let g = m.declare("g", false);
        m.define(g, Vec::new(), TypeId::INT, body);
        assert_eq!(crate::ir::verify::verify(&m), Ok(()));
    }

    /// `rename` renames reads only; a binder and an unlisted local are left alone.
    #[test]
    fn rename_renames_reads_and_leaves_binders_alone() {
        let mut m = Module::new();
        let x = m.local("x", TypeId::INT);
        let y = m.local("y", TypeId::INT);
        let z = m.local("z", TypeId::INT);
        let w = m.local("w", TypeId::INT);
        let mut e = let_(x, Expr::Var(y), Expr::Var(x));
        let map = FxHashMap::from_iter([(x, z), (y, w)]);
        rename(&mut e, &map);
        let Expr::Let { var, value, body } = &e else {
            panic!()
        };
        assert_eq!(*var, x, "the binder is not a read");
        assert_eq!(**value, Expr::Var(w));
        assert_eq!(**body, Expr::Var(z));
    }

    /// `fresh_like` copies the hint and the type of the original local.
    #[test]
    fn fresh_like_copies_hint_and_type() {
        let mut m = Module::new();
        let old = m.local("value", TypeId::INT);
        let new = fresh_like(&mut m, old);
        assert_ne!(new, old);
        assert_eq!(m.locals[new].hint, "value");
        assert_eq!(m.locals[new].ty, TypeId::INT);
    }
}
