//! 単一化，`Never` の join，汎化と具体化．
//!
//! 関数型のエラーの集合と効果の集合は単一化では等しくせず，包含の制約（[`Subset`]）として溜める．
//! 期待する型の位置に置いた関数の E と R は，期待する型の E と R に含まれればよい（7.4，9.2）．
//! 制約は本体の推論が終わってから解く（`body/errors.rs`）．

use emela_resolve::hir::{DefId, Span};
use ena::unify::InPlaceUnificationTable;
use rustc_hash::FxHashMap;
use smol_str::SmolStr;

use crate::error::{TypeError, TypeErrorKind};
use crate::row::{Row, RowElem, RowVar};
use crate::scheme::{Scheme, param_name, set_param_name};
use crate::ty::{ParamKind, Prim, Ty, TyConId, TyParam, TyVar, TyVarValue};

/// 集合の種類．エラーの集合 E か効果の集合 R か．
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SetKind {
    Errors,
    Effects,
}

/// 集合の包含の制約 `sub ⊆ sup`．
#[derive(Clone, Debug)]
pub(crate) struct Subset {
    pub(crate) sub: Row,
    pub(crate) sup: Row,
    pub(crate) origin: Origin,
    pub(crate) kind: SetKind,
}

/// 包含の制約がどこから来たか．解けなかったときの診断を決める．
#[derive(Clone, Debug)]
pub(crate) enum Origin {
    /// 関数の値を，関数型を期待する位置に置いた（単一化）．位置はその式，`def` は推論していた定義．
    Coerce {
        span: Option<Span>,
        def: Option<DefId>,
    },
    /// 定義の本体の E（R）が，宣言の E（R）に含まれる．
    Body(DefId),
    /// handler の `index` 番目の操作の本体の E が，エフェクトの操作の宣言の E に含まれる（8.1）．
    HandlerOp { handler: DefId, index: usize },
    /// main かテスト関数の本体の R が空である（8.3，13.3）．
    Closed(DefId),
    /// ラムダの本体の E（R）が，ラムダの型の E（R）に含まれる．必ず満たせる．
    Lambda,
}

/// 単一化の向き．関数型の E の包含の向きを決める．
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Variance {
    /// 実際の側が期待する側に含まれる．
    Co,
    /// 期待する側が実際の側に含まれる（関数の引数の中）．
    Contra,
    /// 両方向（名前付き型の型引数の中）．
    Inv,
}

impl Variance {
    fn flip(self) -> Self {
        match self {
            Variance::Co => Variance::Contra,
            Variance::Contra => Variance::Co,
            Variance::Inv => Variance::Inv,
        }
    }
}

/// 推論の状態．型変数の表と，集めた型エラーを持つ．
#[derive(Default)]
pub struct InferCtx {
    table: InPlaceUnificationTable<TyVar>,
    errors: Vec<TypeError>,
    /// 整数リテラルの型変数．Int か Int64 にしか束縛できない（2.4）．
    int_vars: Vec<TyVar>,
    /// 次に作る集合変数の番号．
    next_row_var: u32,
    /// 溜めた包含の制約．
    subsets: Vec<Subset>,
    /// 単一化の途中で作った包含の制約．単一化が成功したら `subsets` に移す．
    row_buffer: Vec<(Row, Row, SetKind)>,
    /// これから溜める単一化の制約の位置と，推論している定義．
    origin: (Option<Span>, Option<DefId>),
    /// error の名前付き型 → タグ．`RowElem::Of` をタグにするのに使う．
    error_tags: FxHashMap<TyConId, DefId>,
    /// 集合変数の解．解いた後（[`Self::set_row_solution`]）だけ持つ．
    row_solution: Option<FxHashMap<RowVar, Row>>,
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

    /// 新しい集合変数．
    pub fn new_row_var(&mut self) -> RowVar {
        let v = RowVar(self.next_row_var);
        self.next_row_var += 1;
        v
    }

    /// error の名前付き型からタグへの対応を渡す．
    pub(crate) fn set_error_tags(&mut self, tags: FxHashMap<TyConId, DefId>) {
        self.error_tags = tags;
    }

    /// error の名前付き型のタグ．
    pub(crate) fn error_tag(&self, con: TyConId) -> Option<DefId> {
        self.error_tags.get(&con).copied()
    }

    /// これから単一化で溜める包含の制約の位置．
    pub(crate) fn set_origin(&mut self, span: Option<Span>, def: Option<DefId>) {
        self.origin = (span, def);
    }

