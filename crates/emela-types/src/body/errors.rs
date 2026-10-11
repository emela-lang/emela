//! エラーの集合 E の推論（7章，9.4）．
//!
//! 式の型を推論し終えてから，HIR をもう一度辿って各式の E を求める．呼び出しの E は呼ばれる側の
//! 関数型の E なので，型が決まった後の方が扱いやすい．
//!
//! E は，単一化で溜めた包含の制約（関数の値を関数型の位置に置いたとき）と，ここで足す制約
//! （定義の本体の E ⊆ 宣言の E，ラムダの本体の E ⊆ ラムダの型の E）で決まる．集合変数ごとに
//! 下界を集め，最小の解を求めてから，すべての制約を確かめる．
//!
//! - 制約 `sub ⊆ sup` の `sup` に集合変数があれば，`sub` の要素のうち `sup` にないものを，
//!   `sup` の最初の集合変数の下界にする．`sup` のタグは除いて渡す（`fails Timeout | E` の引数に
//!   関数を渡すと，`E` には Timeout を除いたものが入る．7.4）．
//! - escape は，元の式の E のタグから受けたタグを消す．集合変数は，受けたタグを除いた解を持つ
//!   新しい集合変数に置き換える．集合の型引数（中身の分からない集合）からは何も消さない（7.3）．
//! - 汎化する定義の型で，負の位置（引数の関数型の E）にあり，下界も上界もない集合変数は，
//!   空にせず集合の型引数として汎化する．それ以外の集合変数は最小の解にする．

use emela_resolve::hir::{
    Block, DefId, DefKind, ExprId, ExprKind, Item, PatId, PatKind, Res, Span, Stmt, StrPart,
};
use la_arena::ArenaMap;
use rustc_hash::{FxHashMap, FxHashSet};
use smol_str::SmolStr;

use super::BodyInfer;
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::infer::Origin;
use crate::row::{Row, RowElem, RowVar};
use crate::ty::{Ty, TyParam};

/// 集合変数の下界の1つ．`elem` の要素のうち，`minus` のタグを除いたもの．
struct Lower {
    elem: RowElem,
    minus: Vec<DefId>,
}

