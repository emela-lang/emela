//! Handler bodies: init, release and the operation bodies, and the handlers that a handler's init
//! provides to its members (`[effect.handler]`).

use emela_resolve::hir::{DefId, HandlerItem, Item, Param, Res};
use smol_str::SmolStr;

use super::{BodyInfer, HandlerBody, HandlerPart};
use crate::decl::lower::is_intrinsic_effect;
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::infer::SetKind;
use crate::ty::{CapElem, CapId, Row, RowElem, Tag, Ty, substitute, substitute_bound};

impl BodyInfer<'_> {
    /// Infers a handler's init, release and operation bodies (spec 8.4).
    ///
    /// - init's value is the handler value. If the handler has fields, init is required (E4006).
    /// - `self` in release and operation bodies is the handler value. release's value is dropped.
    /// - Operation parameter and return types come from the effect operation's declaration. A
    ///   suspend fn implementation takes `resume` as its last parameter: a function taking a value
    ///   of the operation's return type and returning `()`. The body's value is `()`.
    /// - Handlers for a host effect cannot be written (`[effect.host]`).
    pub(super) fn infer_handler(&mut self, def: DefId, item: &HandlerItem) {
        let program = self.program;
        // The runtime implements a built-in handler (`Deferred`, `[kw.annotation]`): it has no
        // init, release or operations to check.
        if item.is_intrinsic {
            return;
        }
        let span = program.defs[def].span;
        // The effects listed after `:` (`[effect.handler]`); one that did not resolve leaves the
        // handler unchecked, as name resolution has reported it.
        let mut effects: Vec<DefId> = Vec::with_capacity(item.effects.len());
        for e in &item.effects {
            let Res::Def(effect) = e.res else {
                return;
            };
            if program.is_provided_by_host(effect) {
                self.diags.push(Diagnostic {
                    span,
                    kind: DiagnosticKind::HostEffectHandler {
                        handler: program.defs[def].name.clone(),
                        effect: program.defs[effect].name.clone(),
                    },
                });
                return;
            }
            // Only a built-in handler, or the scope function, provides a built-in effect
            // (`Task.Group`, `[task.group]`; `Deferred`, `[task.deferred]`).
            if is_intrinsic_effect(program, effect) {
                let name = program.defs[effect].name.clone();
                self.diags.push(Diagnostic {
                    span,
                    kind: DiagnosticKind::IntrinsicHandler {
                        handler: program.defs[def].name.clone(),
                        provider: self
                            .builtins
                            .provider(effect)
                            .cloned()
                            .unwrap_or(name.clone()),
                        effect: name,
                    },
                });
                return;
            }
            // A handler of a label is E4003 (signatures); its operations are not looked up.
            if crate::decl::signature::is_label(program, effect) {
                return;
            }
            if !effects.contains(&effect) {
                effects.push(effect);
            }
        }
        if effects.is_empty() {
            return;
        }
        let handler_ty = self.ty_table.self_ty(def).unwrap_or(Ty::Error);
        match item.init {
            Some(init) => {
                let params = self.sigs.init_params(def).map(|p| p.tys.clone());
                for (i, p) in item.init_params.iter().enumerate() {
                    let ty = params
                        .as_ref()
                        .and_then(|tys| tys.get(i).cloned())
                        .unwrap_or(Ty::Error);
                    self.set_local(p.local, ty);
                }
                self.in_init = Some(def);
                // The sets of the fields are decided by what the init gives them; the fields
                // given cannot record a capability of a with inside the init (E4405).
                let made = self.fresh_hidden_caps(def, &handler_ty);
                self.check_expr(init, &made);
                self.in_init = None;
                self.handler_parts.push(HandlerPart {
                    handler: def,
                    body: init,
                    release: false,
                });
            }
            None if !item.fields.is_empty() => self.diags.push(Diagnostic {
                span,
                kind: DiagnosticKind::MissingInit {
                    handler: program.defs[def].name.clone(),
                },
            }),
            None => {}
        }
        if let Some((self_local, body)) = item.release {
            self.set_local(self_local, handler_ty.clone());
            self.infer_expr(body);
            self.handler_parts.push(HandlerPart {
                handler: def,
                body,
                release: true,
            });
        }
        // The operations of every listed effect, in the order of the effects
        // (`[effect.handler]`). A missing handler member is E4001 too, listed after the
        // operations.
        let inner_member = |inner: DefId| match program.item(inner) {
            Some(Item::Handler(h)) => h.member,
            _ => None,
        };
        let declared = |members: bool| -> Vec<DefId> {
            effects
                .iter()
                .flat_map(|&effect| match program.item(effect) {
                    Some(Item::Effect(e)) if members => e.members.as_slice(),
                    Some(Item::Effect(e)) => e.ops.as_slice(),
                    _ => &[],
                })
                .copied()
                .collect()
        };
        let (ops, members) = (declared(false), declared(true));
        // An operation whose name another listed effect has too is E4012
        // (`check_listed_effect`), not missing.
        let missing: Vec<SmolStr> = ops
            .iter()
            .filter(|&&op| !item.ops.iter().any(|h| h.op == Res::Def(op)))
            .map(|&op| program.defs[op].name.clone())
            .filter(|name| {
                ops.iter()
                    .filter(|&&o| program.defs[o].name == *name)
                    .count()
                    == 1
            })
            .collect();
        let missing_members: Vec<SmolStr> = members
            .iter()
            .filter(|&&m| {
                !item
                    .inner
                    .iter()
                    .any(|&i| inner_member(i) == Some(Res::Def(m)))
            })
            .map(|&m| program.defs[m].name.clone())
            .collect();
        for (k, &inner) in item.inner.iter().enumerate() {
            if let Some(Res::Def(m)) = inner_member(inner)
                && item.inner[..k]
                    .iter()
                    .any(|&i| inner_member(i) == Some(Res::Def(m)))
            {
                self.diags.push(Diagnostic {
                    span: program.defs[inner].span,
                    kind: DiagnosticKind::DuplicateHandlerOp {
                        op: program.defs[m].name.clone(),
                    },
                });
            }
        }
        if let Some(Res::Def(member)) = item.member {
            self.check_inner_handler(def, member, item);
        }
        if !missing.is_empty() || !missing_members.is_empty() {
            self.diags.push(Diagnostic {
                span,
                kind: DiagnosticKind::MissingHandlerOps {
                    handler: program.defs[def].name.clone(),
                    ops: missing,
                    members: missing_members,
                },
            });
        }

        for (index, hop) in item.ops.iter().enumerate() {
            self.set_local(hop.self_local, handler_ty.clone());
            let op = match hop.op {
                Res::Def(op) if item.ops[..index].iter().any(|h| h.op == hop.op) => {
                    self.diags.push(Diagnostic {
                        span: Some(hop.name.span),
                        kind: DiagnosticKind::DuplicateHandlerOp {
                            op: program.defs[op].name.clone(),
                        },
                    });
                    None
                }
                Res::Def(op) => Some(op),
                // Name resolution has reported this.
                _ => None,
            };
            // If the implementation's `suspend` differs from the effect's declaration, E4004. The
            // parameters and body are checked as the implementation is written, so errors do not
            // cascade.
            let declared_suspend =
                op.map(|op| matches!(program.item(op), Some(Item::Op(o)) if o.sig.is_suspend));
            if let Some(declared) = declared_suspend
                && declared != hop.is_suspend
                && let Some(effect) = op.and_then(|op| program.defs[op].parent)
            {
                self.diags.push(Diagnostic {
                    span: Some(hop.name.span),
                    kind: DiagnosticKind::SuspendMismatch {
                        op: hop.name.name.clone(),
                        effect: program.defs[effect].name.clone(),
                        suspend: declared,
                    },
                });
            }
            let suspend = hop.is_suspend;
            // Effect type parameters are replaced with the types written in the handler head
            // for the effect that declares the operation (spec 8.1).
            let effect_args = op
                .and_then(|op| self.sigs.handler_effect_of(program, def, op))
                .map(|t| t.args.clone())
                .unwrap_or_default();
            // A capability-bound type of the effect in the result records the handler's
            // `self` (`[effect.handler]`).
            let mut rigid = Vec::new();
            let f = match op.and_then(|op| self.sigs.get(op)).map(|s| {
                let ty = substitute_bound(&s.scheme.ty, &effect_args);
                // A capability parameter is an unknown set in the implementation
                // (`[effect.handler]`), which ends with the body.
                let ty = match hop.body {
                    Some(body) => {
                        let rows: Vec<Row> = s
                            .scheme
                            .params
                            .iter()
                            .enumerate()
                            .map(|(i, p)| match &p.caps {
                                Some(tags) if p.name != "C(self)" => {
                                    let cap = CapElem {
                                        id: CapId::Rigid {
                                            binder: body,
                                            index: i as u32,
                                        },
                                        name: p
                                            .name
                                            .trim_start_matches("C(")
                                            .trim_end_matches(')')
                                            .into(),
                                        tag: tags.first().cloned().unwrap_or_else(|| Tag::new(def)),
                                    };
                                    rigid.push(cap.clone());
                                    Row::from_elems([RowElem::Cap(cap)])
                                }
                                _ => Row::from_elems([RowElem::Bound(i as u32)]),
                            })
                            .collect();
                        substitute(&ty, &[], &rows)
                    }
                    None => ty,
                };
                match s.scheme.params.last() {
                    Some(p) if p.name == "C(self)" => {
                        let recv = s.scheme.params.len() - 1;
                        let mut rows: Vec<Row> = (0..recv)
                            .map(|i| Row::from_elems([RowElem::Bound(i as u32)]))
                            .collect();
                        rows.push(Row::from_elems([RowElem::Cap(CapElem {
                            id: CapId::SelfOf(def),
                            name: SmolStr::new("self"),
                            tag: p
                                .caps
                                .as_ref()
                                .and_then(|t| t.first().cloned())
                                .unwrap_or_else(|| Tag::new(def)),
                        })]));
                        substitute(&ty, &[], &rows)
                    }
                    _ => ty,
                }
            }) {
                Some(Ty::Fn(f)) => Some(*f),
                _ => None,
            };
            let Some(f) = f else {
                // Operations whose type cannot be determined still have their expressions
                // inferred and types recorded.
                for p in &hop.params {
                    self.set_local(p.local, Ty::Error);
                }
                if let Some(body) = hop.body {
                    self.infer_expr(body);
                }
                continue;
            };
            // A suspend fn implementation's parameters are the operation's plus `resume`.
            let mut params = f.params.clone();
            if suspend {
                params.push(Ty::func(vec![f.ret.clone()], Ty::UNIT));
            }
            if hop.params.len() != params.len() {
                self.diags.push(Diagnostic {
                    span: Some(hop.name.span),
                    kind: DiagnosticKind::HandlerOpArgCount {
                        op: hop.name.name.clone(),
                        expected: params.len(),
                        found: hop.params.len(),
                    },
                });
            }
            for (i, p) in hop.params.iter().enumerate() {
                let ty = params.get(i).cloned().unwrap_or(Ty::Error);
                self.set_local(p.local, ty);
            }
            let Some(body) = hop.body else {
                continue;
            };
            // A suspend fn implementation's value is dropped; the operation's value is what is
            // passed to `resume`.
            let ret = if suspend { Ty::UNIT } else { f.ret.clone() };
            if !rigid.is_empty() {
                self.quantified_lambdas.insert(body, rigid);
            }
            self.ctx.enter_scope(body);
            self.check_expr(body, &ret);
            self.ctx.exit_scope();
            let resume_later = suspend
                && hop.params.len() == params.len()
                && hop
                    .params
                    .last()
                    .is_some_and(|resume| self.resume_escapes(body, resume.local));
            self.handler_bodies.push(HandlerBody {
                handler: def,
                index,
                body,
                fails: f.fails.clone(),
                resume_later,
                suspend,
                resume: (suspend && hop.params.len() == params.len())
                    .then(|| hop.params.last().map(|r| r.local))
                    .flatten(),
            });
        }
    }

    /// The type of opening `member` as the handler `handler` implements it: the member's type
    /// with the effect's type parameters replaced with the type arguments of the effect of
    /// `handler`'s outer handler (`[effect.handler]`).
    pub(super) fn inner_member_ty(&self, handler: DefId, member: DefId) -> Option<crate::ty::FnTy> {
        let outer = self.program.defs[handler].parent?;
        let args = self
            .sigs
            .handler_effect_of(self.program, outer, member)
            .map(|t| t.args.clone())
            .unwrap_or_default();
        match self
            .sigs
            .get(member)
            .map(|s| substitute_bound(&s.scheme.ty, &args))
        {
            Some(Ty::Fn(f)) => Some(*f),
            _ => None,
        }
    }

    /// An inner handler implements the handler member of its name (`[effect.handler]`): it
    /// implements the effect the member declares, and its init takes the member's parameters with
    /// the same names and types (E4011).
    pub(super) fn check_inner_handler(&mut self, def: DefId, member: DefId, item: &HandlerItem) {
        let program = self.program;
        let Some(f) = self.inner_member_ty(def, member) else {
            return;
        };
        let Some(Item::Member(m)) = program.item(member) else {
            return;
        };
        let cons = &self.ty_table.cons;
        let effect = program.defs[member]
            .parent
            .map(|e| program.defs[e].name.clone())
            .unwrap_or_default();
        let mismatch = |reason| DiagnosticKind::InnerHandlerMismatch {
            handler: program.defs[def].name.clone(),
            member: program.defs[member].name.clone(),
            effect: effect.clone(),
            reason: Box::new(reason),
        };
        // The inner handler implements the same effects as the member, in any order.
        let own = self.sigs.handler_effects(def);
        if let Ty::Cap(expected, _) = &f.ret
            && !own.is_empty()
            && !matches!(Ty::cap(own.iter().cloned(), Row::empty()), Ty::Cap(own, _) if own == *expected)
        {
            let show = |ts: &[Tag]| {
                let parts: Vec<String> = ts
                    .iter()
                    .map(|t| {
                        Row::of_tag(t.clone())
                            .display_effects(cons, &[])
                            .to_string()
                    })
                    .collect();
                SmolStr::new(parts.join(" + "))
            };
            self.diags.push(Diagnostic {
                span: program.defs[def].span,
                kind: mismatch(crate::diagnostic::InnerHandlerMismatch::Effect {
                    expected: show(expected),
                    found: show(own),
                }),
            });
        }
        let names = |ps: &[Param]| -> Vec<SmolStr> {
            ps.iter()
                .map(|p| program.locals[p.local].name.clone())
                .collect()
        };
        let show_params = |names: &[SmolStr], tys: &[Ty]| -> SmolStr {
            let parts: Vec<String> = names
                .iter()
                .zip(tys)
                .map(|(n, t)| format!("{n}: {}", t.display(cons)))
                .collect();
            SmolStr::new(format!("({})", parts.join(", ")))
        };
        let expected_names = names(&m.sig.params);
        let expected = show_params(&expected_names, &f.params);
        let found = match item.init {
            Some(_) => {
                let tys = self
                    .sigs
                    .init_params(def)
                    .map(|p| p.tys.clone())
                    .unwrap_or_default();
                let found_names = names(&item.init_params);
                let same = found_names == expected_names && tys == f.params;
                (!same).then(|| Some(show_params(&found_names, &tys)))
            }
            // Without an init, the handler takes no arguments.
            None => (!f.params.is_empty()).then_some(None),
        };
        if let Some(found) = found {
            self.diags.push(Diagnostic {
                span: program.defs[def].span,
                kind: mismatch(crate::diagnostic::InnerHandlerMismatch::Params {
                    params: expected,
                    found,
                }),
            });
        }
    }

    /// A handler's dependencies (the union of the R of init, release and the operation bodies):
    /// computed in an earlier group, or a set variable within this group. Empty for handlers not
    /// inferred (Async handlers).
    pub(super) fn handler_deps(&self, handler: DefId) -> Row {
        if let Some(row) = self.handler_facts.uses.get(handler) {
            return row.clone();
        }
        match self.pending_handlers.iter().find(|(h, _, _)| *h == handler) {
            Some(&(_, v, _)) => Row::var(v),
            None => Row::empty(),
        }
    }

    /// `ty`, the type `def` names, with its hidden capability sets replaced by fresh set variables.
    pub(super) fn fresh_hidden_caps(&mut self, def: DefId, ty: &Ty) -> Ty {
        let (Ty::Named(con, args), Some(caps)) = (ty, self.ty_table.decl_caps(def)) else {
            return ty.clone();
        };
        let visible = caps.visible;
        let args = args
            .iter()
            .enumerate()
            .map(|(i, a)| {
                if i >= visible {
                    Ty::Set(SetKind::Caps, Row::var(self.ctx.new_row_var()))
                } else {
                    a.clone()
                }
            })
            .collect::<Vec<_>>();
        Ty::named(*con, args)
    }
}
