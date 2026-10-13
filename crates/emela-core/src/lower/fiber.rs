//! 並行性（11章）の写し方．
//!
//! `Std.Fiber`，`Std.Cause`，`Std.Deferred` の `@intrinsic` の関数は，Core IR の `Expr::Fiber` に
//! 写す（[`FiberFn`]）．組み込みのエフェクト Scope の能力の値は，`Fiber.scope` を呼んだところで
//! lowering が作るハンドラの値（[`ScopeCall`]）．操作 fork と deferred は普通の操作の呼び出しになり，
//! fork だけは呼ぶ側で子の関数を包む（[`ForkCall`]）．
//!
//! 子が使うハンドラは `Fiber.scope` を呼んだ時点のもので，Scope だけはその子が属すスコープ自身を指す
//! （11.2）．Scope のハンドラの値は，`Fiber.scope` を呼んだ位置の証拠の表を閉じ込めて持ち，fork の
//! 実装がその表で子を呼ぶ．Scope は fork した能力の値（呼ぶ側が子の関数を包むときに足す）．スコープの
//! 本体の中の with は子に届かない．

use emela_resolve::hir::{DefId, DefModule, ExprId, Item, Span};
use emela_types::{FnTy, Tag, Ty};

use super::{Call, Lowerer, Target};
use crate::ir::{Callee, EffectId, EnumId, Expr, FiberOp, Lit, Local};

/// lowering が `Expr::Fiber` に写す `@intrinsic` の関数．
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FiberFn {
    Scope,
    Join,
    Await,
    Interrupt,
    Attempt,
    Complete,
    DeferredAwait,
    Poll,
    CauseKind,
    CauseToString,
    CauseRaise,
}

impl FiberFn {
    /// 同梱の宣言（モジュール名と関数名）から引く．
    pub(super) fn lookup(module: &str, name: &str) -> Option<FiberFn> {
        Some(match (module, name) {
            ("Std.Fiber", "scope") => FiberFn::Scope,
            ("Std.Fiber", "join") => FiberFn::Join,
            ("Std.Fiber", "await") => FiberFn::Await,
            ("Std.Fiber", "interrupt") => FiberFn::Interrupt,
            ("Std.Fiber", "attempt") => FiberFn::Attempt,
            ("Std.Deferred", "complete") => FiberFn::Complete,
            ("Std.Deferred", "await") => FiberFn::DeferredAwait,
            ("Std.Deferred", "poll") => FiberFn::Poll,
            ("Std.Cause", "kind") => FiberFn::CauseKind,
            ("Std.Cause", "to_string") => FiberFn::CauseToString,
            ("Std.Cause", "raise") => FiberFn::CauseRaise,
            _ => return None,
        })
    }
}

/// `Fiber.scope(body)` を写すのに要るもの．局所変数は先に割り振っておく．
///
/// ```text
/// let b = body
/// fiber#scope(fn(s) {
///   let h = Scope { fork: fn(g) { fiber#fork(s, fn() { g(ev) }) }, deferred: fn() { fiber#deferred(s) } }
///   b(ev + { Scope: h })
/// })
/// ```
///
/// `ev` は `Fiber.scope` を呼んだ位置の証拠の表．fork の実装は子の関数 `g` をその表で呼ぶ（`g` は
/// 呼ぶ側が [`ForkCall`] で包んだもので，受けた表に Scope を足して子の関数を呼ぶ）．
pub(super) struct ScopeCall {
    /// Scope の IR のエフェクト．
    effect: EffectId,
    /// 操作 fork と deferred の `EffectDef::ops` の添字．
    ops: [usize; 2],
    /// `Fiber.scope` を呼んだ位置の証拠の表．
    ev: Option<Local>,
    /// スコープ（と子）が失敗しうるか．
    fails: bool,
    /// 本体の呼び出しの印．
    body_call: Call,
    body: Local,
    scope: Local,
    handler: Local,
    g: Local,
}

