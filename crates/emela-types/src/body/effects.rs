//! 効果の集合 R の推論（8章，9.4）．
//!
//! E と同じく，式の型を推論し終えてから HIR をもう一度辿って各式の R を求める．制約は E と同じ
//! 表に溜め，`body/errors.rs` で一緒に解く．
//!
//! - `use X` の R は `{X}`．分岐の中に書いても，部分式の R は親の式に合流するので関数全体の R に載る（8.2）．
//! - 呼び出しの R は，呼ばれる側の関数型の R を足す．能力の値の操作の型の R はそのエフェクト．
//! - `with H1, ..., Hn { e }` の R は，`e` の R から供給したエフェクトを消し，各ハンドラの依存を
//!   足したもの（9.4）．layer は中のハンドラに展開する（8.5，`crate::layer`）．ハンドラの依存のうち，同じ with の別のエフェクトを供給するハンドラが
//!   満たすものは足さない（14.2）．集合変数からは，消すエフェクトを除いた解を持つ新しい集合変数に
//!   置き換えて消す．集合の型引数（中身の分からない集合）からは消さない．
//! - 無名関数は本体の R を関数型に閉じ込める．
//! - 本体の R は宣言の R に含まれる．`use` 節を書いていなければ宣言の R は集合変数で，本体の R が
//!   そのまま解になる（9.3）．main とテスト関数の本体の R は，Async を除いて空でなければならない
//!   （8.3，13.3）．
//! - handler の依存は init，release，操作の本体の R の和（集合変数）．resume を後から呼びうる suspend fn の実装は，
//!   Async も依存に足す（8.7，`body/suspend.rs`）．
//! - main とテスト関数の本体の R は Async だけを含んでよい（Async はランタイムが供給する）．

use emela_resolve::hir::{BinaryOp, DefId, ExprId, ExprKind, Item, Res, Span};

use super::BodyInfer;
use crate::infer::{Origin, SetKind};
use crate::layer;
use crate::row::{Row, RowElem};
use crate::ty::Ty;

