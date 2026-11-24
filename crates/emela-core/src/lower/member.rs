//! Handler members (`[effect.decl]`, `[effect.handler]`, `[effect.with]`, `[dyn.with]`).
//!
//! In the IR, a handler member `M` of an effect `E` is an operation of `E`, after the declared
//! operations (`EffectDef::ops`). Calling it opens the inner handler that implements `M` in the
//! handler that provides `E`: it runs the inner handler's init and returns a pair of the handler
//! value opened and a function that releases it. Its parameters are those of the init, its
//! failure is the init's, and it is a suspend fn operation exactly when `M` is a
//! `suspend handler`. The backends need nothing new: opening is an operation call, and the
//! inner handler is an ordinary handler whose functions take the outer handler's evidence table
//! (its requirements are the outer handler's).
//!
//! - An item `x = E.M(a)` or `x = c.M(a)` of `with` calls the operation on the handler of `E`
//!   (or `c`), binds the pair, and binds x to the handler value opened, with the release
//!   function of the pair as its release. x is not put in the evidence table.
//! - Inside an init, `E.M(a)` and `c.M(a)` call the operation and register the release function
//!   with the handler being initialized. The init takes a cell holding a function that releases
//!   what it opened so far, newest first. The handler's release then runs that function after
//!   its own release; if the init fails, the init runs it before the failure propagates.
//! - A handler member opened inside an init through a handler that a `with` inside the init
//!   provides is registered instead in a cell of that `with`, which runs it when the `with` ends,
//!   before the release of its handlers.

use emela_resolve::hir::{DefId, ExprId, HandlerItem, Item, Res};
use emela_types::{Tag, Ty};

use super::{Lowerer, direct, direct_marked, is_stateful};
use crate::intrinsics::Builtin;
use crate::ir::{Arm, Callee, Expr, FnType, Lit, Local, OpDef, Pat, Type, TypeId, WithBind};

