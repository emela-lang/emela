//! Use counts of locals.
//!
//! # Contract
//!
//! ```text
//! Analysis ID: AnalysisId::Uses
//! Domain / lattice: a partial map from Local to a count. `Unknown` is the top element; a map that
//!                   omits a local says the local has no use.
//! Transfer: count every occurrence of a local as a value, a callee, an argument, a field base, a
//!           pattern guard's free variable, a release body and so on; a binder is not a use.
//! Soundness: a count is the number of evaluations of the local's value on the paths the analysis
//!            covers, and never under-counts a use of a local in the function.
//! Deopt (unknown): a consumer that gets `Unknown` does not remove or duplicate a binding. In
//!                  particular, the β reduction does not assume a function is linear.
//! Consumers: β and δ reduction (linearity), dead-let and dead-code removal, the bounds and CSE
//!            rules.
//! ```
//!
//! # What is a use
//!
//! A use is an [`Expr::Var`] read. A binder is not a use: the variable of [`Expr::Let`], the
//! parameters of [`Expr::Lambda`], the variables of [`Expr::Loop`], the variables of
//! [`WithBind`](crate::ir::WithBind) and the local of [`Pat::Bind`](crate::ir::Pat::Bind) bind a
//! name without reading it. Patterns contain no expressions, so the walk over a `Match` only
//! reaches a guard and a body, which is where a pattern-bound local is read.
//!
//! A read inside a nested lambda, inside a `with` release, or inside any other child expression
//! still reads the local, so it is counted: the analysis answers "how many reads are in this
//! function's tree", not "how many reads happen on one path". The count is therefore an upper
//! bound on the number of evaluations of the value, which is what linearity needs.
//!
//! # Order
//!
//! [`Expr::for_each_child`] visits a `With` binding's value, then its release body, then the
//! `with` body; [`crate::tail::for_each_child_mut`] uses the same order. The release of a `with`
//! runs after the body, so the traversal is not an evaluation order. A use count does not depend
//! on the order of the reads (it only adds one per read), so the two facts do not conflict. Any
//! consumer that needs evaluation order reads the IR directly or uses a different analysis.
//!
//! # Status
//!
//! B2/PR-021 implements [`of_function`], [`of_module`] and [`of_expr`]. The [`Analysis`] hook
//! [`analyze`] stays conservative for now: the cache is not wired into a pass yet, and the
//! concrete functions are what the β and δ rules call.

use rustc_hash::FxHashMap;

use crate::ir::{Expr, FnId, Local, Module};

use super::{Analysis, AnalysisId, Fact};

/// The number of uses of each local of one function.
pub type UseCounts = FxHashMap<Local, u32>;

/// The use-count analysis.
pub struct Uses;

impl Analysis for Uses {
    type Output = Fact<UseCounts>;

    const ID: AnalysisId = AnalysisId::Uses;

    fn analyze(&self, _m: &Module) -> Self::Output {
        Fact::Unknown
    }
}

/// The use counts of `m`, or `Unknown` (see the module contract).
pub fn analyze(m: &Module) -> Fact<UseCounts> {
    Uses.analyze(m)
}

/// The number of reads of `local` in `e`.
///
/// This is the expression-level transfer function of the analysis, shared with substitution:
/// [`crate::opt::reduce::subst::use_count`] counts the same reads.
pub fn of_expr(e: &Expr, local: Local) -> u32 {
    crate::opt::reduce::subst::use_count(e, local)
}

/// The reads of each local in the body of `f`.
///
/// The map omits a local with no read, so a missing entry means zero. Locals are unique in a
/// module, so entries of different functions cannot be confused with one another.
pub fn of_function(m: &Module, f: FnId) -> UseCounts {
    let mut counts = UseCounts::default();
    count_reads(&m.functions[f].body, &mut counts);
    counts
}

/// The use counts of every function of `m`, keyed by function.
pub fn of_module(m: &Module) -> FxHashMap<FnId, UseCounts> {
    m.functions
        .iter()
        .map(|(f, _)| (f, of_function(m, f)))
        .collect()
}

