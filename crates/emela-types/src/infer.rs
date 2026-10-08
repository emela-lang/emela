//! 単一化，`Never` の join，汎化と具体化．

use ena::unify::InPlaceUnificationTable;
use rustc_hash::FxHashMap;
use smol_str::SmolStr;

use crate::error::{TypeError, TypeErrorKind};
use crate::scheme::{Scheme, param_name};
use crate::ty::{Ty, TyParam, TyVar, TyVarValue};

/// 推論の状態．型変数の表と，集めた型エラーを持つ．
#[derive(Default)]
pub struct InferCtx {
    table: InPlaceUnificationTable<TyVar>,
    errors: Vec<TypeError>,
}

impl InferCtx {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn fresh_var(&mut self) -> TyVar {
        self.table.new_key(TyVarValue(None))
    }

    pub fn new_var(&mut self) -> Ty {
        Ty::Var(self.fresh_var())
    }

    /// 束縛済みの型変数を辿り，先頭が束縛済みの型変数でない形にする．
    /// 未束縛の型変数は代表元にそろえる．
    pub fn shallow_resolve(&mut self, ty: &Ty) -> Ty {
        let mut ty = ty.clone();
        while let Ty::Var(v) = ty {
            match self.table.probe_value(v).0 {
                Some(bound) => ty = bound,
                None => return Ty::Var(self.table.find(v)),
            }
        }
        ty
    }

    /// 型の中の束縛済みの型変数をすべて辿った形にする．
    pub fn resolve(&mut self, ty: &Ty) -> Ty {
        let ty = self.shallow_resolve(ty);
        ty.map_children(|child| self.resolve(child))
    }

    /// `expected` と `actual` を単一化する．失敗したら表は呼ぶ前の状態に戻る．
    pub fn unify(&mut self, expected: &Ty, actual: &Ty) -> Result<(), TypeError> {
        let snapshot = self.table.snapshot();
        match self.unify_inner(expected, actual) {
            Ok(()) => {
                self.table.commit(snapshot);
                Ok(())
            }
            Err(kind) => {
                // 部分的に束縛した型変数で表示が崩れないように，戻してから辿る．
                self.table.rollback_to(snapshot);
                let kind = match kind {
                    TypeErrorKind::Mismatch { expected, actual } => TypeErrorKind::Mismatch {
                        expected: self.resolve(&expected),
                        actual: self.resolve(&actual),
                    },
                    TypeErrorKind::InfiniteType { var, ty } => TypeErrorKind::InfiniteType {
                        var,
                        ty: self.resolve(&ty),
                    },
                };
                Err(TypeError {
                    expected: self.resolve(expected),
                    actual: self.resolve(actual),
                    kind: Box::new(kind),
                })
            }
        }
    }

    /// `unify` の失敗をエラーとして溜めて続ける．単一化できたら `true`．
    pub fn expect(&mut self, expected: &Ty, actual: &Ty) -> bool {
        self.unify(expected, actual)
            .map_err(|e| self.errors.push(e))
            .is_ok()
    }

    /// 期待した型の位置に `actual` を置く．`actual` が `Never` ならどの型でもよい．
    pub fn coerce(&mut self, expected: &Ty, actual: &Ty) -> Result<(), TypeError> {
        if self.shallow_resolve(actual) == Ty::Never {
            return Ok(());
        }
        self.unify(expected, actual)
    }

    /// 2つの分岐の共通の型．片方が `Never` ならもう片方の型になる．
    ///
    /// 失敗したときは `a` を期待した型，`b` を実際の型として報告する．
    pub fn join(&mut self, a: &Ty, b: &Ty) -> Result<Ty, TypeError> {
        let a = self.shallow_resolve(a);
        let b = self.shallow_resolve(b);
        match (&a, &b) {
            (Ty::Never, _) => Ok(b),
            (_, Ty::Never) => Ok(a),
            _ => {
                self.unify(&a, &b)?;
                Ok(a)
            }
        }
    }

    /// `join` の失敗をエラーとして溜める．失敗したときは `a` を全体の型とする．
    pub fn join_or_report(&mut self, a: &Ty, b: &Ty) -> Ty {
        match self.join(a, b) {
            Ok(ty) => ty,
            Err(e) => {
                self.errors.push(e);
                a.clone()
            }
        }
    }

    pub fn report(&mut self, error: TypeError) {
        self.errors.push(error);
    }

    pub fn errors(&self) -> &[TypeError] {
        &self.errors
    }

    pub fn take_errors(&mut self) -> Vec<TypeError> {
        std::mem::take(&mut self.errors)
    }

