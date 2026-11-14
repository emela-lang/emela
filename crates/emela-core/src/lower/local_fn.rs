//! Local fns (`[fn.local]`), lowered by lambda lifting.
//!
//! A local fn becomes a function of the module at its declaration. What its body refers to from
//! the enclosing scopes becomes leading parameters:
//!
//! 1. the HIR locals of the enclosing scopes that the body refers to (its captures), in the order
//!    of their first reference, and the captures of the local fns the body refers to;
//! 2. the handler values of the `with`s that enclose the declaration in the same function
//!    ([`Lowerer::bound`]), which `use X` in the body looks at first (`[effect.with]`).
//!
//! Its declared parameters follow, and the evidence table last when its R needs one, as for a
//! lambda. Every captured local is bound before the declaration and is immutable (`[fn.block]`),
//! so a call anywhere in the block can pass the same values: a call by name becomes a direct
//! call with them prepended, and the name used as a value becomes a lambda that makes that call.
//! A self tail call is then a direct self tail call, which `tail::loopify` turns into a loop, so
//! `[fn.loop]` holds with nothing else. A call inside the body to the enclosing function calls
//! another function, so it is never a call to itself.
//!
//! The lifted function has the type parameters of the enclosing function, in the same order, and
//! is called with them as its type arguments. Its types are lowered while the enclosing
//! definition is still the current one, so type variables generalized there become the same
//! `Type::Param`s. Each copy of the enclosing function (suspending copies) lifts its own.

use emela_resolve::hir::{DefKind, ExprId, ExprKind, Item, LocalFn, LocalId, Res};
use emela_types::Ty;
use rustc_hash::FxHashSet;

use super::{Call, Lowerer, Target, mentions};
use crate::ir::{Callee, Expr, FnId, Local, SetSummary, Type, TypeId};

/// A local fn lifted to a function of the module ([`Lowerer::local_fns`]).
#[derive(Debug, Clone)]
pub(super) struct Lifted {
    f: FnId,
    /// The HIR locals of the enclosing scopes whose values are passed first, in this order.
    captures: Vec<LocalId>,
    /// How many handler values of enclosing `with`s are passed after them: the first `bound` of
    /// [`Lowerer::bound`]. Every call is inside the scope of the declaration, where `bound` only
    /// grows, so these are the same `with`s at every call.
    bound: usize,
    ty_args: Vec<TypeId>,
    call: Call,
    takes_ev: bool,
}

