//! The items of a `with` and of a layer: handlers, handler members and bound items, and the
//! arguments of their init (`[effect.with]`, `[effect.layer]`).

use emela_resolve::hir::{DefId, DefKind, ExprId, Item, Res, Span, SupplyArgs, WithItem};
use smol_str::SmolStr;

use super::{BodyInfer, cap_effects_name, items, through_label};
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::infer::{Origin, SetKind};
use crate::ty::{CapElem, CapId, Row, RowElem, Tag, Ty, substitute, substitute_bound};

impl BodyInfer<'_> {
    /// The arguments of init written on the items of a `with` (`[effect.with]`). Every item's
    /// arguments are checked, including those of a handler a later item overrides.
    ///
    /// An item that opens a handler member (`x = Files.Open(p)`, `x = fs.Open(p)`) takes the
    /// arguments of the member's init, and binds x to the capability of the effect the member
    /// declares (`[effect.with]`). Another bound item binds x to the capability of the effect it
    /// provides.
    ///
    /// The handler with type parameters that supplies an item is instantiated first
    /// (`body/items.rs`), so that the effects the item provides, the type of a name bound to it
    /// and the parameters of its init have the same type variables.
    pub(super) fn infer_with_args(&mut self, expr: ExprId, items: &[WithItem]) {
        let program = self.program;
        let insts: Vec<items::ItemInst> = items
            .iter()
            .map(|item| {
                if crate::decl::layer::is_member_item(program, item) {
                    return items::ItemInst::default();
                }
                // A label left of `by` takes no type arguments (E3101).
                if item.by.is_some() && !item.ty_args.is_empty() {
                    self.instantiate_item(item.res, &item.ty_args, item.span, false);
                }
                let (target, _) = item.supplier();
                let span = item.by.map_or(item.span, |(_, s)| s);
                let written = match (item.by, target) {
                    // The type arguments of an effect right of `by` are read by `by_tag`.
                    (Some(_), Res::Def(d)) if program.defs[d].kind == DefKind::Effect => &[][..],
                    _ => item.supplier_ty_args(),
                };
                let mut inst = self.instantiate_item(target, written, span, false);
                if item.by.is_some() {
                    self.unify_with_label(item.res, &inst);
                }
                self.freeze(&mut inst);
                inst
            })
            .collect();
        self.item_insts.insert(expr, insts.clone());
        for (i, inst) in insts.iter().enumerate() {
            for arg in &inst.args {
                if let Ty::Set(_, row) = arg
                    && let [RowElem::Var(v)] = row.elems()
                {
                    self.item_set_vars.insert(*v, (expr, i as u32));
                }
            }
        }
        let insts_now = self.item_insts(expr, items);
        let entries =
            crate::decl::layer::with_entries(program, self.sigs, self.ty_table, items, &insts_now);
        let mut opened = Vec::new();
        let args = items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                if let Some((f, member)) = self.infer_member_item(item) {
                    let ret = f.ret.clone();
                    let args = match &item.args {
                        Some(args) => {
                            let labels = self.sigs.get(member).map(|s| s.labels.clone());
                            self.check_args(
                                Some(&program.defs[member].name),
                                labels.as_deref(),
                                &f.params,
                                &args.args,
                                args.span,
                            )
                        }
                        // A member whose init takes parameters needs them, as a handler does
                        // (E4104, `[effect.with]`).
                        None => {
                            if !f.params.is_empty() {
                                let labels = self.sigs.get(member).map(|s| s.labels.clone());
                                self.diags.push(Diagnostic {
                                    span: Some(item.span),
                                    kind: DiagnosticKind::MissingMemberArgs {
                                        member: program.defs[member].name.clone(),
                                        effect: item
                                            .label
                                            .map(|l| program.defs[l].name.clone())
                                            .unwrap_or_else(|| self.member_effect_name(member)),
                                        params: labels
                                            .unwrap_or_default()
                                            .into_iter()
                                            .flatten()
                                            .collect(),
                                    },
                                });
                            }
                            Vec::new()
                        }
                    };
                    // The capability of the item itself (`[effect.with]`).
                    if let Some(local) = item.bind {
                        let ty = match self.ctx.resolve(&ret) {
                            Ty::Cap(tags, _) if !tags.is_empty() => {
                                let cap = self.item_cap(
                                    expr,
                                    i,
                                    program.locals[local].name.clone(),
                                    tags[0].clone(),
                                );
                                Ty::Cap(tags, Row::from_elems([cap]))
                            }
                            ty => ty,
                        };
                        self.set_local(local, ty);
                        self.member_bound.insert(local);
                    }
                    opened.push((i, f));
                    return args;
                }
                if let Res::Def(def) = item.res
                    && item.bind.is_some()
                    && matches!(program.item(def), Some(Item::Layer(_)))
                {
                    self.diags.push(Diagnostic {
                        span: Some(item.span),
                        kind: DiagnosticKind::BoundLayer {
                            layer: program.defs[def].name.clone(),
                        },
                    });
                }
                // The capability of every effect the item provides: all of them for a handler
                // of several effects (`[effect.with]`).
                if let Some(local) = item.bind {
                    let tags: Vec<Tag> = entries
                        .iter()
                        .filter(|e| e.origin == i && !e.from_layer)
                        .map(|e| e.effect.clone())
                        .collect();
                    let ty = if tags.is_empty() {
                        Ty::Error
                    } else {
                        let cap = self.item_cap(
                            expr,
                            i,
                            program.locals[local].name.clone(),
                            tags[0].clone(),
                        );
                        Ty::cap(tags, Row::from_elems([cap]))
                    };
                    self.set_local(local, ty);
                }
                let ty_args = &insts[i].args;
                match item.by {
                    // The label left of `by` takes no arguments.
                    Some((by, by_span)) => {
                        self.infer_supply_args(item.res, item.args.as_ref(), item.span, &[]);
                        self.infer_supply_args(by, item.by_init_args.as_ref(), by_span, ty_args)
                    }
                    None => {
                        self.infer_supply_args(item.res, item.args.as_ref(), item.span, ty_args)
                    }
                }
            })
            .collect();
        self.with_args.insert(expr, args);
        // The arguments of init may have determined the type arguments; only then does the item
        // take part in overriding (`[effect.with]`).
        for (i, inst) in insts.iter().enumerate() {
            if !inst.args.is_empty() && !inst.determined && self.inst_known(inst) {
                let mut inst = inst.clone();
                inst.determined = true;
                self.freeze(&mut inst);
                if let Some(v) = self.item_insts.get_mut(expr) {
                    v[i] = inst;
                }
            }
        }
        let insts_now = self.item_insts(expr, items);
        let entries =
            crate::decl::layer::with_entries(program, self.sigs, self.ty_table, items, &insts_now);
        self.check_item_overrides(expr, &entries);
        if !opened.is_empty() {
            self.with_members.insert(expr, opened);
        }
    }

    /// The handler member an item of `with` opens (`Files.Open(p)` or `fs.Open(p)`).
    pub(super) fn member_item(&self, item: &WithItem) -> Option<DefId> {
        match (item.res, &item.cap) {
            (Res::Def(def), _) if self.program.defs[def].kind == DefKind::Member => Some(def),
            (_, Some((cap, _))) => self.member_opens.get(*cap).copied(),
            _ => None,
        }
    }

    /// For an item of `with` that opens a handler member, the type of opening it
    /// (`[type.rules]`): the parameters of the init, the capability opened, the member's E, and
    /// the R that names the effect (`Files`) or the capability (`fs`). An item without `x =` is
    /// E4105.
    pub(super) fn infer_member_item(
        &mut self,
        item: &WithItem,
    ) -> Option<(crate::ty::FnTy, DefId)> {
        let program = self.program;
        let (ty, member) = match (item.res, &item.cap) {
            (Res::Def(member), _) if program.defs[member].kind == DefKind::Member => {
                let ty = self.instantiate_def(member, item.span).0;
                let ty = match item.label {
                    Some(label) => through_label(ty, label),
                    None => ty,
                };
                (ty, member)
            }
            (_, Some((cap, name))) => {
                let cap_ty = self.infer_expr(*cap);
                let tags = match self.ctx.shallow_resolve(&cap_ty) {
                    Ty::Cap(tags, _) => tags,
                    Ty::Error => return None,
                    ty => {
                        let ty = self.ctx.resolve(&ty);
                        self.diags.push(Diagnostic {
                            span: Some(name.span),
                            kind: DiagnosticKind::NoFieldOrFunction {
                                ty,
                                name: name.name.clone(),
                            },
                        });
                        return None;
                    }
                };
                let found = tags.iter().find_map(|t| {
                    program
                        .effect_member(t.def, &name.name)
                        .map(|m| (t.clone(), m))
                });
                let Some((tag, member)) = found else {
                    self.diags.push(Diagnostic {
                        span: Some(name.span),
                        kind: DiagnosticKind::UnknownOperation {
                            effect: cap_effects_name(program, &tags),
                            op: name.name.clone(),
                        },
                    });
                    return None;
                };
                self.member_opens.insert(*cap, member);
                (self.cap_member_ty(*cap, &tag, member), member)
            }
            _ => return None,
        };
        if item.bind.is_none() {
            self.diags.push(Diagnostic {
                span: Some(item.span),
                kind: DiagnosticKind::UnboundMemberItem {
                    member: program.defs[member].name.clone(),
                    effect: self.member_effect_name(member),
                },
            });
        }
        match ty {
            Ty::Fn(f) => Some((*f, member)),
            _ => None,
        }
    }

    /// Checks the arguments written after `target` (`WorkerKv(binding: "KV")`) against the
    /// parameters of its init, as the arguments of a call (`[effect.handler]`, spec 6.2), and
    /// returns them reordered by parameter. A handler whose init takes parameters needs them
    /// (E4104); anything else takes none (E4104). `span` is where `target` is written.
    ///
    /// For a handler with type parameters, the types of the parameters are instantiated with
    /// `ty_args`, the type arguments of the item (`[effect.with]`).
    pub(super) fn infer_supply_args(
        &mut self,
        target: Res,
        args: Option<&SupplyArgs>,
        span: Span,
        ty_args: &[Ty],
    ) -> Vec<Option<ExprId>> {
        let program = self.program;
        let sigs = self.sigs;
        let handler = match target {
            Res::Def(def) if matches!(program.item(def), Some(Item::Handler(_))) => Some(def),
            _ => None,
        };
        let instantiated = handler.and_then(|h| sigs.init_params(h)).map(|p| {
            let mut p = p.clone();
            if !ty_args.is_empty() {
                p.tys = p.tys.iter().map(|t| substitute_bound(t, ty_args)).collect();
            }
            p
        });
        let params = handler.zip(instantiated.as_ref());
        match (params, args) {
            (None, None) => Vec::new(),
            (Some((h, params)), None) => {
                self.diags.push(Diagnostic {
                    span: Some(span),
                    kind: DiagnosticKind::MissingInitArgs {
                        handler: program.defs[h].name.clone(),
                        params: params.labels.iter().flatten().cloned().collect(),
                    },
                });
                Vec::new()
            }
            (None, Some(args)) => {
                if let Res::Def(def) = target {
                    let kind = match program.defs[def].kind {
                        DefKind::Effect if crate::decl::signature::is_label(program, def) => {
                            "label"
                        }
                        kind => kind.describe(),
                    };
                    self.diags.push(Diagnostic {
                        span: Some(args.span),
                        kind: DiagnosticKind::UnexpectedInitArgs {
                            name: program.defs[def].name.clone(),
                            kind,
                        },
                    });
                }
                for arg in &args.args {
                    self.infer_expr(arg.value);
                }
                Vec::new()
            }
            (Some((h, params)), Some(args)) => {
                // The item decides the capability sets of the parameters.
                let rows: Vec<Row> = (0..params.caps)
                    .map(|_| Row::var(self.ctx.new_row_var()))
                    .collect();
                let tys: Vec<Ty> = params
                    .tys
                    .iter()
                    .map(|t| substitute(t, &[], &rows))
                    .collect();
                self.check_args(
                    Some(&program.defs[h].name),
                    Some(&params.labels),
                    &tys,
                    &args.args,
                    args.span,
                )
            }
        }
    }

    /// A handler value's constructor can only be used inside that handler's init (E4005, spec
    /// 8.4), since constructing it elsewhere would skip init and release.
    pub(super) fn check_handler_ctor(&mut self, def: DefId, span: Span) {
        let program = self.program;
        let Some(Item::Ctor(ctor)) = program.item(def) else {
            return;
        };
        if program.defs[ctor.owner].kind != DefKind::Handler || self.in_init == Some(ctor.owner) {
            return;
        }
        self.diags.push(Diagnostic {
            span: Some(span),
            kind: DiagnosticKind::HandlerCtorOutsideInit {
                handler: program.defs[ctor.owner].name.clone(),
            },
        });
    }

    /// A value of a capability-bound type of E can be constructed only inside a handler of E
    /// (E4407, `[type.rules]`). Returns the capability the value records: the handler's `self`.
    pub(super) fn check_bound_ctor(&mut self, def: DefId, span: Span) -> Option<CapElem> {
        let program = self.program;
        let owner = match program.item(def) {
            Some(Item::Ctor(ctor)) => ctor.owner,
            _ => return None,
        };
        let effect = self.ty_table.bound_effect(owner)?;
        let handler = self.current.filter(|_| self.in_handler());
        let implements = handler.and_then(|h| {
            self.sigs
                .handler_effects(h)
                .iter()
                .find(|t| t.def == effect)
                .map(|t| (h, t.clone()))
        });
        match implements {
            Some((h, tag)) => Some(CapElem {
                id: CapId::SelfOf(h),
                name: SmolStr::new("self"),
                tag,
            }),
            None => {
                self.diags.push(Diagnostic {
                    span: Some(span),
                    kind: DiagnosticKind::BoundTypeOutsideHandler {
                        ty: program.defs[owner].name.clone(),
                        effect: program.defs[effect].name.clone(),
                    },
                });
                None
            }
        }
    }

    /// Puts `cap` in the set of the capability-bound type that the constructor type `ty` makes.
    pub(super) fn record_maker(&mut self, ty: &Ty, cap: CapElem) {
        let made = match ty {
            Ty::Fn(f) => &f.ret,
            ty => ty,
        };
        let Ty::Named(con, args) = made else {
            return;
        };
        let def = self.ty_table.def(*con);
        let Some(i) = self.ty_table.decl_caps(def).map(|c| c.visible) else {
            return;
        };
        if let Some(Ty::Set(SetKind::Caps, row)) = args.get(i) {
            self.ctx.add_subset_of(
                SetKind::Caps,
                Row::from_elems([RowElem::Cap(cap)]),
                row.clone(),
                Origin::Lambda,
            );
        }
    }
}