    /// エラーの集合の包含の制約を足す．
    pub(crate) fn add_subset(&mut self, sub: Row, sup: Row, origin: Origin) {
        self.add_subset_of(SetKind::Errors, sub, sup, origin);
    }

    /// 種類 `kind` の集合の包含の制約を足す．
    pub(crate) fn add_subset_of(&mut self, kind: SetKind, sub: Row, sup: Row, origin: Origin) {
        if sub.is_empty() || sub == sup {
            return;
        }
        self.subsets.push(Subset {
            sub,
            sup,
            origin,
            kind,
        });
    }

    pub(crate) fn subsets(&self) -> &[Subset] {
        &self.subsets
    }

    /// 集合変数の解を渡す．以後の [`Self::resolve`] は集合変数を解に置き換える．
    /// 解にない集合変数は空にする．
    pub(crate) fn set_row_solution(&mut self, solution: FxHashMap<RowVar, Row>) {
        self.row_solution = Some(solution);
    }

    /// 整数リテラルの型変数．Int か Int64 にしか束縛できない．他の型変数と単一化すると，
    /// その型変数も同じ制約を持つ．
    pub fn new_int_var(&mut self) -> Ty {
        let v = self.fresh_var();
        self.int_vars.push(v);
        Ty::Var(v)
    }

