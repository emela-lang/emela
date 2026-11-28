//! Arguments: matched to the parameters by position and label, and checked against their types,
//! capability arguments included.

use emela_resolve::hir::{Arg, ExprId, ExprKind, Label, LocalKind, Res, Span};
use smol_str::SmolStr;

use super::{Assigned, BodyInfer};
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::ty::{CapElem, CapId, Row, RowElem, Tag, Ty};

impl BodyInfer<'_> {
    /// Assigns `args` to the parameters `params` ([`Self::assign_args`]), reports a wrong number
    /// of them at `span` (E3101), checks each argument against its parameter's type and infers
    /// the extra ones (spec 6.2). Returns the arguments in parameter order. Shared by calls and
    /// the arguments of init (`[effect.handler]`).
    pub(super) fn check_args(
        &mut self,
        callee_name: Option<&SmolStr>,
        labels: Option<&[Option<SmolStr>]>,
        params: &[Ty],
        args: &[Arg],
        span: Span,
    ) -> Vec<Option<ExprId>> {
        let assigned = self.assign_args(
            callee_name,
            labels,
            params.len(),
            args.iter().map(|arg| (arg.label.as_ref(), arg.value)),
        );
        if !assigned.reported && (assigned.too_many || assigned.has_missing()) {
            self.diags.push(Diagnostic {
                span: Some(span),
                kind: DiagnosticKind::WrongArgCount {
                    callee: callee_name.cloned(),
                    expected: params.len(),
                    found: args.len(),
                },
            });
        }
        let own = self.callee_uses.take();
        for (slot, param_ty) in assigned.slots.iter().zip(params) {
            if let Some(arg) = slot {
                // A function whose parameter's R writes out an effect that its own R does not
                // have provides that effect by name to the argument (`[effect.escape]`).
                if let Some(own) = &own
                    && matches!(self.program.exprs[*arg].kind, ExprKind::Lambda { .. })
                    && let Ty::Fn(f) = self.ctx.shallow_resolve(param_ty)
                {
                    let provided: Vec<Tag> = f
                        .uses
                        .effect_tags()
                        .filter(|t| !own.effect_tags().any(|o| o.def == t.def))
                        .cloned()
                        .collect();
                    if !provided.is_empty() {
                        self.provided_to_lambda = Some(provided);
                    }
                }
                if let Ty::Cap(wanted, _) = self.ctx.shallow_resolve(param_ty) {
                    self.check_cap_arg(*arg, &wanted, param_ty);
                } else {
                    self.check_expr(*arg, param_ty);
                }
            }
        }
        for extra in assigned.extra {
            self.infer_expr(extra);
        }
        assigned.slots
    }

    /// An argument passed to a parameter of type `use F1 + ... + Fm` (`[effect.use]`). A
    /// capability of `use E1 + ... + En` can be passed when every Fi is one of the Ej; when the
    /// parameter has fewer effects, the argument is recorded in `narrowings`, and only the
    /// operations of the Fi can be called through the parameter. A capability without some Fi is
    /// E4403. Anything else is checked against the parameter type as usual.
    pub(super) fn check_cap_arg(&mut self, arg: ExprId, wanted: &[Tag], param_ty: &Ty) {
        let actual = self.infer_expr(arg);
        let actual = self.self_as_capability(arg, actual);
        let span = self.program.exprs[arg].span;
        let Ty::Cap(have, have_caps) = self.ctx.shallow_resolve(&actual) else {
            self.coerce_at(span, param_ty, &actual);
            return;
        };
        let wanted_caps = match self.ctx.shallow_resolve(param_ty) {
            Ty::Cap(_, caps) => caps,
            _ => Row::empty(),
        };
        let mut missing = Vec::new();
        let mut matched = Vec::with_capacity(wanted.len());
        for w in wanted {
            match have
                .iter()
                .find(|h| h.def == w.def && h.args.len() == w.args.len())
            {
                Some(h) => matched.push((w.clone(), h.clone())),
                None => missing.push(self.program.defs[w.def].name.clone()),
            }
        }
        if !missing.is_empty() {
            let cons = &self.ty_table.cons;
            let expected = SmolStr::new(self.ctx.resolve(param_ty).display(cons).to_string());
            let found = SmolStr::new(self.ctx.resolve(&actual).display(cons).to_string());
            self.diags.push(Diagnostic {
                span: Some(span),
                kind: DiagnosticKind::MissingCapabilityEffects {
                    expected,
                    found,
                    missing,
                },
            });
            return;
        }
        // The type arguments of each effect kept must match (spec 8.1), and the capabilities
        // the argument may be go into the set of the parameter (`[type.rules]`).
        for (w, h) in &matched {
            self.coerce_at(
                span,
                &Ty::cap([w.clone()], wanted_caps.clone()),
                &Ty::cap([h.clone()], have_caps.clone()),
            );
        }
        if have.len() > wanted.len() {
            self.narrowings.insert(arg, wanted.to_vec());
        }
    }

    /// `self` in an operation body or in release, passed to a parameter of type `use E`, is the
    /// capability of the handler (`[effect.handler]`): its type is recorded as such, and lowering
    /// makes the handler's value from the state.
    pub(super) fn self_as_capability(&mut self, arg: ExprId, actual: Ty) -> Ty {
        let program = self.program;
        let ExprKind::Path(Res::Local(l)) = program.exprs[arg].kind else {
            return actual;
        };
        let Some(handler) = self.current.filter(|_| self.in_handler()) else {
            return actual;
        };
        if program.locals[l].kind != LocalKind::SelfParam || self.in_init.is_some() {
            return actual;
        }
        let tags = self.sigs.handler_effects(handler).to_vec();
        let Some(first) = tags.first().cloned() else {
            return actual;
        };
        let cap = Ty::cap(
            tags,
            Row::from_elems([RowElem::Cap(CapElem {
                id: CapId::SelfOf(handler),
                name: SmolStr::new("self"),
                tag: first,
            })]),
        );
        self.exprs.insert(arg, cap.clone());
        self.self_caps.insert(arg);
        cap
    }

    /// Reorders arguments by parameter position: positional arguments from the front, named
    /// arguments by label. Used both for call arguments (`ExprId`) and constructor pattern
    /// arguments (`PatId`).
    ///
    /// `labels` are the callee's parameter labels, or `None` for a value not referred to by name
    /// (a local variable, a lambda). Label errors are reported here; count errors are reported by
    /// the caller, since calls and patterns have different rules.
    pub(super) fn assign_args<'l, T: Copy>(
        &mut self,
        callee_name: Option<&SmolStr>,
        labels: Option<&[Option<SmolStr>]>,
        n_params: usize,
        args: impl IntoIterator<Item = (Option<&'l Label>, T)>,
    ) -> Assigned<T> {
        let mut assigned = Assigned {
            slots: vec![None; n_params],
            extra: Vec::new(),
            reported: false,
            too_many: false,
        };
        let mut next_positional = 0;

        for (label, value) in args {
            match label {
                None => {
                    if next_positional < n_params {
                        assigned.slots[next_positional] = Some(value);
                    } else {
                        assigned.extra.push(value);
                        assigned.too_many = true;
                    }
                    next_positional += 1;
                }
                Some(label) => {
                    match label_slot(callee_name, labels, &assigned.slots, &label.name) {
                        Ok(i) => assigned.slots[i] = Some(value),
                        Err(kind) => {
                            self.diags.push(Diagnostic {
                                span: Some(label.span),
                                kind,
                            });
                            assigned.reported = true;
                            assigned.extra.push(value);
                        }
                    }
                }
            }
        }

        assigned
    }
}

/// The position for a named argument, or the diagnostic explaining why it cannot be placed.
fn label_slot<T>(
    callee_name: Option<&SmolStr>,
    labels: Option<&[Option<SmolStr>]>,
    slots: &[Option<T>],
    label: &SmolStr,
) -> Result<usize, DiagnosticKind> {
    let labels = labels.ok_or_else(|| DiagnosticKind::LabelOnUnnamedCallee {
        label: label.clone(),
    })?;

    let i = labels
        .iter()
        .position(|l| l.as_ref() == Some(label))
        .ok_or_else(|| DiagnosticKind::UnknownLabel {
            callee: callee_name.cloned().unwrap_or_default(),
            label: label.clone(),
        })?;

    if slots[i].is_some() {
        return Err(DiagnosticKind::DuplicateArg {
            label: label.clone(),
        });
    }

    Ok(i)
}
