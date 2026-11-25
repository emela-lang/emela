//! ζ reduction: substituting a let-bound value, and removing a dead trivial let.
//!
//! `let x = y in body` where `y` is a variable bound in the same lambda or a scalar literal
//! becomes `body` with every read of `x` replaced by `y`: reading a variable or a literal again
//! has no effect, so the binding bought nothing. `let x = v in body` where `x` is never read and
//! `v` is trivial is dropped: making the value is at most an allocation, and nothing reads it.
//!
//! The two rules are the glue between lowering, inlining and the fusions. Lowering binds every
//! source binding to a local and inlining binds every argument to a fresh local, so the IR arrives
//! at this pass as chains of `let` bindings around the work; removing them is what lets the later
//! passes see the shapes inside (a constructor next to its consumer, a `match` on a call the
//! source wrote) and what lets the escape analysis see that an aggregate does not leave its
//! function.
//!
//! # Contract
//!
//! ```text
//! Rule ID: zeta
//! Theory: let calculus (copy propagation) and weakening
//! Equation: let x = v in e → e[x := v]        when v is a variable or a scalar literal
//!           let x = v in e → e                when x is unused and v is trivial
//! Preconditions: for the first form, v is an `Expr::Var` or a scalar literal (`Lit::Int`,
//!                `Lit::Int64`, `Lit::Bool`, `Lit::Unit`, `Lit::Float`). A variable's top-level
//!                type carries no finalizer (see `has_finalizer`), and the variable is bound in
//!                the same lambda as the `let` (see "Captures and closures"; a variable bound
//!                outside that lambda is a free variable of it). For the second form, x has no
//!                read in e and v is trivial (subst::is_trivial), so evaluating it neither has an
//!                effect nor can fail, suspend or diverge.
//! Postconditions: the `let` is gone. Every read of the copied variable is the variable or the
//!                 literal it was bound to, and a debug build asserts that none is left; the body
//!                 of a dropped binding is the whole expression. A `let` whose value is a `Var`
//!                 bound outside its lambda stays as it is. The module stays well typed, and a
//!                 second run of the pipeline changes nothing.
//! Preserves: evaluation order, effects, failures, suspension points, drop points, and the order
//!            and count of the releases of `with`. A copied variable's type is one without a
//!            finalizer, which is what keeps its last drop where it was; a dropped trivial value
//!            at most allocates, so its memory use may change (`[dyn.memory]`).
//! Deopt: a value that is not a variable or a scalar literal (a call, builtin, constructor,
//!        lambda, operator, string literal, …) is not copied; an unused binding whose value is
//!        not trivial (a division or remainder that may raise a defect, `fail`, `assert`, a call
//!        that may have an effect or fail, …) stays. A variable whose top-level type is
//!        `Handler`, `Evidence`, `Cell`, `FiberScope` or `Any` is not copied, because copying it
//!        would move its last drop and so when its finalizer runs; that site is counted in
//!        `RuleStats::deopt`. A variable bound outside the `let`'s lambda is not copied either,
//!        because its reads would become reads of a free variable; that site is counted in
//!        `RuleStats::deopt` too. A string literal is left alone: copying it would repeat its
//!        text at every read.
//! Budget: non-increasing. A copy replaces one read with one variable or literal, and the two
//!         nodes of the removed `let` (`Let` and its value) disappear, so the expression strictly
//!         shrinks; a removed dead binding shrinks it too. Both are asserted in debug builds.
//! Measures: [`crate::opt::Stats::zeta`].
//! ```
//!
//! # Finalizer-bearing variables
//!
//! A variable read has no effect, but it is not always free to move: a value with a finalizer
//! runs that finalizer when its last reference is dropped. `let x = v in e` reads `v` once and
//! reads `x` where `e` does; through `x`, the value's last drop happens at the end of `x`'s
//! scope. Replacing every read of `x` by `v` drops it at the end of `v`'s scope instead, which
//! is a different time. Plan §3.1 preserves drop points, so a `Var` of a top-level type from the
//! finalizer set stays bound. Copying an aggregate is not inspected here: its own drop is a
//! deallocation and the elements' drops are the aggregate's, so a later restriction (or the
//! author's reading of §3.1) decides whether an aggregate whose elements carry a finalizer is a
//! copy value.
//!
//! # Captures and closures
//!
//! A `let` inside a lambda may copy a variable bound outside it: `let hi = n in e`, with `n` a
//! parameter of the enclosing function. That copy is sound, because `n` is already a free
//! variable of the lambda – the value side of the `let` reads it when the body starts – but it
//! changes what every read of `hi` costs. A read of a local of the lambda becomes a read of a
//! free variable. On the JavaScript target a free variable lives in a context object of the
//! enclosing function, so a loop that compares its index against the copied variable loads it
//! from that context on every iteration, while a local of the lambda stays in a register or on
//! the stack. The rule therefore copies `Expr::Var(w)` only when `w` is bound in the same lambda
//! as the `let`, or when neither is inside a lambda (the top level of a function).
//!
//! The effects benchmark ran 1.06x slower on JavaScript to exactly one such rewrite: the range
//! fusion writes `let hi = n` inside the lambda `State.run` receives, and ζ turned the loop bound
//! into a capture. matmul keeps the copies ζ has to remove, the argument bindings of an inlined
//! `get`, because they are in the same function as the `let`; without ζ it is 1.25x slower on
//! JavaScript, 1.63x on wasm and 1.32x on C. The C and wasm targets materialise every capture
//! into a local at closure entry, so the restriction changes their output only where it is
//! neutral.
//!
//! The restriction only refuses a rewrite. Where the rule fired before, it still fires unless
//! the value is a `Var` bound in an outer lambda, and in that case the `let` stays with its
//! original scope, evaluation order and drop points, so the soundness of the unrestrained rule
//! carries over. The dead-binding half is not restricted: it removes a binding nothing reads,
//! which cannot turn a read of a local into a read of a free variable. A refused copy is counted
//! in `RuleStats::deopt`, as a finalizer refusal is.
//!
//! # Status
//!
//! A2/PR-011 implements both rules on top of the substitution engine of A1/PR-010. The rule runs
//! only when `Rules::zeta` is set; A10/PR-019 turns it on in `Rules::default`. R1 adds the
//! capture-scoped precondition of "Captures and closures"; it refuses copies only.

