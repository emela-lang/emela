//! Field access, capability operations and handler members, and updates of constructors.

use emela_resolve::hir::{Arg, DefId, ExprId, ExprKind, Item, Label, LocalKind, Res, Span};
use smol_str::SmolStr;

use super::{BodyInfer, FieldTy, cap_effects_name};
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::infer::{Origin, SetKind, substitute, substitute_bound};
use crate::row::{CapElem, CapId, Row, RowElem, Tag};
use crate::ty::Ty;

impl BodyInfer<'_> {
    /// The type of a capability operation `log.info` (spec 8.2): the declared operation type with
    /// the effect added to R. Effect type parameters are replaced with the capability value's
    /// type arguments (`get` of `use State[Int]` is `fn() -> Int`, spec 8.1).
    ///
    /// When `base` is a capability value captured by the closure or local fn being inferred, the
    /// R is the capability itself (`uses { primary }`, `[effect.use]`); it widens to the effect.
    /// An operation on a capability of `tags`. Only the operations of those effects can be
    /// called (`[effect.use]`); calling one adds its own effect (`[type.rules]`).
    pub(super) fn infer_op(
        &mut self,
        expr: ExprId,
        base: ExprId,
        tags: &[Tag],
        name: &Label,
    ) -> Ty {
        let program = self.program;
        // `fs.Open(p)` opens a handler member of the handler that provides `fs`
        // (`[effect.use]`).
        if name.name.starts_with(|c: char| c.is_ascii_uppercase()) {
            return self.infer_cap_member(expr, base, tags, name);
        }
        // A label has the operations of its base (issue #255).
        let found = tags.iter().find_map(|t| {
            program
                .effect_ops(t.def)
                .iter()
                .copied()
                .find(|&op| program.defs[op].name == name.name)
                .map(|op| (t, op))
        });
        let Some((tag, op)) = found else {
            return self.field_error(
                name,
                DiagnosticKind::UnknownOperation {
                    effect: cap_effects_name(program, tags),
                    op: name.name.clone(),
                },
            );
        };
        self.ops.insert(expr, op);
        // The effect's type parameters come first in the operation's scheme, followed by the
        // operation's own (`start[A]`, spec 8.1), which are instantiated per reference. Through a
        // label, they are the base's type arguments (`effect Hits of State[Int]`).
        let args = self.sigs.base_of(tag).args.clone();
        let mut receiver = None;
        let ty = self.sigs.get(op).map(|sig| {
            let n = args.len().min(sig.scheme.params.len());
            let (fresh, fresh_rows) = self.ctx.fresh_args(&sig.scheme.params[n..]);
            let mut tys = args[..n].to_vec();
            let mut rows: Vec<Row> = tys
                .iter()
                .enumerate()
                .map(|(i, a)| match a {
                    Ty::Set(_, row) => row.clone(),
                    _ => Row::from_elems([RowElem::Bound(i as u32)]),
                })
                .collect();
            if !fresh.is_empty() {
                self.type_args.insert(expr, fresh.clone());
            }
            // The set of the receiver (`[effect.handler]`, `op_signature`): the capability the
            // operation is called on.
            if sig
                .scheme
                .params
                .last()
                .is_some_and(|p| p.name == "C(self)")
                && let Some(row) = fresh_rows.last()
            {
                receiver = Some(row.clone());
            }
            tys.extend(fresh);
            rows.extend(fresh_rows);
            substitute(&sig.scheme.ty, &tys, &rows)
        });
        if let Some(row) = receiver
            && let Some(Ty::Cap(_, caps)) =
                self.exprs.get(base).map(|t| self.ctx.shallow_resolve(t))
        {
            self.ctx
                .add_subset_of(SetKind::Caps, caps, row, Origin::Lambda);
        }
        // A capability held in a field of the handler outlives the handler: it belongs to a
        // `with` around the handler's, or to a handler member its init opened (`[effect.use]`,
        // `[effect.handler]`). It is not a requirement of the handler: its element is the
        // handler's `self`, which the end of the body removes. It still counts as its effect for
        // whether the body may suspend (E4303). In release, a suspend fn called that way makes
        // Async a requirement, so that leaving the `with` shows that it may suspend. The field
        // may have been bound to a local variable first (`p = self.pool`): its set is still the
        // field's (#866).
        let uses = match (self.field_capability(base), self.current) {
            (Some(field), Some(handler)) => {
                let in_release = match program.item(handler) {
                    Some(Item::Handler(h)) => h.release.is_some_and(|(_, body)| {
                        let (body, at) = (program.exprs[body].span, program.exprs[expr].span);
                        body.module == at.module && body.range.contains_range(at.range)
                    }),
                    _ => false,
                };
                let mut row = Row::from_elems([RowElem::Cap(CapElem {
                    id: CapId::SelfOf(handler),
                    name: SmolStr::new(field),
                    tag: tag.clone(),
                })]);
                let suspend = matches!(program.item(op), Some(Item::Op(o)) if o.sig.is_suspend);
                if in_release
                    && suspend
                    && let Some(async_) = program.prelude_def("Async")
                {
                    row.insert(RowElem::Tag(Tag::new(async_)));
                }
                row
            }
            // The capabilities the receiver may be (`[effect.use]`): each counts as the effect
            // of the operation for whether the call may suspend. The name of an effect is the
            // effect.
            _ => self.receiver_uses(base, tag),
        };
        match ty {
            // The capabilities passed to the operation's capability parameters are in R too. A
            // set of a field of the handler among them (`self.pool.read(self.id)`) is removed at
            // the end of the body, as the field's capability is (`end_self`, #866).
            Some(Ty::Fn(f)) => {
                let mut uses = uses;
                uses.extend(&f.uses);
                Ty::func_sets(f.params, f.ret, f.fails, uses)
            }
            _ => Ty::Error,
        }
    }

    /// The name of the effect that declares the handler member `member`.
    pub(super) fn member_effect_name(&self, member: DefId) -> SmolStr {
        let program = self.program;
        program.defs[member]
            .parent
            .map(|e| program.defs[e].name.clone())
            .unwrap_or_default()
    }

    /// Whether `expr` is a capability held in a field of the handler, and its name: a field of
    /// the implicit `self` (`self.inner`), or a local variable bound to one (`p` after
    /// `p = self.inner`). The type of such a local variable has a set variable whose solution is
    /// not known yet, so the binding is followed instead.
    pub(super) fn field_capability(&self, expr: ExprId) -> Option<SmolStr> {
        let program = self.program;
        match &program.exprs[expr].kind {
            ExprKind::Field { base, name } => match program.exprs[*base].kind {
                ExprKind::Path(Res::Local(l)) if program.locals[l].kind == LocalKind::SelfParam => {
                    Some(SmolStr::new(format!("self.{}", name.name)))
                }
                _ => None,
            },
            ExprKind::Path(Res::Local(l)) => self
                .let_values
                .get(l)
                .and_then(|&value| self.field_capability(value)),
            _ => None,
        }
    }

    /// The sets of the fields of `handler` as they are in its operation bodies and release: the
    /// hidden set type parameters of the handler's type, `Bound(i)` after the written type
    /// parameters (`[effect.handler]`). They are decided at each item of a with that runs the
    /// init. The written set type parameters of a built-in handler (`Background[E, R]`) are not.
    pub(super) fn field_sets(&self, handler: DefId) -> Vec<RowElem> {
        let Some(caps) = self.ty_table.decl_caps(handler) else {
            return Vec::new();
        };
        let n = self.ty_table.self_ty(handler).map_or(0, |ty| match ty {
            Ty::Named(_, args) => args.len(),
            _ => 0,
        });
        (caps.visible..n)
            .map(|i| RowElem::Bound(i as u32))
            .collect()
    }

    /// Opening a handler member is written only as an item of `with` and, as a call, inside an
    /// init (E4106, `[effect.use]`, `[effect.handler]`). Records the opening.
    pub(super) fn check_member_open(&mut self, expr: ExprId, member: DefId, span: Span) {
        self.member_opens.insert(expr, member);
        if self.in_init.is_none() || !self.member_callees.contains(&expr) {
            self.diags.push(Diagnostic {
                span: Some(span),
                kind: DiagnosticKind::MemberOutsideInit {
                    member: self.program.defs[member].name.clone(),
                    effect: self.member_effect_name(member),
                },
            });
        }
    }

    /// Records the innermost `with` inside the init that provides the handler `expr` opens
    /// `member` through: the capability `base`, or the name of its effect.
    pub(super) fn record_open_with(&mut self, expr: ExprId, member: DefId, base: Option<ExprId>) {
        if self.init_withs.is_empty() {
            return;
        }
        let caps: Vec<CapElem> = match base {
            // A local's own type: a reference to it has a fresh set that contains it.
            Some(base) => match (&self.program.exprs[base].kind, self.exprs.get(base)) {
                (ExprKind::Path(Res::Local(l)), _) if self.locals.get(*l).is_some() => {
                    match self.locals.get(*l).map(|t| self.ctx.shallow_resolve(t)) {
                        Some(Ty::Cap(_, caps)) => {
                            self.ctx.resolve_row(&caps).caps().cloned().collect()
                        }
                        _ => Vec::new(),
                    }
                }
                (_, Some(t)) => match self.ctx.shallow_resolve(t) {
                    Ty::Cap(_, caps) => self.ctx.resolve_row(&caps).caps().cloned().collect(),
                    _ => Vec::new(),
                },
                _ => Vec::new(),
            },
            None => match self.program.defs[member].parent {
                Some(effect) => match self.anchor_name(&Tag::new(effect)) {
                    RowElem::Cap(c) => vec![c],
                    _ => Vec::new(),
                },
                None => Vec::new(),
            },
        };
        let with = self.init_withs.iter().rev().copied().find(|w| {
            caps.iter().any(|c| match c.id {
                CapId::Item { with, .. } => with == *w,
                CapId::Named { provider, .. } => provider == *w,
                _ => false,
            })
        });
        if let Some(with) = with {
            self.member_open_withs.insert(expr, with);
        }
    }

    /// The type `ty` of opening `member` at `expr`, with the capability opened recording the
    /// `with` inside the init that provides its provider, if any ([`Self::record_open_with`]):
    /// it cannot leave that `with` (`[effect.handler]`).
    pub(super) fn opened_in_init(&mut self, expr: ExprId, member: DefId, ty: Ty) -> Ty {
        let Some(&with) = self.member_open_withs.get(expr) else {
            return ty;
        };
        let Ty::Fn(mut f) = ty else {
            return ty;
        };
        if let Ty::Cap(tags, _) = &f.ret
            && let Some(tag) = tags.first().cloned()
        {
            let name = self.program.defs[member].name.clone();
            f.ret = Ty::Cap(
                tags.clone(),
                Row::from_elems([RowElem::Cap(CapElem {
                    id: CapId::Item {
                        with,
                        item: crate::row::OPENED_IN_INIT,
                    },
                    name,
                    tag,
                })]),
            );
        }
        Ty::Fn(f)
    }

    /// The type of opening the handler member `name` through the capability `base` of `tag`
    /// (`fs.Open`, `[type.rules]`): the member's type with the effect's type parameters replaced
    /// by the capability's type arguments, and the capability itself as its R, as for an
    /// operation ([`Self::infer_op`]).
    pub(super) fn infer_cap_member(
        &mut self,
        expr: ExprId,
        base: ExprId,
        tags: &[Tag],
        name: &Label,
    ) -> Ty {
        let program = self.program;
        let found = tags
            .iter()
            .find_map(|t| program.effect_member(t.def, &name.name).map(|m| (t, m)));
        let Some((tag, member)) = found else {
            return self.field_error(
                name,
                DiagnosticKind::UnknownOperation {
                    effect: cap_effects_name(program, tags),
                    op: name.name.clone(),
                },
            );
        };
        self.check_member_open(expr, member, name.span);
        self.record_open_with(expr, member, Some(base));
        let ty = self.cap_member_ty(base, tag, member);
        self.opened_in_init(expr, member, ty)
    }

    /// The type of opening `member` through the capability `cap` of `tag`.
    pub(super) fn cap_member_ty(&mut self, cap: ExprId, tag: &Tag, member: DefId) -> Ty {
        let args = self.sigs.base_of(tag).args.clone();
        let Some(Ty::Fn(f)) = self
            .sigs
            .get(member)
            .map(|sig| substitute_bound(&sig.scheme.ty, &args))
        else {
            return Ty::Error;
        };
        let uses = self.receiver_uses(cap, tag);
        Ty::func_sets(f.params, f.ret, f.fails, uses)
    }

    /// What calling an operation of `tag` on the capability `base` adds to R: the capabilities
    /// it may be (`[effect.use]`). Each counts as `tag` for whether the call may suspend. The
    /// name of an effect is the effect itself in R.
    pub(super) fn receiver_uses(&mut self, base: ExprId, tag: &Tag) -> Row {
        match self.exprs.get(base).map(|t| self.ctx.shallow_resolve(t)) {
            Some(Ty::Cap(_, caps)) => Row::from_elems(caps.elems().iter().map(|e| match e {
                RowElem::Cap(c) if matches!(c.id, CapId::Named { .. }) => {
                    RowElem::Tag(c.tag.clone())
                }
                RowElem::Cap(c) => RowElem::Cap(CapElem {
                    tag: tag.clone(),
                    ..c.clone()
                }),
                e => e.clone(),
            })),
            _ => Row::of_tag(tag.clone()),
        }
    }

    /// The type of field `field` of type definition `type_def` applied to `args`. Fields come from
    /// the constructor's declared type (labels and parameter types).
    pub(super) fn field_ty(&mut self, type_def: DefId, args: &[Ty], field: &SmolStr) -> FieldTy {
        let program = self.program;
        let ctor = match program.item(type_def) {
            Some(Item::Type(item)) => match item.ctor {
                Some(ctor) => ctor,
                None => return FieldTy::NotARecord,
            },
            // An error is its own constructor.
            Some(Item::Error(_)) => type_def,
            // A handler value's fields (`self.conn`, spec 8.4).
            Some(Item::Handler(item)) => match item.ctor {
                Some(ctor) => ctor,
                None => return FieldTy::NoSuchField,
            },
            _ => return FieldTy::NotARecord,
        };
        let Some(sig) = self.sigs.get(ctor) else {
            return FieldTy::NotARecord;
        };
        let Ty::Fn(f) = &sig.scheme.ty else {
            // A type or error without fields.
            return FieldTy::NoSuchField;
        };
        match sig.labels.iter().position(|l| l.as_ref() == Some(field)) {
            Some(i) => FieldTy::Found(i, substitute_bound(&f.params[i], args)),
            None => FieldTy::NoSuchField,
        }
    }

    /// Pushes a field diagnostic at the field name and returns `Ty::Error`.
    pub(super) fn field_error(&mut self, name: &Label, kind: DiagnosticKind) -> Ty {
        self.diags.push(Diagnostic {
            span: Some(name.span),
            kind,
        });
        Ty::Error
    }

    /// The type of an update `C(f: e, ..base)` (spec 5.3).
    ///
    /// The constructor's type is instantiated twice: once for the value being built and once for
    /// `base`, and the type arguments may differ. Written fields are checked against the first;
    /// unwritten fields are carried over from `base`, so their types in the two instances are
    /// unified.
    ///
    /// In the reordered arguments, only written fields are `Some`; unwritten ones are `None`
    /// (carried over from `base`).
    pub(super) fn infer_update(
        &mut self,
        expr: ExprId,
        ctor: ExprId,
        fields: &[Arg],
        base: ExprId,
    ) -> Ty {
        let program = self.program;
        let sigs = self.sigs;
        let ctor_ty = self.infer_expr(ctor);
        let ExprKind::Path(Res::Def(ctor_def)) = program.exprs[ctor].kind else {
            // Name resolution has reported non-constructors.
            for field in fields {
                self.infer_expr(field.value);
            }
            self.infer_expr(base);
            return Ty::Error;
        };
        let Ty::Fn(made) = self.ctx.shallow_resolve(&ctor_ty) else {
            // A constructor without fields (or without a declared type).
            self.check_expr(base, &ctor_ty);
            for field in fields {
                self.infer_expr(field.value);
            }
            return ctor_ty;
        };
        let Some(sig) = sigs.get(ctor_def) else {
            return Ty::Error;
        };
        let base_ty = self.ctx.instantiate(&sig.scheme);
        let Ty::Fn(from) = base_ty else {
            return Ty::Error;
        };
        self.check_expr(base, &from.ret);

        let mut written = vec![false; made.params.len()];
        let mut slots: Vec<Option<ExprId>> = vec![None; made.params.len()];
        for field in fields {
            let Some(label) = &field.label else {
                // Name resolution has reported fields written by position.
                self.infer_expr(field.value);
                continue;
            };
            let slot = sig
                .labels
                .iter()
                .position(|l| l.as_ref() == Some(&label.name));
            match slot {
                Some(i) if written[i] => {
                    self.diags.push(Diagnostic {
                        span: Some(label.span),
                        kind: DiagnosticKind::DuplicateArg {
                            label: label.name.clone(),
                        },
                    });
                    self.infer_expr(field.value);
                }
                Some(i) => {
                    written[i] = true;
                    slots[i] = Some(field.value);
                    self.check_expr(field.value, &made.params[i]);
                }
                None => {
                    let ty = self.ctx.resolve(&made.ret);
                    self.diags.push(Diagnostic {
                        span: Some(label.span),
                        kind: DiagnosticKind::NoField {
                            ty,
                            field: label.name.clone(),
                        },
                    });
                    self.infer_expr(field.value);
                }
            }
        }

        // Unwritten fields are carried over from base.
        let base_span = program.exprs[base].span;
        for (i, was_written) in written.iter().enumerate() {
            if !was_written {
                self.coerce_at(base_span, &made.params[i], &from.params[i]);
            }
        }
        self.calls.insert(expr, slots);

        made.ret
    }
}
