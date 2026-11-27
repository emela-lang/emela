//! The type arguments of handlers with type parameters at the items of a `with`, of a layer and on
//! the right of `by` (`[effect.with]`, `[effect.layer]`, `[effect.label]`).
//!
//! Each item instantiates the handler's type parameters with fresh type variables, once, at the
//! item. They are unified with the type arguments written after the handler, with the arguments
//! of init, with the underlying effect of the label left of `by`, and, by the ordinary rules,
//! with how a name bound to the item is used. The R of the body and the requirements of the
//! other items of the same `with` unify with the effects the item provides when they are
//! removed (`BodyInfer::subtract`), as for `State.run`.
//!
//! Where inference cannot decide, the type arguments are written (E4107):
//!
//! - an item whose type arguments are neither written nor determined by the arguments of init,
//!   and one of whose effects unifies with an effect another item of the same `with` provides,
//!   may override it or be overridden by it ([`BodyInfer::check_item_overrides`]);
//! - two or more effects of the body and of the requirements of the other items fit an effect
//!   the item provides ([`BodyInfer::check_several_fits`]);
//! - a type variable left when the enclosing top-level declaration is inferred that is not
//!   generalized with it ([`BodyInfer::check_items_determined`]). A layer infers them only from
//!   the arguments of init.

use emela_resolve::hir::{DefId, DefKind, ExprId, Item, Program, Res, Span, TypeId, WithItem};
use rustc_hash::FxHashSet;
use smol_str::SmolStr;

use super::BodyInfer;
use crate::decl::layer::{Entry, Insts, Supplier, handler_tags};
use crate::diagnostic::{Diagnostic, DiagnosticKind, HandlerArgsReason};
use crate::ty::{Row, RowElem, Tag, Ty, TyVar};

/// The type arguments an item gives a handler with type parameters.
#[derive(Clone, Debug, Default)]
pub(crate) struct ItemInst {
    /// Type variables of the group, unified with what determines them. Empty for an item that
    /// does not name a handler with type parameters.
    pub(crate) args: Vec<Ty>,
    /// The handler, for E4107.
    pub(crate) handler: Option<DefId>,
    pub(crate) span: Option<Span>,
    /// Whether the type arguments are written or determined by the arguments of init. Only such
    /// an item takes part in overriding another item of the same `with` (`[effect.with]`).
    pub(crate) determined: bool,
    /// The definition whose body has the item.
    pub(crate) owner: Option<DefId>,
    /// An item of a layer, which infers its type arguments only from the arguments of init.
    pub(crate) in_layer: bool,
    /// E4107 has been reported for it.
    pub(crate) reported: bool,
}

