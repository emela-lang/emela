//! Lambdas and local fns.

use emela_resolve::hir::{ExprId, LocalFn, Param, Span};
use smol_str::SmolStr;

use super::{BodyInfer, LocalFnInfo};
use crate::decl::lower::{Omitted, Scope};
use crate::decl::signature::param_labels;
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::infer::{Origin, SetKind};
use crate::ty::{CapElem, CapId, Row, RowElem, Ty};

impl BodyInfer<'_> {
    /// A local fn (`[fn.local]`), checked at its position in the block. Its name is bound to its
    /// function type before the body is checked, so a call to itself in the body has the type
    /// being inferred and is not generalized (`[type.rules]`).
    ///
    /// The signature follows `[type.inference]` as for a top-level fn: parameter types are
    /// written (E3006), an omitted return type is inferred with E, an omitted `fails` with a
    /// return type means no errors, and an omitted `uses` is inferred. The E and R of function
    /// types in the parameters whose clauses are omitted are set variables shared by every use,
    /// and those the body can call are part of the declared E and R. The E and R of the body are
    /// constrained to the type's E and R when E and R are computed (`body/errors.rs`,
    /// `body/effects.rs`); the declaration itself has none.
    pub(super) fn infer_local_fn(&mut self, f: &LocalFn) {
        let program = self.program;
        let outer = self.binder.replace(f.body);
        let mut hidden_fails = Row::empty();
        let mut hidden_uses = Row::empty();
        let mut params = Vec::new();
        for p in &f.sig.params {
            let ty = match p.ty {
                Some(ty) => {
                    let mut scope = Scope {
                        params: &self.type_params,
                        kinds: &self.param_kinds,
                        omitted: Omitted::Local {
                            ctx: &mut self.ctx,
                            positive: &mut hidden_fails,
                            positive_uses: &mut hidden_uses,
                        },
                        sets: true,
                        self_ty: self.self_ty.as_ref(),
                        result_caps: None,
                        quant: None,
                    };
                    self.ty_table
                        .lower_in(program, &mut scope, ty, true, &mut self.diags)
                }
                None => {
                    let local = &program.locals[p.local];
                    self.diags.push(Diagnostic {
                        span: Some(local.span),
                        kind: DiagnosticKind::MissingLocalFnParamType {
                            param: local.name.clone(),
                        },
                    });
                    Ty::Error
                }
            };
            self.set_local(p.local, ty.clone());
            params.push(ty);
        }
        let ret = match f.sig.ret {
            Some(ty) => self.lower_type(ty),
            None => self.ctx.new_var(),
        };
        let scope = Scope {
            params: &self.type_params,
            kinds: &self.param_kinds,
            omitted: Omitted::Empty,
            sets: true,
            self_ty: self.self_ty.as_ref(),
            result_caps: None,
            quant: None,
        };
        let fails_declared = f.sig.ret.is_some();
        let fails = if fails_declared {
            let written = match &f.sig.fails {
                Some(set) => self
                    .ty_table
                    .lower_error_set(program, &scope, set, &mut self.diags),
                None => Row::empty(),
            };
            written.union(&hidden_fails)
        } else {
            Row::var(self.ctx.new_row_var())
        };
        let uses = match &f.sig.uses {
            Some(set) => self
                .ty_table
                .lower_effect_set(program, &scope, set, &mut self.diags)
                .union(&hidden_uses),
            None => Row::var(self.ctx.new_row_var()),
        };
        let ty = Ty::func_sets(params, ret.clone(), fails.clone(), uses.clone());
        // The name belongs to the enclosing body; the parameters and the body to the local fn.
        let inner = std::mem::replace(&mut self.binder, outer);
        self.set_local(f.local, ty);
        self.binder = inner;
        self.local_fns.insert(
            f.local,
            LocalFnInfo {
                labels: param_labels(program, &f.sig.params),
                body: f.body,
                fails,
                uses,
                fails_declared,
                fails_written: f.sig.fails.is_some(),
                uses_written: f.sig.uses.is_some(),
            },
        );
        self.name_providers.push(None);
        self.ctx.scopes.enter(f.body);
        self.check_expr(f.body, &ret);
        self.ctx.scopes.exit();
        self.name_providers.pop();
        self.binder = outer;
    }

    /// If the expected type is a function type with the same number of parameters, its parameter
    /// types are passed to the lambda's parameters first. The body's E and R do not escape; they
    /// are enclosed in the function type's E and R (fresh set variables) (spec 9.4). The
    /// constraints putting the body's E and R into those set variables are added when the body's
    /// E and R are computed (`body/errors.rs`, `body/effects.rs`).
    pub(super) fn infer_lambda(
        &mut self,
        expr: ExprId,
        params: &[Param],
        body: ExprId,
        span: Span,
        expected: Option<&Ty>,
    ) -> Ty {
        let expected_fn = match expected.map(|ty| self.ctx.shallow_resolve(ty)) {
            Some(Ty::Fn(f)) if f.params.len() == params.len() => Some(*f),
            _ => None,
        };
        // Checked against a function type that quantifies capability sets, the closure receives
        // an unknown set for each (`[type.rules]`): a capability of its own scope.
        let quantified = expected_fn.as_ref().is_some_and(|f| f.quantifies());
        let mut rigid = Vec::new();
        let expected_fn = match expected_fn {
            Some(f) if quantified => {
                // Named after the parameter whose type has it (`s` of `|s|`).
                let names: Vec<Option<SmolStr>> = f
                    .params
                    .iter()
                    .zip(params)
                    .map(|(ty, p)| {
                        let name = &self.program.locals[p.local].name;
                        let mut has = false;
                        ty.visit(&mut |t| {
                            if let Ty::Cap(_, row) | Ty::Set(SetKind::Caps, row) = t {
                                has |= row.caps().any(|c| matches!(c.id, CapId::Quant(_)));
                            }
                        });
                        (has && name != "_").then(|| name.clone())
                    })
                    .collect();
                let opened = Ty::Fn(Box::new(f.clone())).open_quant(&mut |i, c| {
                    let name = f
                        .params
                        .iter()
                        .position(|ty| {
                            let mut has = false;
                            ty.visit(&mut |t| {
                                if let Ty::Cap(_, row) | Ty::Set(SetKind::Caps, row) = t {
                                    has |= row
                                        .caps()
                                        .any(|c| matches!(c.id, CapId::Quant(j) if j == i));
                                }
                            });
                            has
                        })
                        .and_then(|k| names[k].clone())
                        .unwrap_or_else(|| c.name.clone());
                    let cap = CapElem {
                        id: CapId::Rigid {
                            binder: expr,
                            index: i,
                        },
                        name,
                        tag: c.tag.clone(),
                    };
                    rigid.push(cap.clone());
                    Row::from_elems([RowElem::Cap(cap)])
                });
                match opened {
                    Ty::Fn(f) => Some(*f),
                    _ => None,
                }
            }
            f => f,
        };
        if quantified {
            self.quantified_lambdas.insert(expr, rigid);
        }
        let outer = self.binder.replace(expr);
        self.name_providers.push(None);
        let provided = self.provided_to_lambda.take();
        let by_name = provided.is_some();
        if let Some(provided) = provided {
            self.name_providers.push(Some((expr, provided)));
        }
        self.ctx.scopes.enter(expr);

        let mut param_tys = Vec::new();
        for (i, param) in params.iter().enumerate() {
            let from_context = expected_fn.as_ref().map(|f| f.params[i].clone());
            let ty = match (param.ty, from_context) {
                (Some(annotation), from_context) => {
                    let ty = self.lower_type(annotation);
                    if let Some(from_context) = from_context {
                        let span = self.program.locals[param.local].span;
                        self.coerce_at(span, &from_context, &ty);
                    }
                    ty
                }
                (None, Some(from_context)) => from_context,
                (None, None) => self.ctx.new_var(),
            };
            self.set_local(param.local, ty.clone());
            param_tys.push(ty);
        }

        let ret_ty = match &expected_fn {
            Some(f) => {
                self.check_expr(body, &f.ret);
                f.ret.clone()
            }
            None => self.infer_expr(body),
        };
        self.ctx.scopes.exit();
        if by_name {
            self.name_providers.pop();
        }
        self.name_providers.pop();
        self.binder = outer;

        let row = self.ctx.new_row_var();
        let uses = self.ctx.new_row_var();
        self.lambda_rows.insert(expr, (row, uses));
        let ty = Ty::func_sets(param_tys, ret_ty, Row::var(row), Row::var(uses));
        let ty = if quantified {
            self.requantify(&ty, expr)
        } else {
            ty
        };
        let origin = Origin::Coerce {
            span: Some(span),
            def: self.current,
        };
        match (expected, &expected_fn) {
            // The lambda's E and R are contained in the expected type's E and R.
            (_, Some(f)) => {
                self.ctx
                    .add_subset(Row::var(row), f.fails.clone(), origin.clone());
                self.ctx
                    .add_subset_of(SetKind::Effects, Row::var(uses), f.uses.clone(), origin);
            }
            // If the expected type was not a function type (or had a different parameter count),
            // check here to report the error.
            (Some(expected), None) => self.coerce_at(span, expected, &ty),
            (None, None) => {}
        }
        ty
    }
}
