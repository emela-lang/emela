//! Reordering named arguments.
//!
//! IR arguments are in positional order, but evaluation follows written order. When the two
//! differ, arguments that may have side effects are bound with `let` in written order, and the
//! variables are then passed in positional order.

use crate::ir::*;

/// Calls `build` with arguments in positional order, given `(position, expr, type)` triples in
/// written order, preserving evaluation order.
///
/// `build` receives the arguments in positional order (usable for both constructors and calls).
/// If the callee is an expression, the caller is responsible for evaluating it first.
pub fn in_written_order(
    module: &mut Module,
    written: Vec<(usize, Expr, TypeId)>,
    build: impl FnOnce(Vec<Expr>) -> Expr,
) -> Expr {
    debug_assert!(
        {
            let mut seen: Vec<usize> = written.iter().map(|(p, ..)| *p).collect();
            seen.sort_unstable();
            seen.iter().copied().eq(0..written.len())
        },
        "positions are not a permutation of 0..n"
    );
    if written.windows(2).all(|w| w[0].0 < w[1].0) {
        return build(written.into_iter().map(|(_, e, _)| e).collect());
    }

    let mut lets = Vec::new();
    let mut slots: Vec<Option<Expr>> = vec![None; written.len()];
    for (pos, e, ty) in written {
        let arg = if is_atomic(&e) {
            e
        } else {
            let l = module.local("arg", ty);
            lets.push((l, e));
            Expr::Var(l)
        };
        slots[pos] = Some(arg);
    }
    let args = slots
        .into_iter()
        .map(|s| s.expect("a position is missing"))
        .collect();
    lets.into_iter()
        .rev()
        .fold(build(args), |body, (var, value)| Expr::Let {
            var,
            value: Box::new(value),
            body: Box::new(body),
        })
}

/// Evaluating it has no effect, so reordering it is unobservable.
fn is_atomic(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Lit(_) | Expr::Var(_) | Expr::Fn(_) | Expr::Lambda { .. }
    )
}