impl BodyInfer<'_> {
    /// 推論した定義の本体の E を求め，宣言の E に含まれる制約を足す．
    pub(super) fn infer_errors(&mut self) {
        let program = self.program;
        for def in self.defs.clone() {
            self.current = Some(def);
            let (body, declared) = match program.item(def) {
                Some(Item::Fn(item)) => {
                    let Some(body) = item.body else {
                        continue;
                    };
                    let declared = match self.def_ty(def).map(|ty| self.ctx.shallow_resolve(&ty)) {
                        Some(Ty::Fn(f)) => f.fails,
                        _ => continue,
                    };
                    (body, declared)
                }
                // const の値は失敗できない（5.6）．
                Some(Item::Const(item)) => (item.value, Row::empty()),
                _ => continue,
            };
            let row = self.expr_row(body);
            self.ctx.add_subset(row, declared, Origin::Body(def));
        }
    }

    /// 定義の型．宣言の型か，まとまりの中の単相の型．
    fn def_ty(&self, def: DefId) -> Option<Ty> {
        match self.sigs.get(def) {
            Some(sig) => Some(sig.scheme.ty.clone()),
            None => self.pending_ty(def).map(|(ty, _)| ty),
        }
    }

    /// 定義の型引数（`Bound(i)` の名前）．
    fn def_params(&self, def: DefId) -> Vec<TyParam> {
        match self.sigs.get(def) {
            Some(sig) => sig.scheme.params.clone(),
            None => self
                .pending_ty(def)
                .map(|(_, params)| params)
                .unwrap_or_default(),
        }
    }

    /// 式の E．部分式の E は親の式に合流する（9.4）．求めた E は式ごとに記録する．
    fn expr_row(&mut self, expr: ExprId) -> Row {
        let program = self.program;
        let e = &program.exprs[expr];
        let row = match &e.kind {
            ExprKind::Missing
            | ExprKind::Lit(_)
            | ExprKind::Path(_)
            | ExprKind::Unit
            | ExprKind::Use { .. } => Row::empty(),
            ExprKind::Interpolated(parts) => {
                let mut row = Row::empty();
                for part in parts {
                    if let StrPart::Interp(e) = part {
                        row.extend(&self.expr_row(*e));
                    }
                }
                row
            }
            ExprKind::Field { base, .. } => self.expr_row(*base),
            ExprKind::Call { callee, args } => {
                let mut row = self.expr_row(*callee);
                for arg in args {
                    row.extend(&self.expr_row(arg.value));
                }
                row.extend(&self.callee_fails(*callee, e.span));
                row
            }
            ExprKind::Unary { operand, .. } => self.expr_row(*operand),
            ExprKind::Binary {
                op: emela_resolve::hir::BinaryOp::Pipe,
                lhs,
                rhs,
            } => {
                // `x |> f(a)` は `f(x, a)`．右辺の呼び出しの式には，左辺を除いた E を記録する．
                let mut row = self.expr_row(*lhs);
                let rhs_row = match &program.exprs[*rhs].kind {
                    ExprKind::Call { callee, args } => {
                        let mut r = self.expr_row(*callee);
                        for arg in args {
                            r.extend(&self.expr_row(arg.value));
                        }
                        r.extend(&self.callee_fails(*callee, e.span));
                        self.expr_rows.insert(*rhs, r.clone());
                        r
                    }
                    _ => {
                        let mut r = self.expr_row(*rhs);
                        r.extend(&self.callee_fails(*rhs, e.span));
                        r
                    }
                };
                row.extend(&rhs_row);
                row
            }
            ExprKind::Binary { lhs, rhs, .. } => self.expr_row(*lhs).union(&self.expr_row(*rhs)),
            ExprKind::Tuple(elems) => self.rows_of(elems),
            ExprKind::List { elems, rest } => {
                let mut row = self.rows_of(elems);
                if let Some(rest) = rest {
                    row.extend(&self.expr_row(*rest));
                }
                row
            }
            ExprKind::Block(block) => self.block_row(block),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let mut row = self.expr_row(*cond);
                row.extend(&self.expr_row(*then_branch));
                if let Some(e) = else_branch {
                    row.extend(&self.expr_row(*e));
                }
                row
            }
            ExprKind::Match { scrutinee, arms } => {
                let mut row = self.expr_row(*scrutinee);
                for arm in arms {
                    if let Some(guard) = arm.guard {
                        row.extend(&self.expr_row(guard));
                    }
                    row.extend(&self.expr_row(arm.body));
                }
                row
            }
            ExprKind::Escape { expr: inner, arms } => {
                let inner_row = self.expr_row(*inner);
                let caught: Vec<(PatId, DefId)> = arms
                    .iter()
                    .filter_map(|arm| match &program.pats[arm.pat].kind {
                        PatKind::Ctor {
                            res: Res::Def(def), ..
                        } if program.defs[*def].kind == DefKind::Error => Some((arm.pat, *def)),
                        _ => None,
                    })
                    .collect();
                let tags: Vec<DefId> = caught.iter().map(|&(_, def)| def).collect();
                // エラーの構成子でない腕（E0320）があれば，誤りを重ねないよう全部受けたとみなす．
                let broken = arms.iter().any(|arm| {
                    !matches!(
                        &program.pats[arm.pat].kind,
                        PatKind::Ctor { .. } | PatKind::Missing
                    )
                });
                let inner_row = if broken { Row::empty() } else { inner_row };
                let mut row = Row::empty();
                for e in self.ctx.resolve_row(&inner_row).elems() {
                    match e {
                        RowElem::Tag(t) if tags.contains(t) => {}
                        // 集合変数は，受けたタグを除いた解を持つ集合変数に置き換える．
                        RowElem::Var(v) if !tags.is_empty() => {
                            let d = self.ctx.new_row_var();
                            self.row_diffs.push((d, *v, tags.clone()));
                            row.insert(RowElem::Var(d));
                        }
                        // 集合の型引数とエラーの型の型引数からは消さない（7.3）．
                        e => row.insert(e.clone()),
                    }
                }
                for arm in arms {
                    if let Some(guard) = arm.guard {
                        row.extend(&self.expr_row(guard));
                    }
                    row.extend(&self.expr_row(arm.body));
                }
                self.escapes.push((*inner, caught));
                row
            }
            ExprKind::Fail(inner) => {
                let mut row = self.expr_row(*inner);
                let ty = self.exprs.get(*inner).cloned().unwrap_or(Ty::Error);
                let ty = self.ctx.resolve(&ty);
                if self.is_error_ty(&ty) {
                    row.insert(RowElem::Of(ty));
                } else {
                    self.diags.push(Diagnostic {
                        span: Some(program.exprs[*inner].span),
                        kind: DiagnosticKind::NotAnError { found: ty },
                    });
                }
                row
            }
            ExprKind::Assert(e) => self.expr_row(*e),
            ExprKind::Update { ctor, fields, base } => {
                let mut row = self.expr_row(*ctor);
                for field in fields {
                    row.extend(&self.expr_row(field.value));
                }
                row.extend(&self.expr_row(*base));
                row
            }
            ExprKind::With { body, .. } => self.expr_row(*body),
            // 本体の E はラムダの型の E に閉じ込める．表に出るのは呼んだとき（9.4）．
            ExprKind::Lambda { body, .. } => {
                let body_row = self.expr_row(*body);
                if let Some(&v) = self.lambda_rows.get(expr) {
                    self.ctx.add_subset(body_row, Row::var(v), Origin::Lambda);
                }
                Row::empty()
            }
        };
        self.expr_rows.insert(expr, row.clone());
        row
    }

    fn rows_of(&mut self, exprs: &[ExprId]) -> Row {
        let mut row = Row::empty();
        for &e in exprs {
            row.extend(&self.expr_row(e));
        }
        row
    }

    fn block_row(&mut self, block: &Block) -> Row {
        let mut row = Row::empty();
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let { value, .. } => row.extend(&self.expr_row(*value)),
                Stmt::Expr(e) => row.extend(&self.expr_row(*e)),
            }
        }
        if let Some(tail) = block.tail {
            row.extend(&self.expr_row(tail));
        }
        row
    }

    /// 呼ばれる側の関数型の E．1つのエラーの型の型引数をエラーでない型で具体化していれば
    /// E0319 を `span`（呼び出しの式）に積む．
    fn callee_fails(&mut self, callee: ExprId, span: Span) -> Row {
        let ty = self.exprs.get(callee).cloned().unwrap_or(Ty::Error);
        let Ty::Fn(f) = self.ctx.shallow_resolve(&ty) else {
            return Row::empty();
        };
        let mut row = Row::empty();
        for e in f.fails.elems() {
            if let RowElem::Of(ty) = e {
                let ty = self.ctx.resolve(ty);
                if !self.is_error_ty(&ty) {
                    self.diags.push(Diagnostic {
                        span: Some(span),
                        kind: DiagnosticKind::NotAnError { found: ty },
                    });
                    continue;
                }
            }
            row.insert(e.clone());
        }
        row
    }

    /// エラーの型として使える型か．error の名前付き型，型引数，まだ決まっていない型変数．
    /// `Never` と `Ty::Error` は誤りを重ねないために受け入れる（E に何も足さない）．
    fn is_error_ty(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Named(con, _) => self.ctx.error_tag(*con).is_some(),
            Ty::Bound(_) | Ty::Var(_) | Ty::Never | Ty::Error => true,
            _ => false,
        }
    }

    /// 包含の制約を解き，満たされない制約の診断を積む．解は以後の `resolve` に使う．
    pub(super) fn solve_errors(&mut self) {
        let subsets = self.ctx.subsets().to_vec();

        // 集合変数の下界．
        let mut lowers: FxHashMap<RowVar, Vec<Lower>> = FxHashMap::default();
        for (d, v, minus) in std::mem::take(&mut self.row_diffs) {
            lowers.entry(d).or_default().push(Lower {
                elem: RowElem::Var(v),
                minus,
            });
        }
        // 上界のある集合変数（集合変数のない集合に含まれる）．汎化しない．
        let mut upper: FxHashSet<RowVar> = FxHashSet::default();
        for c in &subsets {
            let sub = self.ctx.resolve_row(&c.sub);
            let sup = self.ctx.resolve_row(&c.sup);
            match sup.vars().next() {
                Some(v) => {
                    let minus: Vec<DefId> = sup
                        .elems()
                        .iter()
                        .filter_map(|e| match e {
                            RowElem::Tag(t) => Some(*t),
                            _ => None,
                        })
                        .collect();
                    for e in sub.elems() {
                        if !sup.contains(e) {
                            lowers.entry(v).or_default().push(Lower {
                                elem: e.clone(),
                                minus: minus.clone(),
                            });
                        }
                    }
                }
                None => upper.extend(sub.vars()),
            }
        }

        // 汎化する集合変数．
        let mut negative = Vec::new();
        for (_, ty, _) in self.pending.clone() {
            self.negative_row_vars(&ty, true, &mut negative);
        }
        let mut solution: FxHashMap<RowVar, Row> = FxHashMap::default();
        for v in negative {
            if !lowers.contains_key(&v) && !upper.contains(&v) {
                solution.insert(v, Row::var(v));
            }
        }

        // 最小の解．下界の和を，変わらなくなるまで広げる．
        loop {
            let mut changed = false;
            for (&v, entries) in &lowers {
                for lower in entries {
                    let atoms = match &lower.elem {
                        RowElem::Var(w) => solution.get(w).cloned().unwrap_or_default(),
                        e => self.ctx.resolve_row(&Row::from_elems([e.clone()])),
                    };
                    let target = solution.entry(v).or_default();
                    for a in atoms.elems() {
                        if matches!(a, RowElem::Tag(t) if lower.minus.contains(t)) {
                            continue;
                        }
                        if !target.contains(a) {
                            target.insert(a.clone());
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
        self.ctx.set_row_solution(solution);

        // 制約を確かめる．
        for c in &subsets {
            let sub = self.ctx.resolve_row(&c.sub);
            let sup = self.ctx.resolve_row(&c.sup);
            let missing: Vec<RowElem> = sub
                .elems()
                .iter()
                .filter(|e| !sup.contains(e))
                .cloned()
                .collect();
            if missing.is_empty() {
                continue;
            }
            match &c.origin {
                Origin::Body(def) => self.report_unhandled(*def, &missing),
                Origin::Coerce { span, def } => {
                    let params = def.map(|d| self.def_params(d)).unwrap_or_default();
                    let cons = &self.ty_table.cons;
                    self.diags.push(Diagnostic {
                        span: *span,
                        kind: DiagnosticKind::ErrorSetMismatch {
                            expected: SmolStr::new(sup.display_with(cons, &params).to_string()),
                            found: SmolStr::new(sub.display_with(cons, &params).to_string()),
                        },
                    });
                }
                // ラムダの型の E は集合変数なので，必ず満たせる．
                Origin::Lambda => {}
            }
        }

        self.check_unraised();
    }

    /// 型の中の，負の位置（引数の関数型の E）にある集合変数を集める．名前付き型の型引数の中は
    /// 両方の位置とみなす．
    fn negative_row_vars(&mut self, ty: &Ty, positive: bool, out: &mut Vec<RowVar>) {
        match self.ctx.shallow_resolve(ty) {
            Ty::Fn(f) => {
                if !positive {
                    for v in f.fails.vars() {
                        if !out.contains(&v) {
                            out.push(v);
                        }
                    }
                }
                for p in &f.params {
                    self.negative_row_vars(p, !positive, out);
                }
                self.negative_row_vars(&f.ret, positive, out);
            }
            Ty::Named(_, args) => {
                for a in &args {
                    self.negative_row_vars(a, false, out);
                    self.negative_row_vars(a, true, out);
                }
            }
            Ty::List(elem) => self.negative_row_vars(&elem, positive, out),
            Ty::Tuple(elems) => {
                for e in &elems {
                    self.negative_row_vars(e, positive, out);
                }
            }
            _ => {}
        }
    }

    /// 本体のエラーが宣言の E に含まれない（E0318）．エラーごとに，それを起こした式
    /// （`fail` か呼び出し）を探し，同じ式から起きたものをまとめて1つの診断にする．
    fn report_unhandled(&mut self, def: DefId, missing: &[RowElem]) {
        let program = self.program;
        let (body, declared) = match program.item(def) {
            Some(Item::Fn(item)) => match item.body {
                Some(body) => (body, item.sig.fails.is_some()),
                None => return,
            },
            Some(Item::Const(item)) => (item.value, false),
            _ => return,
        };
        let mut rows: ArenaMap<ExprId, Row> = ArenaMap::default();
        for (expr, row) in self.expr_rows.clone().iter() {
            rows.insert(expr, self.ctx.resolve_row(row));
        }
        let mut groups: Vec<(ExprId, Vec<RowElem>)> = Vec::new();
        for e in missing {
            let origin = self.origin_of(&rows, body, e);
            match groups.iter_mut().find(|(o, _)| *o == origin) {
                Some((_, elems)) => elems.push(e.clone()),
                None => groups.push((origin, vec![e.clone()])),
            }
        }
        let params = self.def_params(def);
        let cons = &self.ty_table.cons;
        for (origin, elems) in groups {
            let errors = elems
                .iter()
                .map(|e| {
                    let row = Row::from_elems([e.clone()]);
                    SmolStr::new(row.display_with(cons, &params).to_string())
                })
                .collect();
            self.diags.push(Diagnostic {
                span: Some(program.exprs[origin].span),
                kind: DiagnosticKind::UnhandledErrors { errors, declared },
            });
        }
    }

    /// エラー `elem` を起こした式．E に `elem` を含む部分式を，書かれた順に最初のものへ辿り，
    /// それ以上辿れない式（`fail` か呼び出し）を返す．
    fn origin_of(&self, rows: &ArenaMap<ExprId, Row>, expr: ExprId, elem: &RowElem) -> ExprId {
        let mut current = expr;
        loop {
            let next = self
                .sub_exprs(current)
                .into_iter()
                .find(|&e| rows.get(e).is_some_and(|row| row.contains(elem)));
            match next {
                Some(next) => current = next,
                None => return current,
            }
        }
    }

    /// E が合流する部分式．書かれた順．ラムダの本体は含めない．
    fn sub_exprs(&self, expr: ExprId) -> Vec<ExprId> {
        let program = self.program;
        let mut out = Vec::new();
        match &program.exprs[expr].kind {
            ExprKind::Missing
            | ExprKind::Lit(_)
            | ExprKind::Path(_)
            | ExprKind::Unit
            | ExprKind::Use { .. }
            | ExprKind::Lambda { .. } => {}
            ExprKind::Interpolated(parts) => {
                for part in parts {
                    if let StrPart::Interp(e) = part {
                        out.push(*e);
                    }
                }
            }
            ExprKind::Field { base, .. } => out.push(*base),
            ExprKind::Call { callee, args } => {
                out.push(*callee);
                out.extend(args.iter().map(|a| a.value));
            }
            ExprKind::Unary { operand, .. } => out.push(*operand),
            ExprKind::Binary { lhs, rhs, .. } => out.extend([*lhs, *rhs]),
            ExprKind::Tuple(elems) => out.extend(elems),
            ExprKind::List { elems, rest } => {
                out.extend(elems);
                out.extend(rest);
            }
            ExprKind::Block(block) => {
                for stmt in &block.stmts {
                    match stmt {
                        Stmt::Let { value, .. } => out.push(*value),
                        Stmt::Expr(e) => out.push(*e),
                    }
                }
                out.extend(block.tail);
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                out.extend([*cond, *then_branch]);
                out.extend(else_branch);
            }
            ExprKind::Match {
                scrutinee: inner,
                arms,
            }
            | ExprKind::Escape { expr: inner, arms } => {
                out.push(*inner);
                for arm in arms {
                    out.extend(arm.guard);
                    out.push(arm.body);
                }
            }
            ExprKind::Fail(e) | ExprKind::Assert(e) => out.push(*e),
            ExprKind::Update { ctor, fields, base } => {
                out.push(*ctor);
                out.extend(fields.iter().map(|f| f.value));
                out.push(*base);
            }
            ExprKind::With { body, .. } => out.push(*body),
        }
        out
    }

    /// escape の腕が受けるエラーが，元の式から起きないなら警告する（W0302）．元の式の E に
    /// 中身の分からない集合（集合の型引数，エラーの型の型引数，汎化する集合変数）があれば，
    /// 実行時に受けることがあるので警告しない．
    fn check_unraised(&mut self) {
        for (inner, caught) in std::mem::take(&mut self.escapes) {
            let Some(row) = self.expr_rows.get(inner).cloned() else {
                continue;
            };
            let row = self.ctx.resolve_row(&row);
            if row.elems().iter().any(|e| !matches!(e, RowElem::Tag(_))) {
                continue;
            }
            for (pat, tag) in caught {
                if !row.contains(&RowElem::Tag(tag)) {
                    let error = self.program.defs[tag].name.clone();
                    self.diags.push(Diagnostic {
                        span: Some(self.program.pats[pat].span),
                        kind: DiagnosticKind::UnraisedError { error },
                    });
                }
            }
        }
    }
}