impl BodyInfer<'_> {
    /// Instantiates the handler named by `target` at an item, and unifies its type arguments with
    /// those written (`written`). Type arguments written for anything other than a handler with
    /// type parameters, or a wrong number of them, are E3101; the bounds of the type parameters
    /// are checked once types are settled (E3302).
    pub(super) fn instantiate_item(
        &mut self,
        target: Res,
        written: &[TypeId],
        span: Span,
        in_layer: bool,
    ) -> ItemInst {
        let program = self.program;
        let Res::Def(def) = target else {
            return ItemInst::default();
        };
        let params = match program.item(def) {
            Some(Item::Handler(_)) => self.ty_table.handler_params(def).to_vec(),
            _ => Vec::new(),
        };
        if params.is_empty() && written.is_empty() {
            return ItemInst::default();
        }
        if written.len() != params.len() && !written.is_empty() {
            self.diags.push(Diagnostic {
                span: Some(span),
                kind: DiagnosticKind::WrongTypeArgCount {
                    name: program.defs[def].name.clone(),
                    expected: params.len(),
                    found: written.len(),
                },
            });
            for &t in written {
                self.lower_type(t);
            }
            if params.is_empty() {
                return ItemInst::default();
            }
        }
        // A set type parameter of a built-in handler (`Group[E, R]`, `[task.group]`) is a set
        // variable, solved from what the body starts.
        let args: Vec<Ty> = (0..params.len())
            .map(|i| match self.ty_table.set_kind(def, i) {
                Some(kind) => {
                    // A capability that a task cannot use is reported at the item.
                    let v = self.ctx.new_row_var_at(span);
                    Ty::Set(kind, Row::var(v))
                }
                None => self.ctx.new_var(),
            })
            .collect();
        let determined = written.len() == params.len();
        if determined {
            for (arg, &t) in args.iter().zip(written) {
                let ty = self.lower_type(t);
                let at = program.types[t].span.unwrap_or(span);
                self.coerce_at(at, arg, &ty);
            }
        }
        // Checked after the effects of the group are inferred: the R of the body may be what
        // determines the type arguments.
        let before = self.trait_obligations.len();
        self.require_traits(&params, &args, span);
        let item_obligations = self.trait_obligations.split_off(before);
        self.item_obligations.extend(item_obligations);
        ItemInst {
            args,
            handler: Some(def),
            span: Some(span),
            determined,
            owner: self.current,
            in_layer,
            reported: false,
        }
    }

    /// The type arguments of the items of the `with` `expr`, with those of the members of the
    /// layers it lists ([`ItemsInsts`]).
    pub(super) fn item_insts(&self, expr: ExprId, items: &[WithItem]) -> Insts {
        self.items_insts().of(expr, items)
    }

    pub(super) fn items_insts(&self) -> ItemsInsts<'_> {
        ItemsInsts {
            program: self.program,
            items: &self.item_insts,
            earlier: self.handler_facts.layer_ty_args,
            here: &self.layer_insts,
        }
    }

    /// The E (`SetKind::Errors`) or the R (`SetKind::Effects`) that the built-in handler
    /// `handler` adds to its `with`, at an item with the type arguments `ty_args`: the sets of
    /// its set type parameters of that kind, and Async for the R (`[task.group]`,
    /// `[task.background]`). The errors of the tasks of a handler whose init takes parameters
    /// (`Background(on_failure:)`) go to them, not to the `with`. `None` for other handlers.
    pub(super) fn builtin_handler_sets(
        &self,
        handler: DefId,
        ty_args: &[Ty],
        kind: crate::ty::SetKind,
    ) -> Option<Row> {
        let program = self.program;
        let Some(Item::Handler(h)) = program.item(handler) else {
            return None;
        };
        if !h.is_intrinsic {
            return None;
        }
        let errors_to_init = !h.init_params.is_empty();
        let mut row = Row::empty();
        let mut any = false;
        for (i, arg) in ty_args.iter().enumerate() {
            let Some(k) = self.ty_table.set_kind(handler, i) else {
                continue;
            };
            any = true;
            if k == kind
                && !(kind == crate::ty::SetKind::Errors && errors_to_init)
                && let Ty::Set(_, r) = arg
            {
                row.extend(r);
            }
        }
        if !any {
            return None;
        }
        if kind == crate::ty::SetKind::Effects
            && let Some(async_) = program.prelude_def("Async")
        {
            row.insert(RowElem::Tag(Tag::new(async_)));
        }
        Some(row)
    }

    /// Replaces the type arguments of `inst` with what they resolve to once they contain no
    /// type variable ([`ItemsInsts`]).
    pub(super) fn freeze(&mut self, inst: &mut ItemInst) {
        if !inst.args.is_empty() && self.inst_known(inst) {
            inst.args = inst.args.iter().map(|t| self.ctx.resolve(t)).collect();
        }
    }

    /// requires(H) (`uses`) or the E of init (`fails`) of `handler`, instantiated with the type
    /// arguments of an item (`[type.rules]`). For a handler of an earlier group the set is known
    /// and its type parameters are replaced. For one of this group it is a set variable not
    /// solved yet: a fresh set variable stands for it, which contains its solution with the type
    /// arguments placed once solved, as for a reference to a definition of the same group
    /// (`InstanceSet`).
    pub(super) fn instantiate_facts(
        &mut self,
        facts: Row,
        ty_args: &[Ty],
        kind: crate::ty::SetKind,
    ) -> Row {
        if ty_args.is_empty() {
            return facts;
        }
        if facts.vars().next().is_none() {
            return crate::decl::layer::instantiate_row(&facts, ty_args);
        }
        let v = self.ctx.new_row_var();
        self.instance_sets.push(super::InstanceSet {
            inferred: facts,
            instance: v,
            kind,
            args: ty_args.to_vec(),
            rows: Vec::new(),
        });
        Row::var(v)
    }

    /// Unifies the effect of the handler with type parameters on the right of `by` that serves
    /// the label `label` with the underlying effect of the label (`[effect.label]`). A label has
    /// no type parameters, so its underlying effect fits in one way only.
    pub(super) fn unify_with_label(&mut self, label: Res, inst: &ItemInst) {
        let (Res::Def(label), Some(handler)) = (label, inst.handler) else {
            return;
        };
        let Some(base) = self.sigs.label_base(label).cloned() else {
            return;
        };
        let tags = handler_tags(self.sigs, self.ty_table, handler, &inst.args);
        if let Some(tag) = tags.iter().find(|t| t.def == base.def) {
            self.ctx.same_tag(tag, &base);
        }
    }

    /// Whether the type arguments of `inst` contain no type variable.
    pub(super) fn inst_known(&mut self, inst: &ItemInst) -> bool {
        inst.args.iter().all(|t| {
            let t = self.ctx.resolve(t);
            self.ctx.free_vars(&t).is_empty()
        })
    }

    /// E4107 for an unbound item whose type arguments are neither written nor determined
    /// by the arguments of init, when one of its effects unifies with an effect another unbound
    /// item of the same `with` provides (`[effect.with]`): whether one overrides the
    /// other would depend on what inference picks. The other items include the handlers of a
    /// layer.
    pub(super) fn check_item_overrides(&mut self, expr: ExprId, entries: &[Entry]) {
        let Some(insts) = self.item_insts.get(expr).cloned() else {
            return;
        };
        for (i, inst) in insts.iter().enumerate() {
            if inst.args.is_empty() || inst.determined || bound_item(entries, i) {
                continue;
            }
            let own: Vec<Tag> = entries
                .iter()
                .filter(|e| e.origin == i && matches!(e.supplier, Supplier::Handler(_)))
                .map(|e| e.effect.clone())
                .collect();
            let other = entries
                .iter()
                .filter(|e| e.origin != i && e.named)
                .find(|e| own.iter().any(|t| self.ctx.could_be_same_tag(t, &e.effect)));
            if let Some(other) = other {
                let other = other.clone();
                let effect = self.show_tag(&other.effect);
                let name = match &other.supplier {
                    Supplier::Handler(h) => self.program.defs[*h].name.clone(),
                    Supplier::Effect(t) => self.program.defs[t.def].name.clone(),
                    Supplier::Member { member, .. } => self.program.defs[*member].name.clone(),
                };
                self.report_item(
                    expr,
                    i,
                    HandlerArgsReason::Overrides {
                        effect,
                        other: name,
                        at: other.span,
                    },
                );
            }
        }
    }

    /// E4107 for an unbound item none of whose type arguments the arguments of init
    /// determine, when two or more effects of `candidates` (the R of the body and the
    /// requirements of the other items) fit an effect it provides (`[effect.with]`). The item
    /// does not pick one.
    pub(super) fn check_several_fits(
        &mut self,
        expr: ExprId,
        entries: &[Entry],
        body: &Row,
        deps: &[Row],
    ) {
        let Some(insts) = self.item_insts.get(expr).cloned() else {
            return;
        };
        for (i, inst) in insts.iter().enumerate() {
            if inst.args.is_empty() || inst.determined || inst.reported || bound_item(entries, i) {
                continue;
            }
            let mut candidates: Vec<Tag> = Vec::new();
            let mut add = |row: &Row, ctx: &mut crate::infer::InferCtx| {
                for e in ctx.resolve_row(row).elems() {
                    if let RowElem::Tag(t) = e
                        && !candidates.contains(t)
                    {
                        candidates.push(t.clone());
                    }
                }
            };
            add(body, &mut self.ctx);
            for (entry, d) in entries.iter().zip(deps) {
                if entry.origin != i {
                    add(d, &mut self.ctx);
                }
            }
            for entry in entries.iter().filter(|e| e.origin == i) {
                let tag = entry.effect.clone();
                let fits: Vec<Tag> = candidates
                    .iter()
                    .filter(|c| self.ctx.could_be_same_tag(&tag, c))
                    .cloned()
                    .collect();
                if let [first, second, ..] = fits.as_slice() {
                    let (first, second) = (self.show_tag(first), self.show_tag(second));
                    self.report_item(expr, i, HandlerArgsReason::Several { first, second });
                    break;
                }
            }
        }
    }

    /// E4107 for the type variables left in the type arguments of items once the group is
    /// inferred and its definitions generalized (`[effect.with]`): one that appears in the type
    /// of the enclosing declaration is generalized with it (`generalized`), the others are not
    /// determined. A layer has nothing to generalize into.
    pub(super) fn check_items_determined(&mut self, generalized: &[(DefId, Vec<(TyVar, u32)>)]) {
        let mut sites: Vec<(Option<ExprId>, usize, ItemInst)> = Vec::new();
        for (expr, insts) in self.item_insts.iter() {
            for (i, inst) in insts.iter().enumerate() {
                sites.push((Some(expr), i, inst.clone()));
            }
        }
        for (_, insts) in &self.layer_insts {
            for (i, inst) in insts.iter().enumerate() {
                sites.push((None, i, inst.clone()));
            }
        }
        for (expr, i, inst) in sites {
            if inst.args.is_empty() || inst.reported {
                continue;
            }
            let kept: FxHashSet<TyVar> = generalized
                .iter()
                .filter(|(d, _)| Some(*d) == inst.owner && !inst.in_layer)
                .flat_map(|(_, vs)| vs.iter().map(|(v, _)| *v))
                .collect();
            let left = inst.args.iter().any(|t| {
                let t = self.ctx.resolve(t);
                self.ctx.free_vars(&t).iter().any(|v| !kept.contains(v))
            });
            if !left {
                continue;
            }
            let reason = if inst.in_layer {
                HandlerArgsReason::Layer
            } else {
                HandlerArgsReason::Unknown
            };
            match expr {
                Some(expr) => self.report_item(expr, i, reason),
                None => self.push_unknown(&inst, reason),
            }
        }
    }

    /// Reports E4107 for the `i`-th item of the `with` `expr`, once.
    fn report_item(&mut self, expr: ExprId, i: usize, reason: HandlerArgsReason) {
        let Some(inst) = self.item_insts.get_mut(expr).and_then(|v| v.get_mut(i)) else {
            return;
        };
        if inst.reported {
            return;
        }
        inst.reported = true;
        let inst = inst.clone();
        self.push_unknown(&inst, reason);
    }

    fn push_unknown(&mut self, inst: &ItemInst, reason: HandlerArgsReason) {
        let Some(handler) = inst.handler else {
            return;
        };
        self.diags.push(Diagnostic {
            span: inst.span,
            kind: DiagnosticKind::HandlerArgsUnknown {
                handler: handler_with_params(self.program, self.ty_table, handler),
                reason,
            },
        });
    }

    /// `State[Int]`, for diagnostics.
    fn show_tag(&mut self, tag: &Tag) -> SmolStr {
        let row = self.ctx.resolve_row(&Row::of_tag(tag.clone()));
        row.display_effects(&self.ty_table.cons, &[])
            .to_string()
            .into()
    }
}

