//! Emits `assert` (spec 13.2).
//!
//! For a comparison, each side is evaluated once into a temporary. Only when the comparison is
//! false are both sides turned into strings with their show functions and a defect thrown with
//! `$assertCmp`. Other forms only look at the value of the expression.

use emela_core::{Assert, AssertCond, BinOp, TypeId};

use super::{Emitter, js_string, unparen};

impl Emitter<'_> {
    /// The value (`()`) is emitted by the caller.
    pub(super) fn assert(&mut self, a: &Assert) {
        match &a.cond {
            AssertCond::Compare {
                op,
                op_ty,
                ty,
                lhs,
                rhs,
            } => {
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
                self.assert_compare(a, *op, *ty, &test, l, r);
            }
            // The sides are variables bound before the assert.
            AssertCond::Shown {
                cond,
                op,
                ty,
                lhs,
                rhs,
            } => {
                let c = self.value(cond);
                let test = format!("({})", unparen(&c));
                let (l, r) = (self.value(lhs), self.value(rhs));
                self.assert_compare(a, *op, *ty, &test, &l, &r);
            }
            AssertCond::Bool(cond) => {
                let c = self.value(cond);
                let fail = self.rt("$assertFail");
                self.line(&format!("if (!({})) {{", unparen(&c)));
                self.indent += 1;
                let args = fail_args(a);
                self.line(&format!("{fail}({args});"));
                self.indent -= 1;
                self.line("}");
            }
        }
    }

    /// Unless `test`, a defect showing both sides `l` and `r` with `$assertCmp`.
    fn assert_compare(&mut self, a: &Assert, op: BinOp, ty: TypeId, test: &str, l: &str, r: &str) {
        let op_text = match op {
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            _ => panic!("invalid IR: {op:?} cannot be used in an assert comparison"),
        };
        let left = self.show_call(&ty, l);
        let right = self.show_call(&ty, r);
        let fail = self.rt("$assertCmp");
        self.line(&format!("if (!{test}) {{"));
        self.indent += 1;
        let text = js_string(&a.text);
        let at = a
            .location
            .as_deref()
            .map(|at| format!(", {}", js_string(at)))
            .unwrap_or_default();
        self.line(&format!(
            "{fail}({text}, {}, {left}, {right}{at});",
            js_string(op_text)
        ));
        self.indent -= 1;
        self.line("}");
    }
}

/// The arguments of `$assertFail`: the source text, and the location if the assert has one. The
/// location goes to the defect outside its message (`[test.assert]`).
fn fail_args(a: &Assert) -> String {
    let text = js_string(&a.text);
    match &a.location {
        Some(at) => format!("{text}, {}", js_string(at)),
        None => text,
    }
}
