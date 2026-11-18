//! Values that cannot leave a `Fiber.scope` by their type (E0425, `[fiber.escape]`).
//!
//! A capability value is an ordinary value (`[effect.use]`): it can be passed as an argument,
//! stored in a data structure and returned. Leaving a `with` or a `Fiber.scope` while holding a
//! capability (capture sets, `[effect.escape]`) is checked in `crate::capture`.

use emela_resolve::hir::ExprId;
use smol_str::SmolStr;

use super::BodyInfer;
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::ty::Ty;

impl BodyInfer<'_> {
    /// The value of a scope function (`Fiber.scope`) cannot contain a fiber handle or a Deferred
    /// (E0425, spec 11.8): every child has ended once the scope returns. Looks anywhere in the
    /// type (`Option[Fiber[User]]` too).
    pub(super) fn check_scope_values(&mut self) {
        if self.fibers.scope_fns.is_empty() {
            return;
        }
        let exprs: Vec<ExprId> = self.exprs.iter().map(|(e, _)| e).collect();
        for expr in exprs {
            let Some((scope, _)) = self.scope_call(expr) else {
                continue;
            };
            let Some(ty) = self.exprs.get(expr).cloned() else {
                continue;
            };
            let ty = self.ctx.resolve(&ty);
            if let Some(handle) = self.handle_in(&ty) {
                self.diags.push(Diagnostic {
                    span: Some(self.program.exprs[expr].span),
                    kind: DiagnosticKind::EscapingFromScope { ty: handle, scope },
                });
            }
        }
    }

    /// The name of the first fiber handle or Deferred type found in `ty`.
    fn handle_in(&self, ty: &Ty) -> Option<SmolStr> {
        if let Ty::Named(con, _) = ty {
            let def = self.ty_table.def(*con);
            if self.fibers.handles.contains(&def) {
                return Some(self.program.defs[def].name.clone());
            }
        }
        let mut found = None;
        ty.for_each_child(|child| {
            if found.is_none() {
                found = self.handle_in(child);
            }
        });
        found
    }
}
