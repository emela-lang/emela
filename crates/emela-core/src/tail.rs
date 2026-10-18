//! Self tail call loopification (spec 6.8).
//!
//! A function with a direct call to itself in tail position of its body is rewritten to
//!
//! ```text
//! fn f(p0, p1) = Loop { vars: [q0, q1], inits: [p0, p1], body: body[p := q] }
//! ```
//!
//! and each tail-position `f(a, b)` becomes `Recur([a, b])`. The loop variables `q` are bound
//! afresh on each iteration, so a lambda capturing them is not overwritten by later iterations.
//!
//! Fallible self tail calls (`fails: true`) are loopified too. A failure of a tail-position call
//! propagates straight to the caller, so failing on the next iteration gives the same result.

use rustc_hash::FxHashMap;

use crate::ir::*;

pub fn loopify(module: &mut Module) {
    let ids: Vec<FnId> = module.functions.iter().map(|(id, _)| id).collect();
    for id in ids {
        loopify_function(module, id);
    }
}

/// Does nothing if there is no self tail call. Returns whether the function was rewritten.
pub fn loopify_function(module: &mut Module, f: FnId) -> bool {
    // A const's zero-argument entry point must pass through the runtime cell on every reference.
    // Loopifying a self-tail-call in its initializer bypasses that guard and can hang forever.
    if module.functions[f].constant {
        return false;
    }
    let arity = module.functions[f].params.len();
    if !has_tail_self_call(&module.functions[f].body, f, arity) {
        return false;
    }
    let params = module.functions[f].params.clone();
    let vars: Vec<Local> = params
        .iter()
        .map(|&p| {
            let data = module.locals[p].clone();
            module.local(&data.hint, data.ty)
        })
        .collect();
    let subst: FxHashMap<Local, Local> = params.iter().copied().zip(vars.iter().copied()).collect();

    let func = &mut module.functions[f];
    let mut body = std::mem::replace(&mut func.body, Expr::Lit(Lit::Unit));
    rename(&mut body, &subst);
    rewrite_tail(&mut body, f, arity);
    func.body = Expr::Loop {
        vars,
        inits: params.into_iter().map(Expr::Var).collect(),
        body: Box::new(body),
    };
    true
}

fn is_self_call(e: &Expr, f: FnId, arity: usize) -> bool {
    matches!(e, Expr::Call { callee: Callee::Direct { f: g, .. }, args, .. } if *g == f && args.len() == arity)
}

fn has_tail_self_call(e: &Expr, f: FnId, arity: usize) -> bool {
    if is_self_call(e, f, arity) {
        return true;
    }
    match e {
        Expr::Let { body, .. } => has_tail_self_call(body, f, arity),
        Expr::If { then, else_, .. } => {
            has_tail_self_call(then, f, arity) || has_tail_self_call(else_, f, arity)
        }
        Expr::Match { arms, .. } => arms.iter().any(|a| has_tail_self_call(&a.body, f, arity)),
        // The right side of a short-circuit operator is in tail position too.
        Expr::Binary {
            op: BinOp::And | BinOp::Or,
            rhs,
            ..
        } => has_tail_self_call(rhs, f, arity),
        // An escape's target is not in tail position (its arms may run afterwards, spec 6.8), and
        // its arms are not visited either.
        _ => false,
    }
}

