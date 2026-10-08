//! 名前付き引数の並べ替え．
//!
//! IR の引数は位置の順に並べるが，評価は書かれた順に行う．書かれた順と位置の順が食い違うときは，
//! 副作用のありうる引数を書かれた順に `let` で束縛してから，位置の順に並べた変数を渡す．

use crate::ir::*;

/// 書かれた順の `(位置, 式)` から，評価順を保ったまま位置の順の引数で `build` を呼ぶ．
///
/// `build` には位置の順に並んだ引数が渡る（構成子の構築にも呼び出しにも使える）．
/// 呼び出し先が式なら，それを先に評価する責任は呼び出し側にある．
pub fn in_written_order(
    module: &mut Module,
    written: Vec<(usize, Expr)>,
    build: impl FnOnce(Vec<Expr>) -> Expr,
) -> Expr {
    debug_assert!(
        {
            let mut seen: Vec<usize> = written.iter().map(|(p, _)| *p).collect();
            seen.sort_unstable();
            seen.iter().copied().eq(0..written.len())
        },
        "位置が 0..n の並べ替えになっていない"
    );
    if written.windows(2).all(|w| w[0].0 < w[1].0) {
        return build(written.into_iter().map(|(_, e)| e).collect());
    }

    let mut lets = Vec::new();
    let mut slots: Vec<Option<Expr>> = vec![None; written.len()];
    for (pos, e) in written {
        let arg = if is_atomic(&e) {
            e
        } else {
            let l = module.local("arg");
            lets.push((l, e));
            Expr::Var(l)
        };
        slots[pos] = Some(arg);
    }
    let args = slots
        .into_iter()
        .map(|s| s.expect("位置が欠けている"))
        .collect();
    lets.into_iter()
        .rev()
        .fold(build(args), |body, (var, value)| Expr::Let {
            var,
            value: Box::new(value),
            body: Box::new(body),
        })
}

/// 評価しても何も起きない式．並べ替えても観測できない．
fn is_atomic(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Lit(_) | Expr::Var(_) | Expr::Fn(_) | Expr::Lambda { .. }
    )
}
