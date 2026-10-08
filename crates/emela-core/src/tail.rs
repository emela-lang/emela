//! 自己末尾呼び出しのループ化（仕様 6.8）．
//!
//! 本体の末尾位置に自分自身への直接呼び出しがある関数を
//!
//! ```text
//! fn f(p0, p1) = Loop { vars: [q0, q1], inits: [p0, p1], body: body[p := q] }
//! ```
//!
//! に書き換え，末尾位置の `f(a, b)` を `Recur([a, b])` にする．
//! ループの変数 `q` は回ごとに新しく束縛するので，ラムダが捕捉しても後の回に上書きされない．

use rustc_hash::FxHashMap;

use crate::ir::*;

/// モジュールの全関数に `loopify_function` をかける．
pub fn loopify(module: &mut Module) {
    let ids: Vec<FnId> = module.functions.iter().map(|(id, _)| id).collect();
    for id in ids {
        loopify_function(module, id);
    }
}

/// 1つの関数をループ化する．自己末尾呼び出しがなければ何もしない．返り値は書き換えたかどうか．
pub fn loopify_function(module: &mut Module, f: FnId) -> bool {
    let arity = module.functions[f].params.len();
    if !has_tail_self_call(&module.functions[f].body, f, arity) {
        return false;
    }
    let params = module.functions[f].params.clone();
    let vars: Vec<Local> = params
        .iter()
        .map(|&p| {
            let hint = module.locals[p].hint.clone();
            module.local(&hint)
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
    matches!(e, Expr::Call { callee: Callee::Direct(g), args } if *g == f && args.len() == arity)
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
        // 短絡評価の右辺も末尾位置．
        Expr::Binary {
            op: BinOp::And | BinOp::Or,
            rhs,
            ..
        } => has_tail_self_call(rhs, f, arity),
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
    // 右辺に自己末尾呼び出しを持つ `&&` / `||` は `if` に脱糖してから辿る．
    // `a && b` は `if a then b else false`，`a || b` は `if a then true else b`．
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

/// 変数の出現を置き換える．局所変数は一意なので，束縛の遮蔽は考えなくてよい．
fn rename(e: &mut Expr, subst: &FxHashMap<Local, Local>) {
    if let Expr::Var(l) = e {
        if let Some(&new) = subst.get(l) {
            *l = new;
        }
        return;
    }
    for_each_child_mut(e, &mut |c| rename(c, subst));
}

/// 直下の部分式すべてに `f` をかける（パターンは除く）．
pub fn for_each_child_mut(e: &mut Expr, f: &mut impl FnMut(&mut Expr)) {
    match e {
        Expr::Lit(_) | Expr::Var(_) | Expr::Fn(_) => {}
        Expr::Let { value, body, .. } => {
            f(value);
            f(body);
        }
        Expr::Call { callee, args } => {
            if let Callee::Indirect(c) = callee {
                f(c);
            }
            args.iter_mut().for_each(f);
        }
        Expr::Builtin { args, .. }
        | Expr::Ctor { args, .. }
        | Expr::Tuple(args)
        | Expr::Recur(args) => args.iter_mut().for_each(f),
        Expr::Field { base, .. } => f(base),
        Expr::List { elems, tail } => {
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
    }
}