impl ScopeCall {
    pub(super) fn apply(self, args: Vec<Expr>) -> Expr {
        let [body]: [Expr; 1] = args.try_into().expect("Fiber.scope の引数は1つ");
        let ev = || match self.ev {
            Some(ev) => Expr::Var(ev),
            None => Expr::Lit(Lit::Unit),
        };
        // 子は中断しうる関数として走らせる（g が中断しない関数でもランタイムがそのまま進める）．
        let child = Expr::Lambda {
            params: Vec::new(),
            body: Box::new(Expr::Call {
                callee: Callee::Indirect(Box::new(Expr::Var(self.g))),
                args: vec![ev()],
                fails: self.fails,
                suspends: true,
            }),
            suspends: true,
        };
        let fork = Expr::Lambda {
            params: vec![self.g],
            body: Box::new(Expr::Fiber {
                op: FiberOp::Fork,
                args: vec![Expr::Var(self.scope), child],
            }),
            suspends: false,
        };
        let deferred = Expr::Lambda {
            params: Vec::new(),
            body: Box::new(Expr::Fiber {
                op: FiberOp::Deferred,
                args: vec![Expr::Var(self.scope)],
            }),
            suspends: false,
        };
        let mut ops = vec![Expr::Lit(Lit::Unit), Expr::Lit(Lit::Unit)];
        ops[self.ops[0]] = fork;
        ops[self.ops[1]] = deferred;
        let run = Expr::Call {
            callee: Callee::Indirect(Box::new(Expr::Var(self.body))),
            args: vec![Expr::Provide {
                base: self.ev.map(|ev| Box::new(Expr::Var(ev))),
                bindings: vec![(self.effect, Expr::Var(self.handler))],
            }],
            fails: self.body_call.fails,
            suspends: self.body_call.suspends,
        };
        let inner = Expr::Lambda {
            params: vec![self.scope],
            body: Box::new(Expr::Let {
                var: self.handler,
                value: Box::new(Expr::Handler {
                    name: "Scope".to_owned(),
                    effect: self.effect,
                    ops,
                }),
                body: Box::new(run),
            }),
            suspends: true,
        };
        Expr::Let {
            var: self.body,
            value: Box::new(body),
            body: Box::new(Expr::Fiber {
                op: FiberOp::Scope { fails: self.fails },
                args: vec![inner],
            }),
        }
    }
}

/// `s.fork(f)` を写すのに要るもの．
///
/// ```text
/// let c = s
/// let x = f
/// c.fork(fn(ev) { x(ev + { Scope: c }) })
/// ```
///
/// 包んだ関数は，Scope のハンドラの fork の実装（[`ScopeCall`]）が `Fiber.scope` を呼んだ位置の表で
/// 呼ぶ．子の Scope は fork した能力の値 `c`（子が兄弟を fork できるように，11.2）．`f` の R が表を
/// 要らなければ表を渡さない．
pub(super) struct ForkCall {
    /// 能力の値の式．
    pub(super) handler: Expr,
    effect: EffectId,
    op: usize,
    /// 子の関数 `f` の呼び出しの印．
    f_call: Call,
    /// `f` が証拠の表を受けるか．
    f_takes_ev: bool,
    cap: Local,
    f: Local,
    ev: Local,
}

impl ForkCall {
    pub(super) fn apply(self, args: Vec<Expr>) -> Expr {
        let [f]: [Expr; 1] = args.try_into().expect("fork の引数は1つ");
        let child_args = if self.f_takes_ev {
            vec![Expr::Provide {
                base: Some(Box::new(Expr::Var(self.ev))),
                bindings: vec![(self.effect, Expr::Var(self.cap))],
            }]
        } else {
            Vec::new()
        };
        let g = Expr::Lambda {
            params: vec![self.ev],
            body: Box::new(Expr::Call {
                callee: Callee::Indirect(Box::new(Expr::Var(self.f))),
                args: child_args,
                fails: self.f_call.fails,
                suspends: self.f_call.suspends,
            }),
            suspends: self.f_call.suspends,
        };
        let fork = Expr::Call {
            callee: Callee::Op {
                handler: Box::new(Expr::Var(self.cap)),
                effect: self.effect,
                op: self.op,
            },
            args: vec![g],
            fails: false,
            suspends: false,
        };
        let lets = [(self.cap, self.handler), (self.f, f)];
        lets.into_iter()
            .rev()
            .fold(fork, |body, (var, value)| Expr::Let {
                var,
                value: Box::new(value),
                body: Box::new(body),
            })
    }
}

