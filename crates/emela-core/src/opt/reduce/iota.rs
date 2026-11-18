//! ι reduction: matching a value whose constructor is already known.
//!
//! `match V { arms }`, where `V` is a constructor or a tuple written at the scrutinee, selects its
//! arm before the program runs. The pass replaces the `match` by that arm's body, with the
//! arguments of `V` bound to the names the pattern gives them. A field the pattern ignores is
//! still evaluated, into a fresh local, so the rewrite keeps the evaluation of the scrutinee.
//!
//! ```text
//! match Some(f()) { Some(x) -> body; None -> … }   becomes   let x = f(); body
//! match (f(), g()) { (x, _) -> body }              becomes   let x = f(); let t = g(); body
//! match V { x -> body }                            becomes   let x = V; body
//! ```
//!
//! A nested constructor in the pattern is destructured in place: its own arguments are evaluated
//! once each, in order, and the allocations of the constructors on the way to the names are not
//! made. That changes memory use only (`[dyn.memory]`).
//!
//! The pass is top-down: it reduces a `match`, then recurses into the arm body it left, so a
//! `match` inside the selected arm or in a bound argument is reduced by the same run.
//!
//! # Float literal patterns
//!
//! A pattern with a `Lit::Float` field is never compared and always deopts the match. The
//! specification matches a Float literal with `==` (spec `[fn.match]`), so `0.0` matches
//! `-0.0` and a NaN pattern matches no value. A comparison by `f64::to_bits` would get both cases
//! wrong, and `f64` equality is not a total relation (`NaN != NaN`), so the rule refuses the
//! pattern instead of choosing a comparison that the run time might not share. Matching a Float
//! literal therefore stays at run time; a pattern without one reduces as usual.
//!
//! # Depth budget
//!
//! [`classify`] and [`plan`] recurse over the pattern. A compiler-generated pattern can be deeper
//! than the source ever writes, so the pass stops at [`MAX_PATTERN_DEPTH`] and deopts instead of
//! overflowing the stack.
//!
//! # Allocation on deopt
//!
//! A pattern is only planned (which allocates the fresh local of a `Pat::Wild` field) after
//! [`select`] has chosen the arm, so a deopt of the arm selection allocates nothing. `plan` then
//! fails only on malformed IR (a constructor whose field count or tuple type does not match its
//! arguments), which well-typed IR does not have; such a deopt may leave the locals it had
//! already allocated in the arena. `dump` and `verify` ignore them, and the rule's tests do not
//! compare arena sizes.
//!
//! # Contract
//!
//! ```text
//! Rule ID: iota
//! Theory: case-of-known-constructor
//! Equation: match K(v) { … K(x) -> e … } → e[x := v]
//! Preconditions: the scrutinee is a constructor or a tuple written at the `match`; the arms above
//!                the selected one do not match it (another constructor or variant, another
//!                literal), the selected arm matches it, and that arm has no guard. Its pattern
//!                is made of `Pat::Wild`, `Pat::Bind`, `Pat::Lit` and constructors and tuples of
//!                them, is at most `MAX_PATTERN_DEPTH` deep, and the patterns the rule has to
//!                decide contain no Float literal.
//! Postconditions: the value is the selected arm's body. Each argument of the scrutinee is
//!                 evaluated once, left to right, into the local its field pattern binds, or into
//!                 a fresh local when the pattern ignores the field. A field the pattern compares
//!                 with a literal is a literal, so it need not be bound. A catch-all `Pat::Bind`
//!                 becomes `let x = V; body`; a catch-all `Pat::Wild` becomes `body` when `V` is
//!                 trivial and `let t = V; body` otherwise. The `match` and the constructors on
//!                 the way to the bound fields are not made. A second run of the pipeline changes
//!                 nothing.
//! Preserves: evaluation order, effects, failures, suspension points, and drop points. The arms
//!            above the selected one are not evaluated, as before, and the selected arm has no
//!            guard to evaluate. The aggregate the pattern takes apart is not allocated, so memory
//!            use changes, within `[dyn.memory]`.
//! Deopt: a scrutinee that is not a constructor or tuple written at the `match` (a `Var`, a call,
//!        `Array.get`, …; the Option fusion owns `match Array.get`) is not a candidate and is not
//!        counted. A candidate the rule cannot decide is a deopt: an arm that may match without
//!        being known to, a selected arm with a guard, a Float literal pattern, a pattern deeper
//!        than `MAX_PATTERN_DEPTH`, and a `match` no arm selects.
//! Budget: the selected arm's body is copied once; the pattern depth is at most
//!         `MAX_PATTERN_DEPTH`, so the recursion is bounded. The rewrite never grows the program.
//! Measures: [`crate::opt::Stats::iota`]; the `Option` and record destructuring that inlining
//!           exposes in the caller (`nbody`, `fannkuch`).
//! ```
//!
//! # Status
//!
//! Implemented by A5/PR-014. The pass runs after inlining ([`crate::opt::run_with`]), which is what
//! puts the constructors a callee built where the caller matches them. Float literal patterns are
//! deopted rather than compared (see above), and a pattern deeper than [`MAX_PATTERN_DEPTH`] is
//! left to run time.