impl BodyInfer<'_> {
    /// 推論した定義の本体の R を求め，宣言の R に含まれる制約を足す．
    pub(super) fn infer_effects(&mut self) {
        let program = self.program;
        for def in self.defs.clone() {
            self.current = Some(def);
            match program.item(def) {
                Some(Item::Fn(item)) => {
                    let Some(body) = item.body else {
                        continue;
                    };
                    let declared = match self.def_ty(def).map(|ty| self.ctx.shallow_resolve(&ty)) {
                        Some(Ty::Fn(f)) => f.uses,
                        _ => continue,
                    };
                    let row = self.expr_effects(body);
                    let closed = self.closed.contains_key(&def);
                    if closed {
                        // Async はランタイムとテストランナーが供給する（8.3，13.3）．
                        let allowed = program
                            .prelude_def("Async")
                            .map_or_else(Row::empty, Row::tag);
                        self.ctx.add_subset_of(
                            SetKind::Effects,
                            row.clone(),
                            allowed,
                            Origin::Closed(def),
                        );
                    }
                    // main とテスト関数で `use` 節を書いていれば，空でないことだけを報告する．
                    if !closed || item.sig.uses.is_none() {
                        self.ctx
                            .add_subset_of(SetKind::Effects, row, declared, Origin::Body(def));
                    }
                }
                // const の値は効果を持てない（5.6）．
                Some(Item::Const(item)) => {
                    let row = self.expr_effects(item.value);
                    self.ctx
                        .add_subset_of(SetKind::Effects, row, Row::empty(), Origin::Body(def));
                }
                Some(Item::Handler(_)) => {
                    let deps = self.handler_deps(def);
                    let bodies: Vec<(usize, ExprId, bool)> = self
                        .handler_bodies
                        .iter()
                        .filter(|b| b.handler == def)
                        .map(|b| (b.index, b.body, b.resume_later))
                        .collect();
                    for (index, body, resume_later) in bodies {
                        let mut row = self.expr_effects(body);
                        // resume を後から呼びうる suspend fn の実装は Async を要求する（8.7）．
                        if resume_later && let Some(async_) = program.prelude_def("Async") {
                            row.insert(RowElem::Tag(async_));
                        }
                        self.ctx.add_subset_of(
                            SetKind::Effects,
                            row,
                            deps.clone(),
                            Origin::HandlerOp {
                                handler: def,
                                index,
                            },
                        );
                    }
                    // init と release の本体の R も依存（9.4）．release の中で suspend な操作を
                    // 呼んでよい（8.4）．
                    let parts: Vec<ExprId> = self
                        .handler_parts
                        .iter()
                        .filter(|p| p.handler == def)
                        .map(|p| p.body)
                        .collect();
                    for body in parts {
                        let row = self.expr_effects(body);
                        self.ctx.add_subset_of(
                            SetKind::Effects,
                            row,
                            deps.clone(),
                            Origin::HandlerPart,
                        );
                    }
                }
                _ => {}
            }
        }
    }

    /// 式の R．部分式の R は親の式に合流する（9.4）．求めた R は式ごとに記録する．
    fn expr_effects(&mut self, expr: ExprId) -> Row {
        let program = self.program;
        let row = match &program.exprs[expr].kind {
            // `use Async` は誤り（E0414）．R には足さない．
            ExprKind::Use {
                effect: Res::Def(effect),
            } if program.prelude_def("Async") == Some(*effect) => Row::empty(),
            ExprKind::Use {
                effect: Res::Def(effect),
            } => Row::tag(*effect),
            // 本体の R はラムダの型の R に閉じ込める．表に出るのは呼んだとき（9.4）．
            ExprKind::Lambda { body, .. } => {
                let body_row = self.expr_effects(*body);
                if let Some(&(_, v)) = self.lambda_rows.get(expr) {
                    self.ctx
                        .add_subset_of(SetKind::Effects, body_row, Row::var(v), Origin::Lambda);
                }
                Row::empty()
            }
            ExprKind::With { handlers, body } => self.with_effects(expr, handlers, *body),
            ExprKind::Call { callee, .. } => {
                let mut row = self.sub_effects(expr);
                row.extend(&self.callee_uses(*callee));
                row
            }
            // `x |> f(a)` は `f(x, a)`．右辺が呼び出しなら，その呼び出しの式が呼ばれる側の R を足す．
            ExprKind::Binary {
                op: BinaryOp::Pipe,
                rhs,
                ..
            } => {
                let mut row = self.sub_effects(expr);
                if !matches!(program.exprs[*rhs].kind, ExprKind::Call { .. }) {
                    row.extend(&self.callee_uses(*rhs));
                }
                row
            }
            _ => self.sub_effects(expr),
        };
        self.expr_effects.insert(expr, row.clone());
        row
    }

    /// 部分式の R の和．
    fn sub_effects(&mut self, expr: ExprId) -> Row {
        let mut row = Row::empty();
        for e in self.sub_exprs(expr) {
            row.extend(&self.expr_effects(e));
        }
        row
    }

    /// 呼ばれる側の関数型の R．
    fn callee_uses(&mut self, callee: ExprId) -> Row {
        let ty = self.exprs.get(callee).cloned().unwrap_or(Ty::Error);
        match self.ctx.shallow_resolve(&ty) {
            Ty::Fn(f) => f.uses,
            _ => Row::empty(),
        }
    }

    /// `with H1, ..., Hn { body }` の R（8.3，8.5，9.4）．layer は展開し，同じエフェクトのハンドラは
    /// 後ろを残す（`crate::layer::with_entries`）．
    fn with_effects(&mut self, expr: ExprId, handlers: &[(Res, Span)], body: ExprId) -> Row {
        let program = self.program;
        let body_row = self.expr_effects(body);
        let entries = layer::with_entries(program, handlers);
        let deps: Vec<Row> = entries
            .iter()
            .map(|e| self.handler_deps(e.handler))
            .collect();
        let provided: Vec<DefId> = entries.iter().map(|e| e.effect).collect();
        let mut row = self.subtract(&body_row, &provided);
        // ハンドラの依存は，同じ with で別のエフェクトを供給するハンドラが満たしてよい（14.2）．
        // 自分と同じエフェクトは外側のハンドラが満たす．layer の中で満たされない依存も残る（8.5）．
        for (entry, deps) in entries.iter().zip(&deps) {
            let others: Vec<DefId> = provided
                .iter()
                .copied()
                .filter(|e| *e != entry.effect)
                .collect();
            row.extend(&self.subtract(deps, &others));
        }
        self.withs.push((expr, entries, deps));
        row
    }

    /// 集合 `row` から，タグ `tags` を消す．推論の集合変数は，消すタグを除いた解を持つ新しい
    /// 集合変数に置き換える．集合の型引数からは消さない．
    fn subtract(&mut self, row: &Row, tags: &[DefId]) -> Row {
        let mut out = Row::empty();
        for e in self.ctx.resolve_row(row).elems() {
            match e {
                RowElem::Tag(t) if tags.contains(t) => {}
                RowElem::Var(v) if !tags.is_empty() => {
                    let d = self.ctx.new_row_var();
                    self.row_diffs.push((d, *v, tags.to_vec()));
                    out.insert(RowElem::Var(d));
                }
                e => out.insert(e.clone()),
            }
        }
        out
    }

    /// 同じ with に並べたハンドラの初期化の順を決める（14.2）．依存が循環すれば E0411．
    /// 集合を解いた後に，依存の解で確かめる．layer の中だけの循環は layer の宣言で報告する．
    pub(super) fn check_handler_cycles(&mut self) {
        let program = self.program;
        for (expr, entries, deps) in std::mem::take(&mut self.withs) {
            let deps: Vec<Row> = deps.iter().map(|d| self.ctx.resolve_row(d)).collect();
            let order = match layer::init_order(&entries, &deps) {
                Ok(order) => order,
                Err(cycle) => {
                    if !layer::same_inner_layer(&entries, &cycle) {
                        self.diags
                            .push(layer::cycle_diagnostic(program, &entries, &cycle));
                    }
                    (0..entries.len()).collect()
                }
            };
            self.with_handlers
                .insert(expr, order.iter().map(|&i| entries[i].handler).collect());
        }
    }
}