fn rewrite_tail(e: &mut Expr, f: FnId, arity: usize) {
    if is_self_call(e, f, arity) {
        let Expr::Call { args, .. } = std::mem::replace(e, Expr::Lit(Lit::Unit)) else {
            unreachable!()
        };
        *e = Expr::Recur(args);
        return;
    }
    // `&&` / `||` with a self tail call on the right are desugared to `if` before visiting:
    // `a && b` is `if a then b else false`, `a || b` is `if a then true else b`.
    if let Expr::Binary {
        op: op @ (BinOp::And | BinOp::Or),
        rhs,
        ..
    } = e
        && has_tail_self_call(rhs, f, arity)
    {
        let op = *op;
        let Expr::Binary { lhs, rhs, .. } = std::mem::replace(e, Expr::Lit(Lit::Unit)) else {
            unreachable!()
        };
        let (then, else_) = if op == BinOp::And {
            (rhs, Box::new(Expr::Lit(Lit::Bool(false))))
        } else {
            (Box::new(Expr::Lit(Lit::Bool(true))), rhs)
        };
        *e = Expr::If {
            cond: lhs,
            then,
            else_,
        };
    }
    match e {
        Expr::Let { body, .. } => rewrite_tail(body, f, arity),
        Expr::If { then, else_, .. } => {
            rewrite_tail(then, f, arity);
            rewrite_tail(else_, f, arity);
        }
        Expr::Match { arms, .. } => {
            for arm in arms {
                rewrite_tail(&mut arm.body, f, arity);
            }
        }
        _ => {}
    }
}

/// Locals are unique, so shadowing need not be considered.
fn rename(e: &mut Expr, subst: &FxHashMap<Local, Local>) {
    if let Expr::Var(l) = e {
        if let Some(&new) = subst.get(l) {
            *l = new;
        }
        return;
    }
    for_each_child_mut(e, &mut |c| rename(c, subst));
}

/// Applies `f` to every direct subexpression (not patterns).
pub fn for_each_child_mut(e: &mut Expr, f: &mut impl FnMut(&mut Expr)) {
    match e {
        Expr::Lit(_) | Expr::Var(_) | Expr::Fn { .. } => {}
        Expr::Let { value, body, .. } => {
            f(value);
            f(body);
        }
        Expr::Call { callee, args, .. } => {
            match callee {
                Callee::Direct { .. } => {}
                Callee::Indirect(c) | Callee::Op { handler: c, .. } => f(c),
            }
            args.iter_mut().for_each(f);
        }
        Expr::Builtin { args, .. }
        | Expr::Ctor { args, .. }
        | Expr::Tuple(args)
        | Expr::Recur(args) => args.iter_mut().for_each(f),
        Expr::Field { base, .. } => f(base),
        Expr::List { elems, tail, .. } => {
            elems.iter_mut().for_each(&mut *f);
            if let Some(t) = tail {
                f(t);
            }
        }
        Expr::If { cond, then, else_ } => {
            f(cond);
            f(then);
            f(else_);
        }
        Expr::Match { scrutinee, arms } => {
            f(scrutinee);
            for arm in arms {
                if let Some(g) = &mut arm.guard {
                    f(g);
                }
                f(&mut arm.body);
            }
        }
        Expr::Binary { lhs, rhs, .. } => {
            f(lhs);
            f(rhs);
        }
        Expr::Unary { operand, .. } => f(operand),
        Expr::Lambda { body, .. } => f(body),
        Expr::Concat(parts) => {
            for p in parts {
                if let StrPart::Value(v, _) = p {
                    f(v);
                }
            }
        }
        Expr::Loop { inits, body, .. } => {
            inits.iter_mut().for_each(&mut *f);
            f(body);
        }
        Expr::Assert(a) => a.for_each_operand_mut(f),
        Expr::Fail(e) => f(e),
        Expr::Escape { body, arms } => {
            f(body);
            for arm in arms {
                if let Some(g) = &mut arm.guard {
                    f(g);
                }
                f(&mut arm.body);
            }
        }
        Expr::Use { ctx, .. } => f(ctx),
        Expr::Provide { base, bindings } => {
            if let Some(b) = base {
                f(b);
            }
            for (_, h) in bindings {
                f(h);
            }
        }
        Expr::Handler { ops, .. } => ops.iter_mut().for_each(f),
        Expr::With { binds, body } => {
            for b in binds {
                f(&mut b.value);
                if let Some(r) = &mut b.release {
                    f(&mut r.body);
                }
            }
            f(body);
        }
        Expr::Fiber { args, .. } => args.iter_mut().for_each(f),
    }
}