use crate::ir::{Arm, CtorRef, Expr, Lit, Local, Module, Pat, Type, TypeId};
use crate::opt::{RuleStats, each_body, reduce::subst};

/// The deepest pattern nesting the rule descends into before it deopts.
///
/// The depth is the number of constructor and tuple patterns on the path from the arm's pattern to
/// the field being visited, starting at zero for the arm's own pattern. A source pattern is far
/// shallower; the budget is a guard against a compiler-generated one overflowing the stack.
const MAX_PATTERN_DEPTH: usize = 64;

/// Reduces every `match` whose scrutinee is a known constructor or tuple to its selected arm.
pub(in crate::opt) fn iota(m: &mut Module) -> RuleStats {
    let mut stats = RuleStats::default();
    each_body(m, &mut |m, body| rewrite(m, body, &mut stats));
    stats
}

/// What [`reduce`] decided about one expression.
enum Reduction {
    /// The expression is the selected arm's body, with the scrutinee's arguments bound.
    Reduced(Expr),
    /// The expression is a `match` of a known constructor whose arm cannot be selected.
    Deopt,
    /// The expression is not a candidate (including a `match` of an unknown scrutinee).
    Keep,
}

/// Reduces `e` and then visits what the reduction left, so a `match` inside the selected arm or in
/// a bound argument is reduced by the same run. The direction is load-bearing; do not make this
/// bottom-up.
fn rewrite(m: &mut Module, e: &mut Expr, stats: &mut RuleStats) {
    match reduce(m, e) {
        Reduction::Reduced(reduced) => {
            *e = reduced;
            stats.applied += 1;
        }
        Reduction::Deopt => stats.deopt += 1,
        Reduction::Keep => {}
    }
    crate::tail::for_each_child_mut(e, &mut |child| rewrite(m, child, stats));
}

/// The selected arm's body, with the scrutinee's arguments bound, or a decision to leave the
/// `match`.
fn reduce(m: &mut Module, e: &Expr) -> Reduction {
    let Expr::Match { scrutinee, arms } = e else {
        return Reduction::Keep;
    };
    // v1: only a constructor or tuple written at the scrutinee. A `let`-bound scrutinee is left to
    // the LIR's scalar replacement of aggregates, and `match Array.get` is the Option fusion's.
    if !matches!(&**scrutinee, Expr::Ctor { .. } | Expr::Tuple(_)) {
        return Reduction::Keep;
    }
    let Some(selected) = select(scrutinee, arms) else {
        return Reduction::Deopt;
    };
    let arm = &arms[selected];
    // A catch-all that ignores its value needs no local when evaluating the value has no effect.
    if matches!(arm.pat, Pat::Wild) && subst::is_trivial(scrutinee) {
        return Reduction::Reduced(arm.body.clone());
    }
    // The arm is known to be selected, so the only work left is to bind its fields.
    let ty = scrutinee_type(m, scrutinee);
    let mut bindings = Vec::new();
    if !plan(m, &arm.pat, scrutinee, ty, 0, &mut bindings) {
        return Reduction::Deopt;
    }
    let mut body = arm.body.clone();
    // The first visited field is the outermost `let`, so the arguments are evaluated in the order
    // the pattern visits them (left to right).
    for (var, value) in bindings.into_iter().rev() {
        body = Expr::Let {
            var,
            value: Box::new(value),
            body: Box::new(body),
        };
    }
    Reduction::Reduced(body)
}

/// The index of the arm a known scrutinee selects, or `None` to leave the `match`.
///
/// Arms are tried top to bottom. An arm that does not match is skipped, even when it has a guard:
/// its guard is not evaluated either. The first arm that matches selects, when it has no guard.
/// An arm that might match stops the reduction, because a later, known arm might not be reached.
fn select(scrutinee: &Expr, arms: &[Arm]) -> Option<usize> {
    for (index, arm) in arms.iter().enumerate() {
        match classify(&arm.pat, scrutinee, 0) {
            Verdict::No => {}
            Verdict::Unknown => return None,
            Verdict::Yes => {
                if arm.guard.is_some() {
                    return None;
                }
                return Some(index);
            }
        }
    }
    // The value matches no arm: the `match` is a defect at run time, which the IR keeps (the type
    // checker normally makes the arms exhaustive).
    None
}

/// Whether a pattern matches a known value, does not match it, or cannot be told.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// The pattern does not match: the value selects a later arm.
    No,
    /// The pattern matches: the value selects this arm.
    Yes,
    /// The pattern may match: the reduction cannot decide without running the program.
    Unknown,
}

