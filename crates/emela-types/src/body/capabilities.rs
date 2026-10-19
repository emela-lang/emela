//! Capability value restrictions (spec 8.2): a capability value cannot be passed as an argument,
//! stored in a data structure, or returned.
//!
//! Checked once types are settled (after sets are solved). Capability values can travel through
//! type parameters and lambda parameters, so this looks at expression types. Only the top-level
//! type of a value is checked; a capability nested inside (`List[use Logger]`, ...) is already
//! an error at the expression that built it.
//!
//! Escaping a `with` (capture sets, spec 8.6) is in scope for 0.22 and not checked here.

use emela_resolve::hir::{BinaryOp, DefKind, ExprId, ExprKind, Item, Res};
use smol_str::SmolStr;

use super::{BodyInfer, fn_parts};
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::ty::Ty;

impl BodyInfer<'_> {
    pub(super) fn check_capabilities(&mut self) {
        let program = self.program;
        let exprs: Vec<ExprId> = self.exprs.iter().map(|(e, _)| e).collect();
        for expr in exprs {
            match &program.exprs[expr].kind {
                ExprKind::Call { callee, args } => {
                    let kind = if self.is_ctor(*callee) {
                        DiagnosticKind::CapabilityInData
                    } else {
                        DiagnosticKind::CapabilityArgument
                    };
                    for arg in args {
                        self.reject_cap(arg.value, kind.clone());
                    }
                }
                ExprKind::Binary {
                    op: BinaryOp::Pipe,
                    lhs,
                    rhs,
                } => {
                    let callee = match &program.exprs[*rhs].kind {
                        ExprKind::Call { callee, .. } => *callee,
                        _ => *rhs,
                    };
                    let kind = if self.is_ctor(callee) {
                        DiagnosticKind::CapabilityInData
                    } else {
                        DiagnosticKind::CapabilityArgument
                    };
                    self.reject_cap(*lhs, kind);
                }
                ExprKind::Tuple(elems) => {
                    for &e in elems {
                        self.reject_cap(e, DiagnosticKind::CapabilityInData);
                    }
                }
                ExprKind::List { elems, rest } => {
                    for &e in elems.iter().chain(rest) {
                        self.reject_cap(e, DiagnosticKind::CapabilityInData);
                    }
                }
                ExprKind::Update { fields, .. } => {
                    for field in fields {
                        self.reject_cap(field.value, DiagnosticKind::CapabilityInData);
                    }
                }
                // A lambda that just returns its parameter returns a capability passed in as an
                // argument, which is already an error at the caller.
                ExprKind::Lambda { params, body } => {
                    let value = tail(program, *body);
                    let returns_param = matches!(
                        program.exprs[value].kind,
                        ExprKind::Path(Res::Local(local)) if params.iter().any(|p| p.local == local)
                    );
                    if !returns_param {
                        self.reject_cap(value, DiagnosticKind::CapabilityReturned);
                    }
                }
                _ => {}
            }
        }

        // The values of fn bodies and handler operation bodies.
        for def in self.defs.clone() {
            let bodies: Vec<ExprId> = match program.item(def) {
                Some(Item::Handler(item)) => item.ops.iter().filter_map(|op| op.body).collect(),
                _ => fn_parts(program, def)
                    .and_then(|p| p.body)
                    .into_iter()
                    .collect(),
            };
            for body in bodies {
                self.reject_cap(tail(program, body), DiagnosticKind::CapabilityReturned);
            }
        }
    }

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

    /// Whether the callee is a constructor (its arguments go into a data structure).
    fn is_ctor(&self, callee: ExprId) -> bool {
        match self.program.exprs[callee].kind {
            ExprKind::Path(Res::Def(def)) => {
                matches!(self.program.defs[def].kind, DefKind::Ctor | DefKind::Error)
            }
            _ => false,
        }
    }

    /// Pushes a `kind` diagnostic on `expr` if its value is a capability value.
    fn reject_cap(&mut self, expr: ExprId, kind: DiagnosticKind) {
        let Some(ty) = self.exprs.get(expr).cloned() else {
            return;
        };
        if let Ty::Cap(_) = self.ctx.resolve(&ty) {
            self.diags.push(Diagnostic {
                span: Some(self.program.exprs[expr].span),
                kind,
            });
        }
    }
}

/// The expression producing a body's value, following the last expression of blocks.
fn tail(program: &emela_resolve::hir::Program, mut expr: ExprId) -> ExprId {
    while let ExprKind::Block(block) = &program.exprs[expr].kind {
        match block.tail {
            Some(t) => expr = t,
            None => break,
        }
    }
    expr
}