/// `Fiber.attempt(f)` を写すのに要るもの．`f` の R が表を要るなら，今の表で `f` を呼ぶ引数のない
/// 関数に包む（ランタイムは `f` を引数なしで呼ぶ）．
pub(super) struct AttemptCall {
    exit: EnumId,
    f_call: Call,
    /// `f` に渡す今の証拠の表．表を要らなければ `None`．
    ev: Option<Expr>,
    f: Local,
}

impl AttemptCall {
    pub(super) fn apply(self, args: Vec<Expr>) -> Expr {
        let [f]: [Expr; 1] = args.try_into().expect("Fiber.attempt の引数は1つ");
        let op = FiberOp::Attempt {
            exit: self.exit,
            suspends: self.f_call.suspends,
        };
        let Some(ev) = self.ev else {
            return Expr::Fiber { op, args: vec![f] };
        };
        let wrapped = Expr::Lambda {
            params: Vec::new(),
            body: Box::new(Expr::Call {
                callee: Callee::Indirect(Box::new(Expr::Var(self.f))),
                args: vec![ev],
                fails: self.f_call.fails,
                suspends: self.f_call.suspends,
            }),
            suspends: self.f_call.suspends,
        };
        Expr::Let {
            var: self.f,
            value: Box::new(f),
            body: Box::new(Expr::Fiber {
                op,
                args: vec![wrapped],
            }),
        }
    }
}