/// `depth` is the nesting of `pat` inside the arm's pattern; see [`MAX_PATTERN_DEPTH`].
fn classify(pat: &Pat, value: &Expr, depth: usize) -> Verdict {
    if depth > MAX_PATTERN_DEPTH {
        return Verdict::Unknown;
    }
    match pat {
        Pat::Wild | Pat::Bind(_) => Verdict::Yes,
        // A Float literal is matched with `==` by the specification, which `to_bits` would get
        // wrong for the signed zero and for NaN. Do not compare it; leave the match at run time.
        Pat::Lit(Lit::Float(_)) => Verdict::Unknown,
        Pat::Lit(lit) => match value {
            Expr::Lit(value) if value == lit => Verdict::Yes,
            Expr::Lit(_) => Verdict::No,
            _ => Verdict::Unknown,
        },
        Pat::Ctor { ctor, fields } => {
            let Expr::Ctor {
                ctor: value_ctor,
                args,
                ..
            } = value
            else {
                return Verdict::Unknown;
            };
            if ctor != value_ctor {
                return Verdict::No;
            }
            if fields.len() != args.len() {
                return Verdict::Unknown;
            }
            classify_fields(fields, args, depth)
        }
        Pat::Tuple(patterns) => {
            let Expr::Tuple(values) = value else {
                return Verdict::Unknown;
            };
            if patterns.len() != values.len() {
                return Verdict::Unknown;
            }
            classify_fields(patterns, values, depth)
        }
        // A list pattern compares the spine of a list, which is not a known constructor here.
        Pat::List { .. } => Verdict::Unknown,
    }
}

/// The verdict on the fields of a constructor or tuple pattern, one level below `depth`.
fn classify_fields(patterns: &[Pat], values: &[Expr], depth: usize) -> Verdict {
    let mut verdict = Verdict::Yes;
    for (pattern, value) in patterns.iter().zip(values) {
        match classify(pattern, value, depth + 1) {
            // A field that does not match decides the whole pattern, whatever another field would
            // have said.
            Verdict::No => return Verdict::No,
            Verdict::Unknown => verdict = Verdict::Unknown,
            Verdict::Yes => {}
        }
    }
    verdict
}

/// Collects the `let` bindings that evaluate `value` once, in the order the pattern visits it.
///
/// Returns false only if the pattern and the value do not have the shape [`classify`] accepted
/// (malformed IR reaches that). See the module docs for the locals a failure may leave behind.
fn plan(
    m: &mut Module,
    pat: &Pat,
    value: &Expr,
    ty: TypeId,
    depth: usize,
    bindings: &mut Vec<(Local, Expr)>,
) -> bool {
    if depth > MAX_PATTERN_DEPTH {
        return false;
    }
    match pat {
        Pat::Wild => {
            let local = subst::fresh(m, "t", ty);
            bindings.push((local, value.clone()));
            true
        }
        Pat::Bind(local) => {
            bindings.push((*local, value.clone()));
            true
        }
        // The value is a literal the pattern matched; evaluating it again has no effect.
        Pat::Lit(_) => true,
        Pat::Ctor { ctor, fields } => {
            let Expr::Ctor {
                ctor: value_ctor,
                ty_args,
                args,
            } = value
            else {
                return false;
            };
            if ctor != value_ctor || fields.len() != args.len() {
                return false;
            }
            let tys = field_types(m, *value_ctor, ty_args);
            if tys.len() != args.len() {
                return false;
            }
            for ((field, arg), ty) in fields.iter().zip(args).zip(tys) {
                if !plan(m, field, arg, ty, depth + 1, bindings) {
                    return false;
                }
            }
            true
        }
        Pat::Tuple(patterns) => {
            let Expr::Tuple(values) = value else {
                return false;
            };
            let Type::Tuple(tys) = m.ty(ty).clone() else {
                return false;
            };
            if patterns.len() != values.len() || tys.len() != values.len() {
                return false;
            }
            for ((pattern, value), ty) in patterns.iter().zip(values).zip(tys) {
                if !plan(m, pattern, value, ty, depth + 1, bindings) {
                    return false;
                }
            }
            true
        }
        Pat::List { .. } => false,
    }
}

/// The types of `ctor`'s fields with its type arguments substituted.
fn field_types(m: &mut Module, ctor: CtorRef, ty_args: &[TypeId]) -> Vec<TypeId> {
    let tys = m.variant(ctor).tys.clone();
    tys.into_iter()
        .map(|ty| {
            if ty_args.is_empty() {
                ty
            } else {
                m.types.subst(ty, ty_args)
            }
        })
        .collect()
}

/// The type of the scrutinee, for a fresh local of a catch-all `Pat::Wild` and for the field
/// types of a tuple pattern.
fn scrutinee_type(m: &mut Module, scrutinee: &Expr) -> TypeId {
    match scrutinee {
        Expr::Ctor { ctor, ty_args, .. } => m.enum_type(ctor.enum_id, ty_args.clone()),
        // A tuple's type is not written on the expression: ask the typer, and keep the types it
        // interns so that a local can have one.
        _ => value_type(m, scrutinee),
    }
}

/// The type of `e`, adding the types the typer had to intern to the module.
fn value_type(m: &mut Module, e: &Expr) -> TypeId {
    let mut typer = crate::Typer::new(m);
    let ty = typer.type_of(e);
    if let Some(types) = typer.into_types() {
        m.types = types;
    }
    ty
}