/// Adds one to the count of every read under `e`.
///
/// Bindings are not visited: only [`Expr::Var`] is a read, and a binder is never an `Expr`. The
/// walk goes into every child, including nested lambdas and `with` releases.
fn count_reads(e: &Expr, counts: &mut UseCounts) {
    if let Expr::Var(local) = e {
        *counts.entry(*local).or_insert(0) += 1;
        return;
    }
    e.for_each_child(&mut |child| count_reads(child, counts));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{
        Arm, Assert, Callee, Expr, Lit, Pat, Release, StrPart, TypeId, UnOp, WithBind,
    };

    /// A function with an empty parameter list and `body`.
    fn function(m: &mut Module, name: &str, body: Expr) -> FnId {
        let f = m.declare(name, false);
        m.define(f, Vec::new(), TypeId::UNIT, body);
        f
    }

    /// A read of `local`.
    fn var(local: Local) -> Expr {
        Expr::Var(local)
    }

    /// `a + b` on Int.
    fn add(lhs: Expr, rhs: Expr) -> Expr {
        Expr::Binary {
            op: crate::ir::BinOp::Add,
            ty: crate::ir::OpTy::Int,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        }
    }

    /// A let-bound local read twice is counted twice, and the read of the value is not a use of
    /// the bound local.
    #[test]
    fn a_let_bound_local_read_twice_is_counted_twice() {
        let mut m = Module::new();
        let x = m.local("x", TypeId::INT);
        let f = function(
            &mut m,
            "f",
            Expr::Let {
                var: x,
                value: Box::new(Expr::Lit(Lit::Int(1))),
                body: Box::new(add(var(x), var(x))),
            },
        );
        let counts = of_function(&m, f);
        assert_eq!(counts.get(&x), Some(&2));
        assert_eq!(of_expr(&m.functions[f].body, x), 2);
    }

    /// A lambda parameter is read in the lambda's body; the parameter itself is a binder.
    #[test]
    fn a_lambda_parameter_read_once_is_counted_once() {
        let mut m = Module::new();
        let p = m.local("p", TypeId::INT);
        let ty = m.fn_type(vec![TypeId::INT], TypeId::INT);
        let lambda = m.local("lambda", ty);
        let f = function(
            &mut m,
            "f",
            Expr::Let {
                var: lambda,
                value: Box::new(Expr::Lambda {
                    params: vec![p],
                    ty,
                    body: Box::new(add(var(p), Expr::Lit(Lit::Int(1)))),
                    suspends: false,
                }),
                body: Box::new(var(lambda)),
            },
        );
        let counts = of_function(&m, f);
        assert_eq!(counts.get(&p), Some(&1));
        assert_eq!(counts.get(&lambda), Some(&1));
    }

    /// A loop variable is read in the body and in `recur`; the declaration in `vars` is not a
    /// read.
    #[test]
    fn a_loop_variable_is_counted_in_the_body_and_in_recur() {
        let mut m = Module::new();
        let i = m.local("i", TypeId::INT);
        let f = function(
            &mut m,
            "f",
            Expr::Loop {
                vars: vec![i],
                inits: vec![Expr::Lit(Lit::Int(0))],
                body: Box::new(add(var(i), Expr::Recur(vec![var(i)]))),
            },
        );
        let counts = of_function(&m, f);
        assert_eq!(counts.get(&i), Some(&2));
    }

    /// A pattern binding is not a read, but a read of the bound local in the arm's body is.
    #[test]
    fn a_pattern_binding_read_once_is_counted_once() {
        let mut m = Module::new();
        let x = m.local("x", TypeId::INT);
        let f = function(
            &mut m,
            "f",
            Expr::Match {
                scrutinee: Box::new(Expr::Lit(Lit::Int(1))),
                arms: vec![Arm {
                    pat: Pat::Bind(x),
                    guard: None,
                    body: var(x),
                }],
            },
        );
        let counts = of_function(&m, f);
        assert_eq!(counts.get(&x), Some(&1));
    }

    /// A `with` release body may read a local bound outside the `with`, and the `with` variable is
    /// itself a binder.
    #[test]
    fn a_with_release_counts_a_read_of_an_outer_local() {
        let mut m = Module::new();
        let outer = m.local("outer", TypeId::INT);
        let handler = m.local("handler", TypeId::UNIT);
        let f = function(
            &mut m,
            "f",
            Expr::With {
                binds: vec![WithBind::managed(
                    handler,
                    Expr::Lit(Lit::Unit),
                    "h",
                    var(outer),
                )],
                body: Box::new(var(handler)),
            },
        );
        let counts = of_function(&m, f);
        assert_eq!(counts.get(&outer), Some(&1));
        assert_eq!(counts.get(&handler), Some(&1));
    }

    /// A local that is bound and never read has no entry, which means zero.
    #[test]
    fn a_dead_local_has_count_zero() {
        let mut m = Module::new();
        let dead = m.local("dead", TypeId::INT);
        let f = function(
            &mut m,
            "f",
            Expr::Let {
                var: dead,
                value: Box::new(Expr::Lit(Lit::Int(1))),
                body: Box::new(Expr::Lit(Lit::Unit)),
            },
        );
        let counts = of_function(&m, f);
        assert_eq!(counts.get(&dead), None);
        assert_eq!(counts.get(&dead).copied().unwrap_or(0), 0);
        assert_eq!(of_expr(&m.functions[f].body, dead), 0);
    }

    /// Locals are unique, so a binder of the same name in another function is another key.
    #[test]
    fn the_same_binder_name_in_two_functions_stays_separate() {
        let mut m = Module::new();
        let x1 = m.local("x", TypeId::INT);
        let x2 = m.local("x", TypeId::INT);
        let f = function(
            &mut m,
            "f",
            Expr::Let {
                var: x1,
                value: Box::new(Expr::Lit(Lit::Int(1))),
                body: Box::new(var(x1)),
            },
        );
        let g = function(
            &mut m,
            "g",
            Expr::Let {
                var: x2,
                value: Box::new(Expr::Lit(Lit::Int(2))),
                body: Box::new(add(var(x2), var(x2))),
            },
        );
        let all = of_module(&m);
        assert_eq!(all.get(&f).unwrap().get(&x1), Some(&1));
        assert_eq!(all.get(&f).unwrap().get(&x2), None);
        assert_eq!(all.get(&g).unwrap().get(&x2), Some(&2));
        assert_eq!(all.get(&g).unwrap().get(&x1), None);
    }

    /// Every child is walked, including a lambda body, an assert operand and a string part. This
    /// guards the walk against missing one expression variant.
    #[test]
    fn a_read_in_every_child_position_is_counted() {
        let mut m = Module::new();
        let x = m.local("x", TypeId::INT);
        let ty = m.fn_type(vec![TypeId::INT], TypeId::INT);
        let f = function(
            &mut m,
            "f",
            Expr::Tuple(vec![
                var(x),
                Expr::Lambda {
                    params: Vec::new(),
                    ty,
                    body: Box::new(var(x)),
                    suspends: false,
                },
                Assert::bool(var(x), "x"),
                Expr::Concat(vec![
                    StrPart::Lit("x=".to_owned()),
                    StrPart::Value(var(x), TypeId::INT),
                ]),
                Expr::Unary {
                    op: UnOp::Not,
                    ty: crate::ir::OpTy::Bool,
                    operand: Box::new(var(x)),
                },
                Expr::Call {
                    callee: Callee::Indirect(Box::new(var(x))),
                    args: Vec::new(),
                    fails: false,
                    suspends: false,
                },
            ]),
        );
        let counts = of_function(&m, f);
        assert_eq!(counts.get(&x), Some(&6));
    }

    /// A hand-built release and a `Pat` binding are not `Expr::Var`, so no variant of the walk
    /// treats a binder as a read.
    #[test]
    fn a_binder_is_not_a_read_in_any_position() {
        let mut m = Module::new();
        let x = m.local("x", TypeId::INT);
        let y = m.local("y", TypeId::INT);
        let bind = WithBind {
            var: y,
            value: Expr::Lit(Lit::Unit),
            release: Some(Release {
                handler: "h".to_owned(),
                body: Expr::Lit(Lit::Unit),
            }),
        };
        let f = function(
            &mut m,
            "f",
            Expr::With {
                binds: vec![bind],
                body: Box::new(var(x)),
            },
        );
        let counts = of_function(&m, f);
        assert_eq!(counts.get(&x), Some(&1));
        assert_eq!(counts.get(&y), None);
    }
}