use rustc_hash::FxHashMap;

use crate::ir::{Expr, Lit, Local, Module, Pat, Type, TypeId};
use crate::opt::{RuleStats, each_body};

use super::subst::{is_trivial, size, subst, use_count};

/// Propagates copies and removes dead bindings in every function body.
///
/// See the module docs for the contract. The pass allocates no locals: a copy reuses the value's
/// variable or literal, so the passes of the same sweep see the names lowering and inlining made.
pub(in crate::opt) fn zeta(m: &mut Module) -> RuleStats {
    let mut stats = RuleStats::default();
    each_body(m, &mut |m, body| {
        let owners = Owners::of(body);
        rewrite(m, body, &owners, 0, &mut stats);
    });
    stats
}

/// The lambda that binds each local of one function body.
///
/// A local is bound by one binder, and the binder is either inside some lambda of the body or at
/// its top level (a parameter of the function, or a `let`, pattern, loop or `with` binding outside
/// every lambda). The map records the nesting depth of the innermost lambda containing the binder;
/// a local that is not in the map is bound outside every lambda, which reads as depth 0.
///
/// One depth identifies one lambda: a local read at depth `d` is bound either by a binder of the
/// lambda at depth `d` (or by a `let` etc. in its body), or at a smaller depth, and two different
/// lambdas on the same path have different depths.
struct Owners {
    depth: FxHashMap<Local, u32>,
}

impl Owners {
    /// Collects the binder depth of every local that some lambda of `body` binds.
    fn of(body: &Expr) -> Owners {
        let mut depth = FxHashMap::default();
        collect_binders(body, 0, &mut depth);
        Owners { depth }
    }

    /// The depth of the lambda that binds `l`; 0 for a local bound outside every lambda.
    fn of_local(&self, l: Local) -> u32 {
        self.depth.get(&l).copied().unwrap_or(0)
    }
}

fn collect_binders(e: &Expr, depth: u32, owners: &mut FxHashMap<Local, u32>) {
    match e {
        Expr::Let { var, .. } => {
            owners.insert(*var, depth);
        }
        Expr::Lambda { params, .. } => {
            // The parameters are bound by the lambda, so they belong to the depth inside it.
            let inner = depth + 1;
            for &p in params {
                owners.insert(p, inner);
            }
            e.for_each_child(&mut |child| collect_binders(child, inner, owners));
            return;
        }
        Expr::Loop { vars, .. } => {
            for &v in vars {
                owners.insert(v, depth);
            }
        }
        Expr::With { binds, .. } => {
            for b in binds {
                owners.insert(b.var, depth);
            }
        }
        Expr::Match { arms, .. } | Expr::Escape { arms, .. } => {
            for arm in arms {
                collect_pat_binders(&arm.pat, depth, owners);
            }
        }
        _ => {}
    }
    e.for_each_child(&mut |child| collect_binders(child, depth, owners));
}

fn collect_pat_binders(p: &Pat, depth: u32, owners: &mut FxHashMap<Local, u32>) {
    match p {
        Pat::Bind(l) => {
            owners.insert(*l, depth);
        }
        Pat::Ctor { fields, .. } => {
            for f in fields {
                collect_pat_binders(f, depth, owners);
            }
        }
        Pat::Tuple(ps) => {
            for p in ps {
                collect_pat_binders(p, depth, owners);
            }
        }
        Pat::List { elems, rest } => {
            for p in elems {
                collect_pat_binders(p, depth, owners);
            }
            if let Some(rest) = rest {
                collect_pat_binders(rest, depth, owners);
            }
        }
        Pat::Wild | Pat::Lit(_) => {}
    }
}

