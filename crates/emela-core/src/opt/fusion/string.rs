//! String builder fusion: a chain of `String.append` calls into one `concat`.
//!
//! `String.append` copies both operands into a new String, so
//! `String.append(String.append(a, b), c)` copies `a` and `b` once into an intermediate string
//! and then copies that string and `c` again. The chain can be one `Expr::Concat` instead: the
//! LIR lowering already builds a three-or-more-part interpolation as one `RuntimeOp::Concat`,
//! whose runtime adds up the byte lengths, allocates one String and copies every part into it
//! (the `from_core::concat` module of `emela-lir`).
//!
//! The leaves of the append tree are collected left to right. An append evaluates its left
//! operand before its right one, so the left-to-right order of the leaves is the evaluation order
//! of the original expression, whatever the tree's association. The leaves are kept as
//! `StrPart::Value` even when one is a string literal: the LIR's own normalization merges
//! adjacent texts and drops empty ones for interpolation, and this law does not add or remove
//! any other shortcut.
//!
//! A chain of two leaves is left alone: one `String.append` already copies once and allocates
//! once, and interpolation lowers two pieces to one append for the same reason. A chain whose
//! leaves are all the empty string literal is also left alone: the LIR normalization would turn
//! it into the literal empty string, and the native target rejects a constant empty `String.split`
//! separator at compile time while a separator that is empty at run time is a defect (or a split
//! by character). That is the same reason the algebraic reduction does not fold an `if` whose
//! taken arm is the literal empty string.
//!
//! This is the body of the registry entry [`LawId::StringConcat`]; the registry's flags, order
//! and budget are in [`super`], and the contract text of the law is beside its entry in
//! [`super::law`].

use crate::intrinsics::Builtin;
use crate::ir::{Expr, Lit, Module, StrPart, TypeId};
use crate::opt::{CoreFns, PassStats, each_body};

use super::law::LawRun;

/// Rewrites a chain of three or more `String.append` operands to one `Expr::Concat`.
///
/// Preconditions: every leaf of the append tree is a String and the tree has at least three
/// leaves; not every leaf is the empty string literal.
/// Postconditions: the result is one `Expr::Concat` whose value parts are the leaves in
/// left-to-right order; the intermediate appends and their strings are not made.
/// Preserves: evaluation order, effects, failures, suspension points and drop points of the
/// leaves, and the byte content of the result.
/// Deopt: a single append, a two-leaf chain, and a chain whose leaves are all the empty string
/// literal are left alone.
/// Measures: the applied count of `fusion::run` and one String per site in the C allocation
/// report.
pub(super) fn concat(m: &mut Module, _core_fns: &CoreFns) -> LawRun {
    let mut changed = 0;
    each_body(m, &mut |_, e| rewrite(e, &mut changed));
    LawRun {
        applied: changed,
        // The registry has no `Stats` field for this law yet: the applied count is its report.
        stats: PassStats::default(),
    }
}

/// Rewrites `e` if it is the root of a long enough append tree, then its children.
///
/// The rewrite is top-down: the outermost append tree is flattened in one step, so an inner
/// append node is never counted separately. Recursing into the replacement reaches an append
/// chain inside a leaf (for example inside a `let` body), which a second run would otherwise
/// have to find, so the law is idempotent.
fn rewrite(e: &mut Expr, changed: &mut u32) {
    if let Some(fused) = fuse(e) {
        *e = fused;
        *changed += 1;
    }
    crate::tail::for_each_child_mut(e, &mut |c| rewrite(c, changed));
}

/// The one concat for the append tree at `e`, or `None` if `e` is not a long enough chain.
fn fuse(e: &Expr) -> Option<Expr> {
    let mut leaves = Vec::new();
    flatten(e, &mut leaves);
    if leaves.len() < 3 || leaves.iter().all(is_empty_literal) {
        return None;
    }
    Some(Expr::Concat(
        leaves
            .into_iter()
            .map(|leaf| StrPart::Value(leaf, TypeId::STRING))
            .collect(),
    ))
}

/// Collects the leaves of the `String.append` tree at `e`, in evaluation order.
fn flatten(e: &Expr, leaves: &mut Vec<Expr>) {
    if let Expr::Builtin {
        op: Builtin::StringAppend,
        ty_args,
        args,
    } = e
    {
        // The intrinsic takes no type arguments; a call of it always has exactly two arguments.
        if ty_args.is_empty() && args.len() == 2 {
            flatten(&args[0], leaves);
            flatten(&args[1], leaves);
            return;
        }
    }
    leaves.push(e.clone());
}

/// Whether `e` is the literal empty string.
fn is_empty_literal(e: &Expr) -> bool {
    matches!(e, Expr::Lit(Lit::String(s)) if s.is_empty())
}
