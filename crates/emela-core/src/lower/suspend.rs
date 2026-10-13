//! 中断しうるかの印（8.1，9.5，16.5）．
//!
//! 関数，ラムダ，呼び出しの印は，型の効果の集合 R が Immediate でないか（中断しうるか）で決める．
//! R の要素ごとに，エフェクトは suspend fn を持つか Async なら中断しうる（9.5）．操作の呼び出しは
//! 操作の宣言（`suspend fn` か）で決め，fn の操作の実装は，本体の R が Immediate でなく，本体の中に
//! 中断しうる呼び出しの印もあるときだけ中断しうるとする（E0417，8.1）．
//!
//! 集合の型引数（`List.map` の `R(f)` など）は，具体化しだいで中断しうるかが変わる．中断しうる関数は
//! ジェネレータにし，呼び出しの形も変わるので，そういう関数は，中断しうる集合に具体化した呼び出し用に
//! 「中断用の写し」を別の IR の関数として作る．どの型引数が中断しうるかは [`Specialization`]（効果の
//! 集合の型引数のうち，中断しうる集合に具体化したもののビット）で表す．0 は全部が Immediate の場合で，
//! 元の関数そのもの（今までと同じ出力）．参照ごとに具体化した集合（`ProgramTypes::type_rows`）から
//! どれを呼ぶかを決める．中断用の写しの本体の中では，集合の型引数が中断しうるかが決まっているので，ラムダや関数の値の呼び出しの印も型から一意に決まる．

use emela_resolve::hir::{DefId, ExprId, Item};
use emela_types::{Row, RowElem, Scheme, SetKind, Ty};

use super::Lowerer;
use crate::ir::Expr;

/// どの中断用の写しか．効果の集合の型引数のうち，中断しうる集合に具体化したものの添字のビット．
/// 0 は元の関数（全部 Immediate）．
pub(super) type Specialization = u64;

/// 組み込みの制約 Immediate の名前（9.5）．
const IMMEDIATE: &str = "Immediate";

impl Lowerer<'_> {
    /// エフェクトが Immediate か（9.5）．suspend fn を1つも持たず，Async でない．
    pub(super) fn is_immediate_effect(&self, effect: DefId) -> bool {
        let hir = self.hir;
        if hir.prelude_def("Async") == Some(effect) {
            return false;
        }
        match hir.item(effect) {
            Some(Item::Effect(e)) => e
                .ops
                .iter()
                .all(|&op| !matches!(hir.item(op), Some(Item::Op(o)) if o.sig.is_suspend)),
            _ => true,
        }
    }

    /// 本体を写している関数の中の集合 `row` が中断しうるか．集合の型引数（と，推論で汎化した集合
    /// 変数）は，写している関数（元の関数か，どの中断用の写しか）で決まる．
    pub(super) fn row_suspends(&self, row: &Row) -> bool {
        row.elems().iter().any(|e| match e {
            RowElem::Tag(effect) => !self.is_immediate_effect(effect.def),
            RowElem::Bound(i) => spec_has(self.spec, *i),
            RowElem::Var(v) => self
                .current_def
                .and_then(|def| self.types.generalized_rows.get(def))
                .and_then(|rows| rows.iter().find(|(w, _)| w == v))
                .is_some_and(|&(_, i)| spec_has(self.spec, i)),
            RowElem::Of(_) => false,
        })
    }

    /// 本体を写している関数の中の関数型 `ty` の値を呼ぶと中断しうるか．
    pub(super) fn ty_suspends(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Fn(f) if self.row_suspends(&f.uses))
    }

    /// 定義 `def` の写し `spec` が中断しうるか（ジェネレータにするか）．宣言の型の R で
    /// 決める．
    pub(super) fn fn_suspends(&self, def: DefId, spec: Specialization) -> bool {
        let Some(Ty::Fn(f)) = self.types.sigs.get(def).map(|s| &s.scheme.ty) else {
            return false;
        };
        f.uses.elems().iter().any(|e| match e {
            RowElem::Tag(effect) => !self.is_immediate_effect(effect.def),
            RowElem::Bound(i) => spec_has(spec, *i),
            _ => false,
        })
    }

    /// 定義 `def` を式 `expr` で参照したときに呼ぶ写し．`expr` が `None` なら（参照の式がない根）
    /// 元の関数（0）．
    pub(super) fn specialization_of(&self, def: DefId, expr: Option<ExprId>) -> Specialization {
        let Some(sig) = self.types.sigs.get(def) else {
            return 0;
        };
        let candidates = effect_params(&sig.scheme);
        if candidates == 0 {
            return 0;
        }
        let Some(expr) = expr else {
            return 0;
        };
        let mut spec = 0;
        match self.types.type_rows.get(expr) {
            Some(rows) => {
                for (i, row) in rows.iter().enumerate() {
                    if spec_has(candidates, i as u32) && self.row_suspends(row) {
                        spec |= 1 << i;
                    }
                }
            }
            // 自分自身の参照は具体化しない（型検査の `instantiate_def`）ので，今写している関数と同じ．
            None if self.current_def == Some(def) => return self.spec,
            // 同じまとまりの中でまだ汎化していない定義の参照も具体化しない．その定義の型引数は，
            // 汎化した集合変数をそのまま共有している．
            None => {
                for &(v, i) in self.types.generalized_rows.get(def).into_iter().flatten() {
                    if spec_has(candidates, i) && self.row_suspends(&Row::var(v)) {
                        spec |= 1 << i;
                    }
                }
            }
        }
        spec
    }
}

/// 写した式 `e` が，中断しうる呼び出し（`Expr::Call::suspends`）を含むか．無名関数の本体の中は
/// 見ない（その無名関数を呼んだところで，関数の値の呼び出しの印になる）．呼び出しの印は型検査の
/// E0417 と同じ判定（操作の呼び出しは操作の宣言，関数の呼び出しは型の R）で付けてある．
pub(super) fn has_suspending_call(e: &Expr) -> bool {
    match e {
        Expr::Lambda { .. } => false,
        Expr::Call { suspends: true, .. } => true,
        Expr::Fiber { op, .. } if op.suspends() => true,
        _ => {
            let mut found = false;
            e.for_each_child(&mut |c| found = found || has_suspending_call(c));
            found
        }
    }
}

/// 写し `spec` で `i` 番目の型引数が中断しうるか．
fn spec_has(spec: Specialization, i: u32) -> bool {
    i < Specialization::BITS && spec & (1 << i) != 0
}

/// スキームの型引数のうち，効果の集合として使われ，Immediate の制約を持たないもの（中断用の
/// 写しのビットの候補）．
fn effect_params(scheme: &Scheme) -> Specialization {
    let mut out = 0;
    collect_effect_params(&scheme.ty, &mut out);
    for (i, p) in scheme.params.iter().enumerate() {
        if p.bounds.iter().any(|b| b == IMMEDIATE) && (i as u32) < Specialization::BITS {
            out &= !(1 << i);
        }
    }
    out
}

fn collect_effect_params(ty: &Ty, out: &mut Specialization) {
    let mut row = |row: &Row| {
        for e in row.elems() {
            if let RowElem::Bound(i) = e
                && *i < Specialization::BITS
            {
                *out |= 1 << i;
            }
        }
    };
    match ty {
        Ty::Fn(f) => row(&f.uses),
        Ty::Set(SetKind::Effects, r) => row(r),
        _ => {}
    }
    ty.for_each_child(|child| collect_effect_params(child, out));
}