/// The type arguments of the items of the `with`s and layers of a group, for
/// [`crate::decl::layer::with_entries`].
///
/// They are read without following unification: those that the written type arguments, the
/// arguments of init or the label left of `by` determine are already the types they resolve to
/// ([`BodyInfer::freeze`]), so that whether an item overrides another compares them as types.
/// Those left as type variables differ from every other type, and an item with them does not
/// override another (E4107 when it could, [`BodyInfer::check_item_overrides`]); the effects
/// they give unify with others where effects are removed, as for `State.run`.
#[derive(Clone, Copy)]
pub(crate) struct ItemsInsts<'a> {
    program: &'a Program,
    items: &'a crate::sparse::SparseMap<ExprId, Vec<ItemInst>>,
    earlier: &'a la_arena::ArenaMap<DefId, Vec<Vec<Ty>>>,
    here: &'a [(DefId, Vec<ItemInst>)],
}

impl ItemsInsts<'_> {
    pub(crate) fn of(&self, expr: ExprId, items: &[WithItem]) -> Insts {
        let args: Vec<Vec<Ty>> = self
            .items
            .get(expr)
            .map(|insts| insts.iter().map(|i| i.args.clone()).collect())
            .unwrap_or_default();
        Insts::new(self.program, items, args, |layer| {
            if let Some(args) = self.earlier.get(layer) {
                return Some(args.clone());
            }
            let (_, insts) = self.here.iter().find(|(l, _)| *l == layer)?;
            Some(insts.iter().map(|i| i.args.clone()).collect())
        })
    }
}

/// `Memory[S]`: a handler with its type parameters, for diagnostics.
pub(crate) fn handler_with_params(
    program: &Program,
    ty_table: &crate::decl::lower::TyTable,
    handler: DefId,
) -> SmolStr {
    let name = &program.defs[handler].name;
    let params = ty_table.handler_params(handler);
    if params.is_empty() || program.defs[handler].kind != DefKind::Handler {
        return name.clone();
    }
    let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
    SmolStr::new(format!("{name}[{}]", names.join(", ")))
}

/// Whether the `origin`-th item of a `with` is bound to a name: its entries provide nothing by
/// name ([`Entry::named`]). Such an item neither overrides nor is overridden, and its type
/// arguments are not inferred from the R of the body (`[effect.with]`), so the checks of E4107
/// for unbound items skip it.
fn bound_item(entries: &[Entry], origin: usize) -> bool {
    entries.iter().any(|e| e.origin == origin && !e.named)
}