    fn unify_inner(&mut self, expected: &Ty, actual: &Ty) -> Result<(), TypeErrorKind> {
        let e = self.shallow_resolve(expected);
        let a = self.shallow_resolve(actual);
        let mismatch = |e: &Ty, a: &Ty| TypeErrorKind::Mismatch {
            expected: e.clone(),
            actual: a.clone(),
        };
        match (&e, &a) {
            (Ty::Var(x), Ty::Var(y)) => {
                self.table.union(*x, *y);
                Ok(())
            }
            (Ty::Var(v), t) | (t, Ty::Var(v)) => self.bind(*v, t),
            (Ty::Prim(p), Ty::Prim(q)) if p == q => Ok(()),
            (Ty::Never, Ty::Never) => Ok(()),
            (Ty::Bound(i), Ty::Bound(j)) if i == j => Ok(()),
            (Ty::Fn(f), Ty::Fn(g)) if f.params.len() == g.params.len() => {
                for (p, q) in f.params.iter().zip(&g.params) {
                    self.unify_inner(p, q)?;
                }
                self.unify_inner(&f.ret, &g.ret)
            }
            (Ty::Tuple(xs), Ty::Tuple(ys)) if xs.len() == ys.len() => self.unify_all(xs, ys),
            (Ty::Named(c, xs), Ty::Named(d, ys)) if c == d && xs.len() == ys.len() => {
                self.unify_all(xs, ys)
            }
            (Ty::List(x), Ty::List(y)) => self.unify_inner(x, y),
            _ => Err(mismatch(&e, &a)),
        }
    }

    fn unify_all(&mut self, xs: &[Ty], ys: &[Ty]) -> Result<(), TypeErrorKind> {
        for (x, y) in xs.iter().zip(ys) {
            self.unify_inner(x, y)?;
        }
        Ok(())
    }

    /// 未束縛の型変数 `v` を `ty` に束縛する．`v` が `ty` に現れるなら無限型として拒否する．
    fn bind(&mut self, v: TyVar, ty: &Ty) -> Result<(), TypeErrorKind> {
        if self.occurs(v, ty) {
            return Err(TypeErrorKind::InfiniteType {
                var: v,
                ty: ty.clone(),
            });
        }
        self.table.union_value(v, TyVarValue(Some(ty.clone())));
        Ok(())
    }

    fn occurs(&mut self, v: TyVar, ty: &Ty) -> bool {
        match self.shallow_resolve(ty) {
            Ty::Var(w) => self.table.unioned(v, w),
            ty => {
                let mut found = false;
                ty.for_each_child(|child| found = found || self.occurs(v, child));
                found
            }
        }
    }

    /// 型に現れる未束縛の型変数（代表元）を，最初に現れた順に重複なく返す．
    pub fn free_vars(&mut self, ty: &Ty) -> Vec<TyVar> {
        let mut out = Vec::new();
        self.collect_free_vars(ty, &mut out);
        out
    }

    fn collect_free_vars(&mut self, ty: &Ty, out: &mut Vec<TyVar>) {
        match self.shallow_resolve(ty) {
            Ty::Var(v) => {
                if !out.contains(&v) {
                    out.push(v);
                }
            }
            ty => ty.for_each_child(|child| self.collect_free_vars(child, out)),
        }
    }

    /// `ty` の型変数のうち，`env_vars`（環境に現れる型変数）にないものを量化する．
    ///
    /// 型引数には現れた順に `A`，`B`，… と名前を付ける．制約は付けない．
    pub fn generalize(&mut self, ty: &Ty, env_vars: &[TyVar]) -> Scheme {
        // 環境の型変数が束縛済みなら，その先に現れる型変数も環境のものとして扱う．
        let mut env = Vec::new();
        for v in env_vars {
            self.collect_free_vars(&Ty::Var(*v), &mut env);
        }
        let mut map = FxHashMap::default();
        let mut params = Vec::new();
        for v in self.free_vars(ty) {
            if !env.contains(&v) {
                map.insert(v, params.len() as u32);
                params.push(TyParam::new(SmolStr::new(param_name(params.len()))));
            }
        }
        let ty = self.resolve(ty);
        Scheme::new(params, replace_vars(&ty, &map))
    }

    /// スキームの型引数をそれぞれ新しい型変数に置き換える．
    pub fn instantiate(&mut self, scheme: &Scheme) -> Ty {
        let args: Vec<Ty> = scheme.params.iter().map(|_| self.new_var()).collect();
        substitute_bound(&scheme.ty, &args)
    }
}

fn replace_vars(ty: &Ty, map: &FxHashMap<TyVar, u32>) -> Ty {
    match ty {
        Ty::Var(v) => map.get(v).map_or_else(|| ty.clone(), |i| Ty::Bound(*i)),
        _ => ty.map_children(|child| replace_vars(child, map)),
    }
}

/// `Ty::Bound(i)` を `args[i]` に置き換える．
pub fn substitute_bound(ty: &Ty, args: &[Ty]) -> Ty {
    match ty {
        Ty::Bound(i) => args.get(*i as usize).cloned().unwrap_or_else(|| ty.clone()),
        _ => ty.map_children(|child| substitute_bound(child, args)),
    }
}
