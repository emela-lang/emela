//! `assert`（仕様 13.2）の出力．
//!
//! 比較の形なら両辺を1回ずつ評価して一時変数に入れ，比較が偽のときだけ両辺を表示関数で
//! 文字列にして `$assertCmp` で defect を投げる．比較でない形は式の値だけを見る．

use emela_core::{Assert, AssertCond, BinOp};

use super::{Emitter, js_string, unparen};

impl Emitter<'_> {
    /// assert の文を出す．値（`()`）は呼び出し側が出す．
    pub(super) fn assert(&mut self, a: &Assert) {
        let text = js_string(&a.text);
        match &a.cond {
            AssertCond::Compare {
                op,
                op_ty,
                ty,
                lhs,
                rhs,
            } => {
                let op_text = match op {
                    BinOp::Eq => "==",
                    BinOp::Ne => "!=",
                    BinOp::Lt => "<",
                    BinOp::Le => "<=",
                    BinOp::Gt => ">",
                    BinOp::Ge => ">=",
                    _ => panic!("不正な IR: assert の比較に {op:?} は使えない"),
                };
                // 両辺は比較と表示の2回使うので，必ず一時変数に入れる．
                let vs = self.values_in_order(&[lhs, rhs]);
                let mut operands = Vec::with_capacity(2);
                for v in vs {
                    let t = self.tmp();
                    self.line(&format!("const {t} = {v};"));
                    operands.push(t);
                }
                let (l, r) = (&operands[0], &operands[1]);
                let test = self.binary_js(*op, *op_ty, l, r);
                let left = self.show_call(ty, l);
                let right = self.show_call(ty, r);
                let fail = self.rt("$assertCmp");
                self.line(&format!("if (!{test}) {{"));
                self.indent += 1;
                self.line(&format!(
                    "{fail}({text}, {}, {left}, {right});",
                    js_string(op_text)
                ));
                self.indent -= 1;
                self.line("}");
            }
            AssertCond::Bool(cond) => {
                let c = self.value(cond);
                let fail = self.rt("$assertFail");
                self.line(&format!("if (!({})) {{", unparen(&c)));
                self.indent += 1;
                self.line(&format!("{fail}({text});"));
                self.indent -= 1;
                self.line("}");
            }
        }
    }
}