/// Rewrites the `let`s in `e`, children first, and then `e` itself if it is one.
///
/// A `let` is decided only after its value and body are rewritten, so the body the rules read is
/// the final one: a copy inside it has already been propagated away, and a nested dead binding
/// has already been dropped. Each rewrite removes one `let` and adds only variables and literals
/// ([`is_copy_value`]), so the traversal terminates.
///
/// `depth` is the number of lambdas enclosing `e`; a `let` copies a variable only when the
/// variable's binder has the same depth (see [`Owners`]).
fn rewrite(m: &mut Module, e: &mut Expr, owners: &Owners, depth: u32, stats: &mut RuleStats) {
    let inner = if matches!(e, Expr::Lambda { .. }) {
        depth + 1
    } else {
        depth
    };
    crate::tail::for_each_child_mut(e, &mut |child| rewrite(m, child, owners, inner, stats));
    let before = size(e);
    let Expr::Let { var, value, body } = e else {
        return;
    };
    let var = *var;
    let propagated = is_copy_value(m, value, owners, depth);
    let dead = is_trivial(value) && use_count(body, var) == 0;
    if !propagated && !dead {
        // A copy the rule had to refuse is a deopt the reader should see; a value that is simply
        // not a copy (`let point = Point(…)`) is not a candidate at all.
        if is_refused_copy(m, value, owners, depth) {
            stats.deopt += 1;
        }
        return;
    }
    if propagated {
        let with = (**value).clone();
        let done = subst(m, body, var, &with);
        if done.deopt {
            stats.deopt += 1;
            return;
        }
        debug_assert_eq!(
            use_count(body, var),
            0,
            "copy propagation left a read of the local it removed (a compiler bug)"
        );
    }
    // `let x = value; body` evaluated `value` and then `body`. Both rules have established that
    // what `value` did is unobservable: the expression is `body`, wherever the `let` was.
    let body = std::mem::replace(body.as_mut(), Expr::Lit(Lit::Unit));
    let after = size(&body);
    if propagated {
        debug_assert!(
            after <= before,
            "copy propagation grew the expression ({after} > {before} nodes)"
        );
    } else {
        debug_assert!(
            after < before,
            "removing a dead binding did not shrink the expression ({after} >= {before} nodes)"
        );
    }
    *e = body;
    stats.applied += 1;
}

/// Whether `let x = value` may become `value` in place of every read of `x`.
///
/// A scalar literal evaluation has no effect, cannot fail, cannot suspend and cannot raise a
/// defect, so repeating one where a read of `x` was is unobservable. A variable read has the same
/// properties, but it is not always a local read: a variable bound outside the `let`'s lambda is a
/// free variable there, and copying it would make every read of `x` a read of that free variable
/// instead of a read of a local (see "Captures and closures"). It may also move the value's last
/// drop, so a variable whose type carries a finalizer is not a copy value ([`has_finalizer`]). A
/// string literal is excluded: copying it would repeat the text at every read, and the
/// duplication is what substitution does.
fn is_copy_value(m: &Module, e: &Expr, owners: &Owners, depth: u32) -> bool {
    match e {
        Expr::Var(v) => !has_finalizer(m, m.locals[*v].ty) && owners.of_local(*v) == depth,
        Expr::Lit(Lit::Int(_) | Lit::Int64(_) | Lit::Bool(_) | Lit::Unit | Lit::Float(_)) => true,
        _ => false,
    }
}

/// Whether `value` is a variable that [`is_copy_value`] refused, for its type or because it is a
/// free variable of the `let`'s lambda.
fn is_refused_copy(m: &Module, value: &Expr, owners: &Owners, depth: u32) -> bool {
    matches!(value, Expr::Var(v)
        if has_finalizer(m, m.locals[*v].ty) || owners.of_local(*v) != depth)
}

/// Whether a value of `ty` owns a finalizer, or an untracked representation, which the pass must
/// not move.
///
/// `Handler` owns the handler value, `Evidence` the table it points at, `Cell` its contents,
/// `FiberScope` the scope it opens, and `Any` is not tracked by a type at all. Dropping one of
/// them runs that finalizer, so the last drop of such a value is a point the pass keeps where it
/// is (plan §3.1). The check reads the top-level type only; an aggregate's elements are not
/// inspected (see the module docs).
fn has_finalizer(m: &Module, ty: TypeId) -> bool {
    matches!(
        m.ty(ty),
        Type::Handler(_) | Type::Evidence | Type::Cell(_) | Type::FiberScope | Type::Any
    )
}