impl Lowerer<'_> {
    /// Lifts the local fn at its declaration ([`Lowerer::local_fns`]).
    pub(super) fn local_fn(&mut self, f: &LocalFn) {
        let types = self.types;
        let Some(ty @ Ty::Fn(fn_ty)) = types.locals.get(f.local) else {
            self.report("local function without a type", f.span);
            return;
        };
        let mut captures = Vec::new();
        self.captures(f.body, &mut captures, &mut FxHashSet::default());
        let (type_params, set_params) = self.enclosing_type_params();
        let ty_args = (0..type_params)
            .map(|i| {
                let param = self.module.type_param(i);
                if set_params.contains(&i) {
                    self.module.intern(Type::Set(SetSummary {
                        params: vec![param],
                        ..SetSummary::default()
                    }))
                } else {
                    param
                }
            })
            .collect();
        let takes_ev = self.ty_takes_ev(ty);
        let call = Call {
            fails: !fn_ty.fails.is_empty(),
            suspends: self.ty_suspends(ty),
        };
        let name = self.lifted_name(&self.hir.locals[f.local].name);
        let id = self.module.declare(&name, false);
        self.module.functions[id].suspends = call.suspends;
        self.local_fns.insert(
            f.local,
            Lifted {
                f: id,
                captures: captures.clone(),
                bound: self.bound.len(),
                ty_args,
                call,
                takes_ev,
            },
        );

        // The body sees the captures and the handler values as the lifted function's parameters.
        let mut params = Vec::new();
        let mut outer_locals = Vec::new();
        for &l in &captures {
            let outer = self.locals[l];
            let data = &self.module.locals[outer];
            let (hint, ty) = (data.hint.clone(), data.ty);
            let p = self.module.local(&hint, ty);
            params.push(p);
            outer_locals.push((l, outer));
            self.locals.insert(l, p);
        }
        let outer_bound = self.bound.clone();
        for i in 0..self.bound.len() {
            let ty = self.module.locals[self.bound[i].1].ty;
            let p = self.module.local("h", ty);
            params.push(p);
            self.bound[i].1 = p;
        }
        params.extend(f.sig.params.iter().map(|p| self.local(p.local)));
        let outer_ev = self.ev;
        let outer_name = std::mem::replace(&mut self.fn_name, name);
        let body = if takes_ev {
            let p = self.ev_local();
            params.push(p);
            self.body_with_ev(p, f.body)
        } else {
            // An empty R needs no table: nothing in the body looks one up, and a `with` in it
            // starts from an empty one.
            self.ev = None;
            self.expr(f.body)
        };
        self.fn_name = outer_name;
        self.ev = outer_ev;
        self.bound = outer_bound;
        for (l, outer) in outer_locals {
            self.locals.insert(l, outer);
        }

        let ret = self.ty(&fn_ty.ret);
        self.module.define(id, params, ret, body);
        let sets = self.fn_sets(&fn_ty.fails, &fn_ty.uses);
        let func = &mut self.module.functions[id];
        func.type_params = type_params;
        func.set_params = set_params;
        func.fails = call.fails;
        func.sets = sets;
    }

    /// The local fn that `res` names, if it is one lifted in the function being lowered.
    pub(super) fn lifted(&self, res: Res) -> Option<Lifted> {
        match res {
            Res::Local(l) => self.local_fns.get(l).cloned(),
            _ => None,
        }
    }

    /// A call of a lifted local fn by its name: a direct call with the captures first.
    pub(super) fn lifted_target(&mut self, lifted: &Lifted) -> Target {
        Target::Lifted {
            f: lifted.f,
            ty_args: lifted.ty_args.clone(),
            call: lifted.call,
            captures: self.capture_args(lifted),
            ev: lifted.takes_ev.then(|| self.ev_arg()),
        }
    }

    /// A lifted local fn used as a value (`[fn.decl]`): a lambda that calls it. `id` is the
    /// reference, whose type is the function value's.
    pub(super) fn lifted_value(&mut self, id: ExprId, lifted: &Lifted) -> Expr {
        let ty = self.expr_ty(id).clone();
        let Ty::Fn(fn_ty) = &ty else {
            unreachable!("a local fn has a function type");
        };
        let mut params: Vec<Local> = fn_ty
            .params
            .iter()
            .map(|t| {
                let ty = self.ty(t);
                self.module.local("x", ty)
            })
            .collect();
        let ev = lifted.takes_ev.then(|| self.ev_local());
        let mut args = self.capture_args(lifted);
        args.extend(params.iter().map(|&p| Expr::Var(p)));
        args.extend(ev.map(Expr::Var));
        params.extend(ev);
        Expr::Lambda {
            params,
            ty: self.ty(&ty),
            body: Box::new(Expr::Call {
                callee: Callee::Direct {
                    f: lifted.f,
                    ty_args: lifted.ty_args.clone(),
                },
                args,
                fails: lifted.call.fails,
                suspends: lifted.call.suspends,
            }),
            suspends: lifted.call.suspends,
        }
    }

    /// The values of the captures and handler values of `lifted`, at a call in its scope.
    fn capture_args(&self, lifted: &Lifted) -> Vec<Expr> {
        let locals = lifted.captures.iter().map(|&l| Expr::Var(self.locals[l]));
        let bound = self.bound[..lifted.bound]
            .iter()
            .map(|(_, h)| Expr::Var(*h));
        locals.chain(bound).collect()
    }

    /// The body of a function or lambda that takes the evidence table `p`, lowered with the
    /// handler values of enclosing `with`s added to it, since `use X` looks at them first
    /// (`[effect.with]`).
    pub(super) fn body_with_ev(&mut self, p: Local, body: ExprId) -> Expr {
        let outer = self.ev;
        let (ctx, prelude) = if self.bound.is_empty() {
            (p, None)
        } else {
            let c = self.ev_local();
            let bindings = self
                .bound
                .clone()
                .into_iter()
                .map(|(e, h)| (self.effect_id(&e), Expr::Var(h)))
                .collect();
            (
                c,
                Some(Expr::Provide {
                    base: Some(Box::new(Expr::Var(p))),
                    bindings,
                }),
            )
        };
        self.ev = Some(ctx);
        let mut body = self.expr(body);
        self.ev = outer;
        if let Some(value) = prelude.filter(|_| mentions(&body, ctx)) {
            body = Expr::Let {
                var: ctx,
                value: Box::new(value),
                body: Box::new(body),
            };
        }
        body
    }

    /// Adds to `out` the HIR locals of the enclosing scopes that `expr` refers to: those already
    /// lowered (bound before the declaration), and the captures of the lifted local fns it refers
    /// to. Locals bound inside `expr` are lowered later, so they are not in [`Lowerer::locals`]
    /// yet.
    fn captures(&self, expr: ExprId, out: &mut Vec<LocalId>, seen: &mut FxHashSet<LocalId>) {
        let hir = self.hir;
        if let ExprKind::Path(Res::Local(l)) = hir.exprs[expr].kind {
            let refs = match self.local_fns.get(l) {
                Some(lifted) => lifted.captures.clone(),
                None if self.locals.get(l).is_some() => vec![l],
                None => Vec::new(),
            };
            for l in refs {
                if seen.insert(l) {
                    out.push(l);
                }
            }
        }
        for child in hir.child_exprs(expr) {
            self.captures(child, out, seen);
        }
    }

    /// The number of type parameters of the function being lowered and the indices of the set
    /// ones, which a local fn in it shares. Only fns have type parameters.
    fn enclosing_type_params(&mut self) -> (usize, Vec<usize>) {
        let Some(def) = self.current_def else {
            return (0, Vec::new());
        };
        let hir = self.hir;
        let is_fn = matches!(hir.item(def), Some(Item::Fn(_) | Item::TraitFn(_)))
            || matches!(hir.defs[def].kind, DefKind::TraitFn | DefKind::ImplFn);
        if !is_fn {
            return (0, Vec::new());
        }
        let (_, n, sets) = self.signature(def);
        (n, sets)
    }

    /// A name for a lifted local fn, unique in the module: the enclosing function's name, `$` and
    /// its own name (`$` never appears in Emela names).
    fn lifted_name(&self, name: &str) -> String {
        let base = format!("{}${name}", self.fn_name);
        let taken = |n: &str| self.module.functions.iter().any(|(_, f)| f.name == n);
        if !taken(&base) {
            return base;
        }
        (2..)
            .map(|i| format!("{base}${i}"))
            .find(|n| !taken(n))
            .expect("some suffix is free")
    }
}