    /// `v` が整数リテラルの型変数（と単一化した型変数）か．
    pub fn is_int_var(&mut self, v: TyVar) -> bool {
        let int_vars = std::mem::take(&mut self.int_vars);
        let found = int_vars.iter().any(|&w| self.table.unioned(v, w));
        self.int_vars = int_vars;
        found
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
    ///
    /// エラーの集合の中のエラーの型（`RowElem::Of`）は，error の名前付き型ならタグにし，
    /// 型引数と型変数のほかは捨てる（誤りは別に出す）．集合変数を解いた後なら，集合変数を解に置き換える．
    pub fn resolve(&mut self, ty: &Ty) -> Ty {
        match self.shallow_resolve(ty) {
            Ty::Fn(f) => Ty::func_sets(
                f.params.iter().map(|p| self.resolve(p)).collect::<Vec<_>>(),
                self.resolve(&f.ret),
                self.resolve_row(&f.fails),
                self.resolve_row(&f.uses),
            ),
            ty => ty.map_children(|child| self.resolve(child)),
        }
    }

    /// 集合の中の型を辿り，[`Self::resolve`] と同じく整える．
    pub fn resolve_row(&mut self, row: &Row) -> Row {
        row.flat_map(|e| match e {
            RowElem::Of(ty) => match self.resolve(ty) {
                Ty::Named(con, _) => match self.error_tag(con) {
                    Some(def) => Row::tag(def),
                    None => Row::empty(),
                },
                ty @ (Ty::Bound(_) | Ty::Var(_)) => Row::from_elems([RowElem::Of(ty)]),
                _ => Row::empty(),
            },
            RowElem::Var(v) => match &self.row_solution {
                Some(solution) => solution.get(v).cloned().unwrap_or_default(),
                None => Row::var(*v),
            },
            e => Row::from_elems([e.clone()]),
        })
    }

    /// `expected` と `actual` を単一化する．失敗したら表は呼ぶ前の状態に戻る．
    ///
    /// 関数型のエラーの集合は，実際の側が期待する側に含まれる制約にする（引数の中は逆向き，
    /// 名前付き型の型引数の中は両方向）．制約は単一化が成功したときだけ溜める．
    pub fn unify(&mut self, expected: &Ty, actual: &Ty) -> Result<(), TypeError> {
        let snapshot = self.table.snapshot();
        self.row_buffer.clear();
        match self.unify_inner(expected, actual, Variance::Co) {
            Ok(()) => {
                self.table.commit(snapshot);
                let origin = Origin::Coerce {
                    span: self.origin.0,
                    def: self.origin.1,
                };
                for (sub, sup, kind) in std::mem::take(&mut self.row_buffer) {
                    self.add_subset_of(kind, sub, sup, origin.clone());
                }
                Ok(())
            }
            Err(kind) => {
                self.row_buffer.clear();
                // 部分的に束縛した型変数で表示が崩れないように，戻してから辿る．
                self.table.rollback_to(snapshot);
                let kind = match kind {
                    TypeErrorKind::Mismatch { expected, actual } => TypeErrorKind::Mismatch {
                        expected: self.resolve_for_report(&expected),
                        actual: self.resolve_for_report(&actual),
                    },
                    TypeErrorKind::InfiniteType { var, ty } => TypeErrorKind::InfiniteType {
                        var,
                        ty: self.resolve_for_report(&ty),
                    },
                };
                Err(TypeError {
                    expected: self.resolve_for_report(expected),
                    actual: self.resolve_for_report(actual),
                    kind: Box::new(kind),
                })
            }
        }
    }

    /// エラーに出すための形．まだ決まっていない整数リテラルの型変数は `Int` と出す
    /// （決まらなければ Int になるので）．
    /// まだ解いていない集合変数は出さない（型の食い違いの説明には関わらないので）．
    fn resolve_for_report(&mut self, ty: &Ty) -> Ty {
        match self.shallow_resolve(ty) {
            Ty::Var(v) if self.is_int_var(v) => Ty::INT,
            Ty::Fn(f) => {
                let known = |row: &Row| {
                    Row::from_elems(
                        row.elems()
                            .iter()
                            .filter(|e| !matches!(e, RowElem::Var(_)))
                            .cloned(),
                    )
                };
                let fails = known(&f.fails);
                let uses = known(&f.uses);
                Ty::func_sets(
                    f.params
                        .iter()
                        .map(|p| self.resolve_for_report(p))
                        .collect::<Vec<_>>(),
                    self.resolve_for_report(&f.ret),
                    fails.map_tys(|t| self.resolve_for_report(t)),
                    uses,
                )
            }
            ty => ty.map_children(|child| self.resolve_for_report(child)),
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
    ///
    /// 両方が関数型なら，エラーの集合と効果の集合は新しい集合変数にして両方の E と R を含める．
    pub fn join(&mut self, a: &Ty, b: &Ty) -> Result<Ty, TypeError> {
        let a = self.shallow_resolve(a);
        let b = self.shallow_resolve(b);
        match (&a, &b) {
            (Ty::Never, _) => Ok(b),
            (_, Ty::Never) => Ok(a),
            (Ty::Fn(f), Ty::Fn(g)) if f.params.len() == g.params.len() => {
                let e = self.new_row_var();
                let r = self.new_row_var();
                let joined =
                    Ty::func_sets(f.params.clone(), f.ret.clone(), Row::var(e), Row::var(r));
                self.unify(&joined, &a)?;
                self.unify(&joined, &b).map_err(|mut e| {
                    e.expected = self.resolve_for_report(&a);
                    e
                })?;
                Ok(joined)
            }
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

    fn unify_inner(
        &mut self,
        expected: &Ty,
        actual: &Ty,
        variance: Variance,
    ) -> Result<(), TypeErrorKind> {
        let e = self.shallow_resolve(expected);
        let a = self.shallow_resolve(actual);
        let mismatch = |e: &Ty, a: &Ty| TypeErrorKind::Mismatch {
            expected: e.clone(),
            actual: a.clone(),
        };
        match (&e, &a) {
            (Ty::Var(x), Ty::Var(y)) => {
                let int = self.is_int_var(*x) || self.is_int_var(*y);
                self.table.union(*x, *y);
                if int {
                    let root = self.table.find(*x);
                    self.int_vars.push(root);
                }
                Ok(())
            }
            (Ty::Var(v), t) | (t, Ty::Var(v)) => {
                if self.is_int_var(*v)
                    && !matches!(t, Ty::Prim(Prim::Int | Prim::Int64) | Ty::Error)
                {
                    return Err(mismatch(&e, &a));
                }
                self.bind(*v, t)
            }
            (Ty::Error, _) | (_, Ty::Error) => Ok(()),
            (Ty::Prim(p), Ty::Prim(q)) if p == q => Ok(()),
            (Ty::Never, Ty::Never) => Ok(()),
            (Ty::Bound(i), Ty::Bound(j)) if i == j => Ok(()),
            (Ty::Cap(x), Ty::Cap(y)) if x == y => Ok(()),
            (Ty::Fn(f), Ty::Fn(g)) if f.params.len() == g.params.len() => {
                for (p, q) in f.params.iter().zip(&g.params) {
                    self.unify_inner(p, q, variance.flip())?;
                }
                self.unify_inner(&f.ret, &g.ret, variance)?;
                self.buffer_rows(SetKind::Errors, &f.fails, &g.fails, variance);
                self.buffer_rows(SetKind::Effects, &f.uses, &g.uses, variance);
                Ok(())
            }
            (Ty::Tuple(xs), Ty::Tuple(ys)) if xs.len() == ys.len() => {
                self.unify_all(xs, ys, variance)
            }
            // 型引数がどの位置に使われるかは見ないので，両方向にする．
            (Ty::Named(c, xs), Ty::Named(d, ys)) if c == d && xs.len() == ys.len() => {
                self.unify_all(xs, ys, Variance::Inv)
            }
            // 値は不変なので，リストとタプルの要素は外側と同じ向きでよい．
            (Ty::List(x), Ty::List(y)) => self.unify_inner(x, y, variance),
            _ => Err(mismatch(&e, &a)),
        }
    }

    /// 関数型の集合の包含の制約を溜める．`expected` は期待する側の集合．
    fn buffer_rows(&mut self, kind: SetKind, expected: &Row, actual: &Row, variance: Variance) {
        if expected == actual {
            return;
        }
        let (e, a) = (expected.clone(), actual.clone());
        match variance {
            Variance::Co => self.row_buffer.push((a, e, kind)),
            Variance::Contra => self.row_buffer.push((e, a, kind)),
            Variance::Inv => {
                self.row_buffer.push((a.clone(), e.clone(), kind));
                self.row_buffer.push((e, a, kind));
            }
        }
    }

    fn unify_all(&mut self, xs: &[Ty], ys: &[Ty], variance: Variance) -> Result<(), TypeErrorKind> {
        for (x, y) in xs.iter().zip(ys) {
            self.unify_inner(x, y, variance)?;
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
        self.generalize_with(ty, Vec::new(), env_vars)
    }

    /// [`Self::generalize`] と同じく量化する．`existing`（宣言で書いた型引数．`ty` の中では
    /// `Ty::Bound(i)`）の後ろに，量化した型変数を足す．名前は書いた型引数と重ならないものを選ぶ．
    ///
    /// 型の中に残った集合変数（[`Self::resolve`] の後に残るもの）も，集合の型引数として量化する．
    /// 名前はエラーの集合が `E`，`E1`，`E2`，…，効果の集合が `R`，`R1`，….
    /// 型変数の後ろに並べる．
    pub fn generalize_with(
        &mut self,
        ty: &Ty,
        existing: Vec<TyParam>,
        env_vars: &[TyVar],
    ) -> Scheme {
        self.generalize_bounded(ty, existing, env_vars, &Default::default())
    }

    /// [`Self::generalize_with`] と同じく量化する．`immediate` の集合変数を量化した型引数には
    /// `Immediate` の制約を付ける（9.5）．
    pub(crate) fn generalize_bounded(
        &mut self,
        ty: &Ty,
        existing: Vec<TyParam>,
        env_vars: &[TyVar],
        immediate: &rustc_hash::FxHashSet<RowVar>,
    ) -> Scheme {
        // 環境の型変数が束縛済みなら，その先に現れる型変数も環境のものとして扱う．
        let mut env = Vec::new();
        for v in env_vars {
            self.collect_free_vars(&Ty::Var(*v), &mut env);
        }
        let mut map = FxHashMap::default();
        let mut params = existing;
        let mut next_name = 0;
        for v in self.free_vars(ty) {
            if !env.contains(&v) {
                map.insert(v, params.len() as u32);
                let name = loop {
                    let name = param_name(next_name);
                    next_name += 1;
                    if !params.iter().any(|p| p.name == name) {
                        break name;
                    }
                };
                params.push(TyParam::new(SmolStr::new(name)));
            }
        }
        let ty = self.resolve(ty);
        let mut row_map = FxHashMap::default();
        let mut next_name = [0, 0];
        for (v, kind) in free_row_vars(&ty) {
            row_map.insert(v, params.len() as u32);
            let (prefix, next_name) = match kind {
                SetKind::Errors => ("E", &mut next_name[0]),
                SetKind::Effects => ("R", &mut next_name[1]),
            };
            let name = loop {
                let name = set_param_name(prefix, *next_name);
                *next_name += 1;
                if !params.iter().any(|p| p.name == name) {
                    break name;
                }
            };
            let mut param = TyParam::set(SmolStr::new(name));
            if immediate.contains(&v) {
                param.bounds.push(SmolStr::new(crate::lower::IMMEDIATE));
            }
            params.push(param);
        }
        Scheme::new(params, replace_vars(&ty, &map, &row_map))
    }

    /// スキームの型引数をそれぞれ新しい型変数（集合の型引数は新しい集合変数）に置き換える．
    pub fn instantiate(&mut self, scheme: &Scheme) -> Ty {
        self.instantiate_with_args(scheme).0
    }

    /// [`Self::instantiate`] と同じく具体化し，型引数に置いた型変数も返す．
    /// 集合の型引数の位置には，使わない型変数を置く（並びを宣言の型引数とそろえるため）．
    pub fn instantiate_with_args(&mut self, scheme: &Scheme) -> (Ty, Vec<Ty>) {
        let (tys, rows) = self.fresh_args(&scheme.params);
        (substitute(&scheme.ty, &tys, &rows), tys)
    }

    /// 型引数ごとに新しい型変数と集合変数を作る．型の型引数の集合は空，集合の型引数の型は
    /// 使わない型変数．
    pub(crate) fn fresh_args(&mut self, params: &[TyParam]) -> (Vec<Ty>, Vec<Row>) {
        let mut tys = Vec::new();
        let mut rows = Vec::new();
        for p in params {
            tys.push(self.new_var());
            rows.push(match p.kind {
                ParamKind::Type => Row::empty(),
                ParamKind::Set => Row::var(self.new_row_var()),
            });
        }
        (tys, rows)
    }
}

/// 型の中の集合変数と，その集合の種類を，現れた順に重複なく集める．
pub(crate) fn free_row_vars(ty: &Ty) -> Vec<(RowVar, SetKind)> {
    let mut out = Vec::new();
    collect_row_vars(ty, &mut out);
    out
}

fn collect_row_vars(ty: &Ty, out: &mut Vec<(RowVar, SetKind)>) {
    if let Ty::Fn(f) = ty {
        for (row, kind) in [(&f.fails, SetKind::Errors), (&f.uses, SetKind::Effects)] {
            for v in row.vars() {
                if !out.iter().any(|(w, _)| *w == v) {
                    out.push((v, kind));
                }
            }
        }
    }
    ty.for_each_child(|child| collect_row_vars(child, out));
}

fn replace_vars(ty: &Ty, map: &FxHashMap<TyVar, u32>, rows: &FxHashMap<RowVar, u32>) -> Ty {
    match ty {
        Ty::Var(v) => map.get(v).map_or_else(|| ty.clone(), |i| Ty::Bound(*i)),
        Ty::Fn(f) => {
            let replace_row = |row: &Row| {
                row.flat_map(|e| match e {
                    RowElem::Var(v) => match rows.get(v) {
                        Some(i) => Row::from_elems([RowElem::Bound(*i)]),
                        None => Row::var(*v),
                    },
                    RowElem::Of(t) => Row::from_elems([RowElem::Of(replace_vars(t, map, rows))]),
                    e => Row::from_elems([e.clone()]),
                })
            };
            Ty::func_sets(
                f.params
                    .iter()
                    .map(|p| replace_vars(p, map, rows))
                    .collect::<Vec<_>>(),
                replace_vars(&f.ret, map, rows),
                replace_row(&f.fails),
                replace_row(&f.uses),
            )
        }
        _ => ty.map_children(|child| replace_vars(child, map, rows)),
    }
}

/// `Ty::Bound(i)` を `args[i]` に置き換える．集合の型引数（`RowElem::Bound`）はそのまま残す．
pub fn substitute_bound(ty: &Ty, args: &[Ty]) -> Ty {
    substitute(ty, args, &[])
}

/// `Ty::Bound(i)` を `tys[i]` に，集合の中の `RowElem::Bound(i)` を `rows[i]` に置き換える．
/// 範囲外の添字はそのまま残す．
pub(crate) fn substitute(ty: &Ty, tys: &[Ty], rows: &[Row]) -> Ty {
    match ty {
        Ty::Bound(i) => tys.get(*i as usize).cloned().unwrap_or_else(|| ty.clone()),
        Ty::Fn(f) => Ty::func_sets(
            f.params
                .iter()
                .map(|p| substitute(p, tys, rows))
                .collect::<Vec<_>>(),
            substitute(&f.ret, tys, rows),
            substitute_row(&f.fails, tys, rows),
            substitute_row(&f.uses, tys, rows),
        ),
        _ => ty.map_children(|child| substitute(child, tys, rows)),
    }
}

/// 集合の中の型引数を [`substitute`] と同じく置き換える．
pub(crate) fn substitute_row(row: &Row, tys: &[Ty], rows: &[Row]) -> Row {
    row.flat_map(|e| match e {
        RowElem::Bound(i) => rows
            .get(*i as usize)
            .cloned()
            .unwrap_or_else(|| Row::from_elems([e.clone()])),
        RowElem::Of(t) => Row::from_elems([RowElem::Of(substitute(t, tys, rows))]),
        e => Row::from_elems([e.clone()]),
    })
}
