//! References to definitions: instantiation of their declared types, and the trait bounds their
//! type arguments must satisfy.

use emela_resolve::hir::{DefId, DefKind, Span};

use super::{BodyInfer, InstanceSet, SelfObligation, TraitObligation};
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::infer::SetKind;
use crate::row::{Row, RowElem};
use crate::ty::Ty;

impl BodyInfer<'_> {
    /// Replaces the declared type's type parameters with fresh type variables.
    ///
    /// Definitions in the same group without a declared type use their monomorphic type as is
    /// (not instantiated).
    pub(super) fn infer_def_ref(&mut self, def: DefId, span: Span) -> Ty {
        self.instantiate_def(def, span).0
    }

    /// Gives the reference's function type `instance` its own E and R where the referenced
    /// definition's are inferred (contain set variables). Their solutions can name that
    /// definition's type parameters and invisible sets, so they are placed with the reference's
    /// arguments once solved ([`InstanceSet`]) instead of being shared.
    ///
    /// `ty` is the referenced definition's monomorphic type, resolved.
    pub(super) fn instance_sets(&mut self, ty: &Ty, instance: Ty, args: &[Ty], rows: &[Row]) -> Ty {
        let (Ty::Fn(def_fn), Ty::Fn(f)) = (ty, &instance) else {
            return instance;
        };
        let place = |inferred: &Row, kind: SetKind, this: &mut Self| -> Option<Row> {
            inferred.vars().next()?;
            let v = this.ctx.new_row_var();
            this.instance_sets.push(InstanceSet {
                inferred: inferred.clone(),
                instance: v,
                kind,
                args: args.to_vec(),
                rows: rows.to_vec(),
            });
            Some(Row::var(v))
        };
        let fails = place(&def_fn.fails, SetKind::Errors, self);
        let uses = place(&def_fn.uses, SetKind::Effects, self);
        if fails.is_none() && uses.is_none() {
            return instance;
        }
        Ty::func_sets(
            f.params.clone(),
            f.ret.clone(),
            fails.unwrap_or_else(|| f.fails.clone()),
            uses.unwrap_or_else(|| f.uses.clone()),
        )
    }

    /// Remembers that `ty` must satisfy the Prelude's trait `name` at `span`: the operand of
    /// `==` (Eq) or a comparison (Ord, Float aside), or an interpolated value (Show) (spec 10.3).
    pub(super) fn require_prelude_trait(&mut self, ty: &Ty, name: &str, span: Span) {
        let Some(trait_def) = self.program.prelude_def(name) else {
            return;
        };
        self.trait_obligations.push(TraitObligation {
            ty: ty.clone(),
            trait_def,
            span,
            owner: self.current,
            float_ok: name == "Ord",
        });
    }

    /// Remembers the `Self` of a trait function whose parameters do not mention `Self`
    /// (`Parse.parse`): only the result type picks the impl, so it must be known once the group is
    /// inferred (E3307, spec 10.3).
    pub(super) fn require_known_self(
        &mut self,
        def: DefId,
        scheme: &crate::Scheme,
        tys: &[Ty],
        span: Span,
    ) {
        let program = self.program;
        if program.defs[def].kind != DefKind::TraitFn {
            return;
        }
        let (Some(self_ty), Some(param), Ty::Fn(f)) =
            (tys.first(), scheme.params.first(), &scheme.ty)
        else {
            return;
        };
        let Some(&trait_def) = param.traits.first() else {
            return;
        };
        if f.params.iter().any(|p| mentions_bound(p, 0)) {
            return;
        }
        self.self_obligations.push(SelfObligation {
            ty: self_ty.clone(),
            trait_def,
            span,
        });
    }

    /// Reports the `Self` of trait functions left undetermined (E3307, spec 10.3). No type is
    /// chosen by default.
    pub(super) fn check_known_selves(&mut self) {
        let program = self.program;
        for o in std::mem::take(&mut self.self_obligations) {
            if let Ty::Var(_) = self.ctx.resolve(&o.ty) {
                self.diags.push(Diagnostic {
                    span: Some(o.span),
                    kind: DiagnosticKind::AmbiguousSelf {
                        trait_name: program.defs[o.trait_def].name.clone(),
                    },
                });
            }
        }
    }

    /// Checks the trait constraints remembered at references (E3302, spec 10.3). A type
    /// parameter of the definition with the reference satisfies what its own constraints imply,
    /// supertraits included.
    pub(super) fn check_traits(&mut self) {
        let program = self.program;
        for o in std::mem::take(&mut self.trait_obligations) {
            let ty = self.ctx.resolve(&o.ty);
            if o.float_ok && ty == Ty::FLOAT {
                continue;
            }
            let env = o.owner.map(|d| self.def_params(d)).unwrap_or_default();
            if let Err(e) = self
                .traits
                .satisfies(program, self.ty_table, &ty, o.trait_def, &env)
            {
                // Unsolved sets (`fails ?e1`) of a function type say nothing here.
                let e = crate::traits::Unsatisfied {
                    ty: without_set_vars(&e.ty),
                    ..e
                };
                self.diags.push(Diagnostic {
                    span: Some(o.span),
                    kind: crate::traits::not_implemented(program, self.ty_table, &e, &env),
                });
            }
        }
    }
}

fn without_set_vars(ty: &Ty) -> Ty {
    let ty = ty.map_children(without_set_vars);
    match ty {
        Ty::Fn(mut f) => {
            let keep = |e: &RowElem| match e {
                RowElem::Var(_) => Row::empty(),
                e => Row::from_elems([e.clone()]),
            };
            f.fails = f.fails.flat_map(keep);
            f.uses = f.uses.flat_map(keep);
            Ty::Fn(f)
        }
        ty => ty,
    }
}

/// Whether `ty` mentions the type parameter `Ty::Bound(index)`.
fn mentions_bound(ty: &Ty, index: u32) -> bool {
    if let Ty::Bound(i) = ty {
        return *i == index;
    }
    let mut found = false;
    ty.for_each_child(|child| found |= mentions_bound(child, index));
    found
}