impl Lowerer<'_> {
    /// The index in `EffectDef::ops` of the handler member `member` of `effect` (a label's are
    /// its base's): after the operations.
    pub(super) fn member_index(&self, effect: DefId, member: DefId) -> Option<usize> {
        let pos = self
            .hir
            .effect_members(effect)
            .iter()
            .position(|&m| m == member)?;
        Some(self.hir.effect_ops(effect).len() + pos)
    }

    /// The operation that opens `member`, for the effect `tag` (its type arguments as seen from
    /// `ctx`, as in `EffectKey`).
    pub(super) fn member_def(&mut self, member: DefId, tag: &Tag, ctx: Option<DefId>) -> OpDef {
        let name = self.hir.defs[member].name.to_string();
        let suspend = member_suspends(self.hir, member);
        let release = self.release_fn_ty(suspend);
        let Some(Ty::Fn(f)) = self
            .types
            .sigs
            .get(member)
            .map(|s| emela_types::substitute_bound(&s.scheme.ty, &tag.args))
        else {
            return OpDef {
                suspend,
                ..OpDef::new(&name, Vec::new(), TypeId::UNIT)
            };
        };
        let outer = std::mem::replace(&mut self.current_def, ctx);
        let params = f.params.iter().map(|t| self.ty(t)).collect();
        let handler = self.ty(&f.ret);
        self.current_def = outer;
        let ret = self.module.tuple(vec![handler, release]);
        OpDef {
            name,
            suspend,
            type_params: 0,
            params,
            ret,
            fails: !f.fails.is_empty(),
        }
    }

    /// The type of a function that releases what a handler member opened: `fn() -> ()`, which may
    /// suspend for a `suspend handler`.
    pub(super) fn release_fn_ty(&mut self, suspends: bool) -> TypeId {
        self.module.func(FnType {
            params: Vec::new(),
            ret: TypeId::UNIT,
            fails: false,
            suspends,
            ev: false,
            sets: None,
        })
    }

    /// For a handler whose init opens handler members (`[effect.handler]`), whether any of them
    /// may suspend; `None` if its init opens none. Calls inside lambdas in the init count too.
    pub(super) fn init_opens(&self, def: DefId) -> Option<bool> {
        let Some(Item::Handler(h)) = self.hir.item(def) else {
            return None;
        };
        let init = h.init?;
        let mut found: Option<bool> = None;
        let mut stack = vec![init];
        while let Some(e) = stack.pop() {
            if let Some(&m) = self.types.members.get(e) {
                found = Some(found.unwrap_or(false) || member_suspends(self.hir, m));
            }
            stack.extend(self.hir.child_exprs(e));
        }
        found
    }

    /// The type of the cell an init keeps the release of what it opened in.
    pub(super) fn opened_cell_ty(&mut self, suspends: bool) -> TypeId {
        let f = self.release_fn_ty(suspends);
        self.module.intern(Type::Cell(f))
    }

    /// A new cell for an init that opens handler members, holding a function that does nothing.
    pub(super) fn new_opened_cell(&mut self, suspends: bool) -> Expr {
        let f = self.release_fn_ty(suspends);
        Expr::Builtin {
            op: Builtin::CellNew,
            ty_args: vec![f],
            args: vec![Expr::Lambda {
                params: Vec::new(),
                ty: f,
                body: Box::new(Expr::Lit(Lit::Unit)),
                suspends: false,
            }],
        }
    }

    /// Calls the function in the cell `cell`: releases what the init opened, newest first.
    pub(super) fn release_opened(&mut self, cell: Local, suspends: bool) -> Expr {
        let f = self.release_fn_ty(suspends);
        Expr::Call {
            callee: Callee::Indirect(Box::new(Expr::Builtin {
                op: Builtin::CellGet,
                ty_args: vec![f],
                args: vec![Expr::Var(cell)],
            })),
            args: Vec::new(),
            fails: false,
            suspends,
        }
    }

    /// `body`, then the release of what the init opened (`cell`), which runs even if `body` ends
    /// in a defect. `handler` names the handler, for a defect in a release.
    pub(super) fn then_release_opened(
        &mut self,
        body: Expr,
        cell: Local,
        suspends: bool,
        handler: &str,
    ) -> Expr {
        let guard = self.module.local("opened", TypeId::UNIT);
        let release = self.release_opened(cell, suspends);
        Expr::With {
            binds: vec![WithBind::cleanup(
                guard,
                Expr::Lit(Lit::Unit),
                handler,
                release,
            )],
            body: Box::new(body),
        }
    }

    /// The body of an init that opens handler members: if `body` does not end with a value, what
    /// it opened so far is released, newest first, before the failure or defect propagates
    /// (`[effect.handler]`).
    pub(super) fn guard_init(
        &mut self,
        body: Expr,
        ty: TypeId,
        cell: Local,
        suspends: bool,
        handler: &str,
    ) -> Expr {
        let bool_ty = TypeId::BOOL;
        let done_ty = self.module_cell(bool_ty);
        let done = self.module.local("done", done_ty);
        let guard = self.module.local("opened", TypeId::UNIT);
        let value = self.module.local("init", ty);
        let release = Expr::If {
            cond: Box::new(Expr::Builtin {
                op: Builtin::CellGet,
                ty_args: vec![bool_ty],
                args: vec![Expr::Var(done)],
            }),
            then: Box::new(Expr::Lit(Lit::Unit)),
            else_: Box::new(self.release_opened(cell, suspends)),
        };
        Expr::With {
            binds: vec![
                WithBind::new(
                    done,
                    Expr::Builtin {
                        op: Builtin::CellNew,
                        ty_args: vec![bool_ty],
                        args: vec![Expr::Lit(Lit::Bool(false))],
                    },
                ),
                WithBind::cleanup(guard, Expr::Lit(Lit::Unit), handler, release),
            ],
            body: Box::new(Expr::Let {
                var: value,
                value: Box::new(body),
                body: Box::new(Expr::Let {
                    var: self.module.local("_", TypeId::UNIT),
                    value: Box::new(Expr::Builtin {
                        op: Builtin::CellSet,
                        ty_args: vec![bool_ty],
                        args: vec![Expr::Var(done), Expr::Lit(Lit::Bool(true))],
                    }),
                    body: Box::new(Expr::Var(value)),
                }),
            }),
        }
    }

    fn module_cell(&mut self, content: TypeId) -> TypeId {
        self.module.intern(Type::Cell(content))
    }

    /// The implementation of the operation that opens `member` in the handler being lowered: a
    /// lambda that opens `inner` with its arguments (`[effect.handler]`). For a `suspend
    /// handler`, it takes resume last and resumes with the pair.
    pub(super) fn member_impl(&mut self, inner: DefId, op: &OpDef) -> Expr {
        let params: Vec<Local> = op
            .params
            .iter()
            .map(|&t| self.module.local("x", t))
            .collect();
        let args = params.iter().map(|&l| Expr::Var(l)).collect();
        let (opened, suspends) = self.open_inner(inner, args);
        let mut lambda_params = params;
        let mut param_tys: Vec<TypeId> = op.params.clone();
        let (body, ret) = if op.suspend {
            let resume_ty = self.module.func(FnType {
                params: vec![op.ret],
                ret: TypeId::UNIT,
                fails: false,
                suspends: false,
                ev: false,
                sets: None,
            });
            let resume = self.module.local("resume", resume_ty);
            lambda_params.push(resume);
            param_tys.push(resume_ty);
            let body = Expr::Call {
                callee: Callee::Indirect(Box::new(Expr::Var(resume))),
                args: vec![opened],
                fails: false,
                suspends: false,
            };
            (body, TypeId::UNIT)
        } else {
            (opened, op.ret)
        };
        let ty = self.module.func(FnType {
            params: param_tys,
            ret,
            fails: op.fails,
            suspends,
            ev: false,
            sets: None,
        });
        Expr::Lambda {
            params: lambda_params,
            ty,
            body: Box::new(body),
            suspends,
        }
    }

    /// Opens the inner handler `inner` of the handler being lowered with the arguments of its
    /// init: the pair of its handler value and the function that releases it, and whether opening
    /// may suspend. The inner handler's functions take the outer handler's evidence table.
    fn open_inner(&mut self, inner: DefId, args: Vec<Expr>) -> (Expr, bool) {
        let hir = self.hir;
        let Some(Item::Handler(item)) = hir.item(inner) else {
            unreachable!("an inner handler");
        };
        let suspend = match item.member {
            Some(Res::Def(m)) => member_suspends(hir, m),
            _ => false,
        };
        let takes_ev = self
            .types
            .handler_uses
            .get(inner)
            .is_some_and(|r| self.row_needs_ev(r));
        let ev: Vec<Expr> = takes_ev.then(|| self.ev_arg()).into_iter().collect();
        let f = self.fn_id(inner, 0);
        let release_ty = self.release_fn_ty(suspend);
        let unit_release = || Expr::Lambda {
            params: Vec::new(),
            ty: release_ty,
            body: Box::new(Expr::Lit(Lit::Unit)),
            suspends: false,
        };
        if !is_stateful(item) {
            let value = direct(f, ev, false);
            return (Expr::Tuple(vec![value, unit_release()]), false);
        }
        let (init, release) = self.handler_fns(inner);
        let state_ty = self.handler_state_ty(inner, item);
        let state = self.module.local("self", state_ty);
        let opens = self.init_opens(inner);
        let cell = opens.map(|s| {
            let ty = self.opened_cell_ty(s);
            (self.module.local("opened", ty), s)
        });
        let init_fails = self
            .types
            .handler_fails
            .get(inner)
            .is_some_and(|r| !r.is_empty());
        let mut init_args = args;
        init_args.extend(cell.map(|(c, _)| Expr::Var(c)));
        init_args.extend(ev.clone());
        let init_call = direct_marked(&self.module, init, init_args, init_fails);
        let init_suspends = self.module.functions[init].suspends;
        let with_state: Vec<Expr> = std::iter::once(Expr::Var(state)).chain(ev).collect();
        let handler_value = direct(f, with_state.clone(), false);
        // The value of release is discarded (`[effect.handler]`).
        let mut release_body = match release {
            Some(r) => {
                // As `handler_parts` defines it, from the type of the release block.
                let ret = match item.release {
                    Some((_, b)) => {
                        let ty = self.expr_ty(b);
                        self.ty(ty)
                    }
                    None => TypeId::UNIT,
                };
                Expr::Let {
                    var: self.module.local("_", ret),
                    value: Box::new(direct_marked(&self.module, r, with_state, false)),
                    body: Box::new(Expr::Lit(Lit::Unit)),
                }
            }
            None => Expr::Lit(Lit::Unit),
        };
        let mut release_suspends = release.is_some_and(|r| self.module.functions[r].suspends);
        if let Some((c, s)) = cell {
            let name = hir.defs[inner].name.to_string();
            release_body = self.then_release_opened(release_body, c, s, &name);
            release_suspends |= s;
        }
        let release_fn = Expr::Lambda {
            params: Vec::new(),
            ty: release_ty,
            body: Box::new(release_body),
            suspends: release_suspends,
        };
        let pair = Expr::Tuple(vec![handler_value, release_fn]);
        let mut body = Expr::Let {
            var: state,
            value: Box::new(init_call),
            body: Box::new(pair),
        };
        if let Some((c, s)) = cell {
            body = Expr::Let {
                var: c,
                value: Box::new(self.new_opened_cell(s)),
                body: Box::new(body),
            };
        }
        (body, init_suspends)
    }

    /// The handler value that a handler member item of `with` opens a member of
    /// (`[effect.with]`), the effect whose operation opens it (a label's base), and the index of
    /// that operation. `earlier` are the providers already bound in the same `with`.
    pub(super) fn member_provider(
        &mut self,
        member: DefId,
        provider: &emela_types::MemberProvider,
        earlier: &[(super::EffectKey, Local)],
    ) -> Option<(Expr, crate::ir::EffectId, usize)> {
        let (value, tag) = match provider {
            emela_types::MemberProvider::Effect(tag) => {
                let key = self.key_here(tag);
                let value = match earlier.iter().rev().find(|(k, _)| *k == key) {
                    Some((_, l)) => Expr::Var(*l),
                    None => self.lookup_effect(&key)?,
                };
                (value, tag.clone())
            }
            emela_types::MemberProvider::Cap(cap) => {
                let Some(Ty::Cap(tags, _)) = self.types.exprs.get(*cap).cloned() else {
                    return None;
                };
                let value = self.expr(*cap);
                self.cap_table(value, &tags, member)?
            }
        };
        let effect = self.effect_id(&self.key_here(&tag));
        let effect = self.module.handler_effect(effect);
        let base = self.types.sigs.base_of(&tag).def;
        let index = self.member_index(base, member)?;
        Some((value, effect, index))
    }

    /// Opening a handler member inside an init (`Files.Open(p)`, `fs.Open(p)`): `call` is the
    /// call of the operation that opens it. Registers the release of what it opens in the init's
    /// cell, newest first, and gives the handler value.
    pub(super) fn open_in_init(
        &mut self,
        call: Expr,
        callee: ExprId,
        effect: crate::ir::EffectId,
        index: usize,
    ) -> Expr {
        let op = self.module.effects[effect].ops[index].clone();
        // Opened through a handler that a `with` inside the init provides: released at the end
        // of that `with` (`[effect.handler]`).
        let in_with = self.types.member_open_withs.get(callee).and_then(|w| {
            self.with_cells
                .iter()
                .rev()
                .find(|(id, _)| id == w)
                .map(|(_, cell)| *cell)
        });
        let Some((cell, suspends)) = in_with.or(self.opened_cell) else {
            // The type checker allows it only inside an init (E0448).
            return call;
        };
        let pair = self.module.local("opened", op.ret);
        let (handler_ty, release_ty) = match self.module.ty(op.ret) {
            Type::Tuple(elems) => (elems[0], elems[1]),
            _ => unreachable!("a member's operation returns a pair"),
        };
        let cell_fn = self.release_fn_ty(suspends);
        let previous = self.module.local("previous", cell_fn);
        let release = Expr::Call {
            callee: Callee::Indirect(Box::new(tuple_get(
                pair,
                1,
                2,
                release_ty,
                &mut self.module,
            ))),
            args: Vec::new(),
            fails: false,
            suspends: op.suspend,
        };
        let guard = self.module.local("opened", TypeId::UNIT);
        let chained = Expr::Lambda {
            params: Vec::new(),
            ty: cell_fn,
            body: Box::new(Expr::With {
                binds: vec![WithBind::cleanup(
                    guard,
                    Expr::Lit(Lit::Unit),
                    &op.name,
                    Expr::Call {
                        callee: Callee::Indirect(Box::new(Expr::Var(previous))),
                        args: Vec::new(),
                        fails: false,
                        suspends,
                    },
                )],
                body: Box::new(release),
            }),
            suspends,
        };
        Expr::Let {
            var: pair,
            value: Box::new(call),
            body: Box::new(Expr::Let {
                var: previous,
                value: Box::new(Expr::Builtin {
                    op: Builtin::CellGet,
                    ty_args: vec![cell_fn],
                    args: vec![Expr::Var(cell)],
                }),
                body: Box::new(Expr::Let {
                    var: self.module.local("_", TypeId::UNIT),
                    value: Box::new(Expr::Builtin {
                        op: Builtin::CellSet,
                        ty_args: vec![cell_fn],
                        args: vec![Expr::Var(cell), chained],
                    }),
                    body: Box::new(tuple_get(pair, 0, 2, handler_ty, &mut self.module)),
                }),
            }),
        }
    }

    /// The handler member that the callee `callee` of a call opens inside an init, if any.
    pub(super) fn member_callee(&self, callee: ExprId) -> Option<DefId> {
        self.types.members.get(callee).copied()
    }
}

