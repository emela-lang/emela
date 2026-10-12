//! Emits `assert` (spec 13.2).
//!
//! For a comparison, each side is evaluated once into a temporary. Only when the comparison is
//! false are both sides turned into strings with their show functions and a defect thrown with
//! `$assertCmp`. Other forms only look at the value of the expression.

use emela_core::{Assert, AssertCond, BinOp};

use super::{Emitter, js_string, unparen};

impl Emitter<'_> {
    /// The value (`()`) is emitted by the caller.
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
                    _ => panic!("invalid IR: {op:?} cannot be used in an assert comparison"),
                };
                // Both sides are used twice (compare and show), so always bind them to temporaries.
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