impl Lowerer<'_> {
    /// 組み込みのエフェクト（`@intrinsic effect`．今は Scope だけ）か．
    pub(super) fn is_intrinsic_effect(&self, effect: DefId) -> bool {
        matches!(self.hir.item(effect), Some(Item::Effect(e)) if e.is_intrinsic)
    }

    /// `@intrinsic` の fn `def` が 11章の関数なら，その呼ばれる側．`callee` は参照の式．
    pub(super) fn fiber_target(
        &mut self,
        def: DefId,
        callee: ExprId,
        call: Call,
        span: Span,
    ) -> Option<Result<Target, Expr>> {
        let data = &self.hir.defs[def];
        let module = match &data.module {
            DefModule::Source(m) => self.modules[*m].name.to_string(),
            _ => return None,
        };
        let which = FiberFn::lookup(&module, &data.name)?;
        let Ty::Fn(fn_ty) = self.expr_ty(callee).clone() else {
            return Some(Err(
                self.unsupported(&format!("{module}.{}", data.name), span)
            ));
        };
        let simple = |op| Some(Ok(Target::Fiber(op)));
        match which {
            FiberFn::Scope => Some(
                self.scope_call(&fn_ty, call)
                    .map(|s| Target::Scope(Box::new(s)))
                    .ok_or_else(|| self.unsupported("Fiber.scope", span)),
            ),
            FiberFn::Join => simple(FiberOp::Join),
            FiberFn::Interrupt => simple(FiberOp::Interrupt),
            FiberFn::Complete => simple(FiberOp::Complete),
            FiberFn::DeferredAwait => simple(FiberOp::DeferredAwait),
            FiberFn::Poll => simple(FiberOp::Poll),
            FiberFn::CauseToString => simple(FiberOp::CauseToString),
            FiberFn::CauseRaise => simple(FiberOp::CauseRaise),
            FiberFn::Await => match self.ret_enum(&fn_ty) {
                Some(exit) => simple(FiberOp::Await { exit }),
                None => Some(Err(self.unsupported("Fiber.await", span))),
            },
            FiberFn::CauseKind => match self.ret_enum(&fn_ty) {
                Some(kind) => simple(FiberOp::CauseKind { kind }),
                None => Some(Err(self.unsupported("Cause.kind", span))),
            },
            FiberFn::Attempt => {
                let (Some(exit), Some(f_ty)) = (self.ret_enum(&fn_ty), fn_ty.params.first()) else {
                    return Some(Err(self.unsupported("Fiber.attempt", span)));
                };
                let f_call = Call {
                    fails: matches!(f_ty, Ty::Fn(f) if !f.fails.is_empty()),
                    suspends: self.ty_suspends(f_ty),
                };
                let ev = self.ty_takes_ev(f_ty).then(|| self.ev_arg());
                Some(Ok(Target::Attempt(Box::new(AttemptCall {
                    exit,
                    f_call,
                    ev,
                    f: self.module.local("f"),
                }))))
            }
        }
    }

    /// 戻り値の型の enum（Exit，CauseKind）．
    fn ret_enum(&self, fn_ty: &FnTy) -> Option<EnumId> {
        match &fn_ty.ret {
            Ty::Named(con, _) => self.enums.get(self.types.ty_table.def(*con)).copied(),
            _ => None,
        }
    }

    /// `Fiber.scope` の呼び出しを写す準備．`fn_ty` は具体化した `Fiber.scope` の型．
    fn scope_call(&mut self, fn_ty: &FnTy, call: Call) -> Option<ScopeCall> {
        let Some(Ty::Fn(body)) = fn_ty.params.first() else {
            return None;
        };
        let tag = body
            .uses
            .tags()
            .find(|t| self.is_intrinsic_effect(t.def))?
            .clone();
        let (effect, ops) = self.scope_effect(&tag)?;
        Some(ScopeCall {
            effect,
            ops,
            ev: self.ev,
            fails: call.fails,
            body_call: Call {
                fails: !body.fails.is_empty(),
                suspends: self.row_suspends(&body.uses),
            },
            body: self.module.local("body"),
            scope: self.module.local("s"),
            handler: self.module.local("scope"),
            g: self.module.local("g"),
        })
    }

    /// Scope のタグの IR のエフェクトと，操作 fork と deferred の添字．
    fn scope_effect(&mut self, tag: &Tag) -> Option<(EffectId, [usize; 2])> {
        let effect = self.effect_id(&self.key_here(tag));
        let index = |name: &str| {
            self.module.effects[effect]
                .ops
                .iter()
                .position(|o| o.name == name)
        };
        Some((effect, [index("fork")?, index("deferred")?]))
    }

    /// 能力の値 `handler`（型の Scope のタグは `tag`）の fork の呼び出しを写す準備．`op_ty` は
    /// 具体化した fork の型．
    pub(super) fn fork_call(&mut self, handler: Expr, tag: &Tag, op_ty: &Ty) -> Option<ForkCall> {
        let (effect, [fork, _]) = self.scope_effect(tag)?;
        let Ty::Fn(op_ty) = op_ty else {
            return None;
        };
        let f_ty = op_ty.params.first()?;
        Some(ForkCall {
            handler,
            effect,
            op: fork,
            f_call: Call {
                fails: matches!(f_ty, Ty::Fn(f) if !f.fails.is_empty()),
                suspends: self.ty_suspends(f_ty),
            },
            f_takes_ev: self.ty_takes_ev(f_ty),
            cap: self.module.local("scope"),
            f: self.module.local("f"),
            ev: self.module.local("ev"),
        })
    }

    /// 能力の値の操作 `op` が Scope の fork か．
    pub(super) fn is_fork_op(&self, effect: DefId, op: DefId) -> bool {
        self.is_intrinsic_effect(effect) && self.hir.defs[op].name == "fork"
    }
}