/// Whether `member` is a `suspend handler`.
pub(super) fn member_suspends(hir: &emela_resolve::hir::Program, member: DefId) -> bool {
    matches!(hir.item(member), Some(Item::Member(m)) if m.sig.is_suspend)
}

/// Whether an inner handler implements a handler member without `suspend`: then its init and
/// release do not suspend (E0417).
pub(super) fn implements_plain_member(hir: &emela_resolve::hir::Program, h: &HandlerItem) -> bool {
    matches!(h.member, Some(Res::Def(m)) if !member_suspends(hir, m))
}

/// The element `index` of the tuple of `n` elements in `var`.
pub(super) fn tuple_get(
    var: Local,
    index: usize,
    n: usize,
    ty: TypeId,
    module: &mut crate::ir::Module,
) -> Expr {
    tuple_project(Expr::Var(var), index, n, ty, module)
}

/// The `index`-th of the `n` elements of the tuple `value` evaluates to, of type `ty`.
pub(super) fn tuple_project(
    value: Expr,
    index: usize,
    n: usize,
    ty: TypeId,
    module: &mut crate::ir::Module,
) -> Expr {
    let x = module.local("x", ty);
    let pats = (0..n)
        .map(|i| if i == index { Pat::Bind(x) } else { Pat::Wild })
        .collect();
    Expr::Match {
        scrutinee: Box::new(value),
        arms: vec![Arm {
            pat: Pat::Tuple(pats),
            guard: None,
            body: Expr::Var(x),
        }],
    }
}
