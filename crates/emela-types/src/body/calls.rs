//! Calls of functions, constructors and capability operations.

use emela_resolve::hir::{Arg, ExprId, ExprKind, Res, Span};

use super::{BodyInfer, methods};
use crate::FnTy;
use crate::row::Row;
use crate::ty::Ty;

impl BodyInfer<'_> {
    /// Reorders the arguments by parameter position, then checks types (spec 6.2). `expr` is
    /// where the reordered arguments are recorded (the call or the pipe expression).
    pub(super) fn infer_call(
        &mut self,
        expr: ExprId,
        callee: ExprId,
        args: &[Arg],
        span: Span,
    ) -> Ty {
        let program = self.program;
        let sigs = self.sigs;

        // A local fn called by its name takes named arguments (`[fn.args]`). Its labels are
        // recorded at its declaration, which comes before every call to it.
        let local_labels = match &program.exprs[callee].kind {
            ExprKind::Path(Res::Local(local)) => self
                .local_fns
                .get(*local)
                .map(|info| (&program.locals[*local].name, info.labels.clone())),
            _ => None,
        };
        let (callee_name, labels) = match (&program.exprs[callee].kind, &local_labels) {
            (ExprKind::Path(Res::Def(def)), _) => (
                Some(&program.defs[*def].name),
                sigs.get(*def).map(|sig| sig.labels.as_slice()),
            ),
            (_, Some((name, labels))) => (Some(*name), Some(labels.as_slice())),
            _ => (None, None),
        };
        // A handler member is opened by calling it (`Files.Open(p)`, `fs.Open(p)`).
        self.member_callees.insert(callee);
        // `x.f(a)` may be a method call (`[type.rules]`).
        let callee_ty = match &program.exprs[callee].kind {
            ExprKind::Field { base, name } => {
                let base_ty = self.infer_expr(*base);
                match self.infer_dot(callee, *base, &base_ty, name, true) {
                    methods::Dot::Method(def) => {
                        return self.infer_method_call(
                            expr, callee, *base, &base_ty, def, name, args, span,
                        );
                    }
                    methods::Dot::Value(ty) => {
                        self.exprs.insert(callee, ty.clone());
                        ty
                    }
                }
            }
            _ => self.infer_expr(callee),
        };
        // Capability operations (`log.info(...)`) use the declared operation's parameter names as
        // labels.
        let (callee_name, labels) = match (callee_name, self.ops.get(callee)) {
            (None, Some(&op)) => (
                Some(&program.defs[op].name),
                sigs.get(op).map(|sig| sig.labels.as_slice()),
            ),
            _ => (callee_name, labels),
        };

        let f: FnTy = match self.ctx.shallow_resolve(&callee_ty) {
            // A function of a quantified type takes the sets of its arguments, as a capability
            // parameter does (`[type.rules]`): each call puts a fresh set variable in their place.
            // The callee's type is recorded opened, so that its R names them.
            Ty::Fn(f) if f.quantifies() => {
                let opened = Ty::Fn(f).open_quant(&mut |_, _| Row::var(self.ctx.new_row_var()));
                self.exprs.insert(callee, opened.clone());
                match opened {
                    Ty::Fn(f) => *f,
                    _ => unreachable!("a function type stays one"),
                }
            }
            Ty::Fn(f) => *f,
            Ty::Error => {
                for arg in args {
                    self.infer_expr(arg.value);
                }
                return Ty::Error;
            }
            _ => {
                let params: Vec<Ty> = (0..args.len()).map(|_| self.ctx.new_var()).collect();
                let ret = self.ctx.new_var();
                let fails = Row::var(self.ctx.new_row_var());
                let uses = Row::var(self.ctx.new_row_var());
                let expected =
                    Ty::func_sets(params.clone(), ret.clone(), fails.clone(), uses.clone());
                let callee_span = program.exprs[callee].span;
                self.coerce_at(callee_span, &expected, &callee_ty);
                FnTy {
                    params,
                    ret,
                    fails,
                    uses,
                }
            }
        };

        self.callee_uses = Some(f.uses.clone());
        let slots = self.check_args(callee_name, labels, &f.params, args, span);
        self.calls.insert(expr, slots);

        f.ret
    }
}
