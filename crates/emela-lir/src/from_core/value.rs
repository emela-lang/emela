//! Values: the variables and types of the Flow IR, primitives, calls, closures and concurrency
//! operations.
//!
//! A value of a type without representation has no operand (`Val::op` is `None`); the helpers here
//! keep that distinction, and read a value into a variable only where one is needed.

use emela_core::{
    BinOp, Builtin, Callee as CoreCallee, EffectId, Expr, FiberOp, FnType, Lit, Local, OpTy, Sig,
    Type, TypeId,
};

use super::{Div, Lower, R, TMP, Val, ix};
use crate::ir::{
    Callee, Closure, Const, LabelData, LabelId, Operand, PrimOp, PrimTy, RuntimeOp, Stmt, Term,
    Value, VarData, VarId,
};
use crate::repr::{Repr, repr_of};

impl<'s> Lower<'s> {
    pub(super) fn repr(&self, ty: TypeId) -> Repr {
        repr_of(&self.sh.core, ty)
    }

    pub(super) fn intern(&mut self, ty: Type) -> TypeId {
        self.sh.core.intern(ty)
    }

    pub(super) fn ty(&self, ty: TypeId) -> &Type {
        self.sh.core.ty(ty)
    }

    pub(super) fn subst(&mut self, ty: TypeId, args: &[TypeId]) -> TypeId {
        if args.is_empty() {
            return ty;
        }
        self.sh.core.types.subst(ty, args)
    }

    /// A new variable; the type must have a representation.
    pub(super) fn var(&mut self, hint: &str, ty: TypeId) -> VarId {
        let repr = self.repr(ty);
        debug_assert!(repr != Repr::Void, "a variable without representation");
        self.f.vars.alloc(VarData {
            hint: if hint.is_empty() { "v" } else { hint }.to_owned(),
            repr,
            ty,
        })
    }

    /// A new variable if the type has a representation.
    pub(super) fn new_var(&mut self, hint: &str, ty: TypeId) -> Option<VarId> {
        (self.repr(ty) != Repr::Void).then(|| self.var(hint, ty))
    }

    pub(super) fn param(&mut self, hint: &str, ty: TypeId) -> VarId {
        let v = self.var(hint, ty);
        self.f.params.push(v);
        v
    }

    /// A variable for a Core local (a parameter or loop variable), mapped for its uses.
    pub(super) fn local_var(&mut self, l: Local) -> Option<VarId> {
        let data = &self.sh.core.locals[l];
        let (hint, ty) = (data.hint.clone(), data.ty);
        let v = self.new_var(&hint, ty);
        self.locals.insert(l, v.map(Operand::Var));
        v
    }

    pub(super) fn label(&mut self, hint: &str) -> LabelId {
        self.f.labels.alloc(LabelData {
            hint: hint.to_owned(),
        })
    }

    /// `dst = value`, with a destination if the type has a representation.
    pub(super) fn let_(&mut self, hint: &str, ty: TypeId, value: Value) -> Option<VarId> {
        let dst = self.new_var(hint, ty);
        self.push(Stmt::Let {
            dst,
            value,
            on_defect: None,
            on_suspend: None,
        });
        dst
    }

    /// A runtime operation without arguments or value.
    pub(super) fn runtime(&mut self, op: RuntimeOp) {
        self.let_(
            TMP,
            TypeId::UNIT,
            Value::Runtime {
                op,
                args: Vec::new(),
            },
        );
    }

    /// Whether a value of type `from` needs [`Value::Coerce`] to be used at type `to`: the types
    /// differ other than in set type arguments (`Exit[Int, ()]` and `Exit[Int, {}]` are the same
    /// values), and `to` has a representation.
    pub(super) fn differs(&mut self, from: TypeId, to: TypeId) -> bool {
        if from == to || self.repr(to) == Repr::Void {
            return false;
        }
        let types = &mut self.sh.core.types;
        types.erase_sets(from) != types.erase_sets(to)
    }

    /// A read (a field, an element, a list head) that has no effect: nothing for a type without
    /// representation.
    pub(super) fn get(&mut self, hint: &str, ty: TypeId, value: Value) -> Option<VarId> {
        (self.repr(ty) != Repr::Void).then(|| self.assign(hint, ty, value))
    }

    /// `let_` for a type with a representation.
    pub(super) fn assign(&mut self, hint: &str, ty: TypeId, value: Value) -> VarId {
        let v = self.var(hint, ty);
        self.push(Stmt::Let {
            dst: Some(v),
            value,
            on_defect: None,
            on_suspend: None,
        });
        v
    }

    /// A temporary bound to `value`, as the expression's value.
    pub(super) fn emit(&mut self, ty: TypeId, value: Value) -> R {
        let dst = self.let_(TMP, ty, value);
        self.val(dst.map(Operand::Var), ty)
    }

    /// The value, or `unreachable` if its type is `Never`.
    pub(super) fn val(&mut self, op: Option<Operand>, ty: TypeId) -> R {
        if ty == TypeId::NEVER {
            self.term(Term::Unreachable);
            return Err(Div);
        }
        Ok(Val { op, ty })
    }

    /// The value as a variable (constants are bound to one). `None` without representation.
    pub(super) fn as_var(&mut self, v: &Val) -> Option<VarId> {
        match &v.op {
            Some(Operand::Var(x)) => Some(*x),
            Some(Operand::Const(c)) => Some(self.assign(TMP, v.ty, Value::Const(c.clone()))),
            None => None,
        }
    }

    /// `as_var` for a value that must have a representation.
    pub(super) fn must_var(&mut self, v: &Val, what: &str) -> VarId {
        match self.as_var(v) {
            Some(x) => x,
            None => {
                self.error(format!("{what} has no representation"));
                self.assign(TMP, TypeId::ANY, Value::Const(Const::Int(0)))
            }
        }
    }

    /// The type of a top-level function used as a value (`Typer::fn_value`).
    pub(super) fn fn_value_type(&mut self, f: emela_core::FnId) -> TypeId {
        let core = &self.sh.core;
        let func = &core.functions[f];
        let ev = core.takes_ev(f);
        let declared = &func.params[..func.params.len() - usize::from(ev)];
        let fn_ty = FnType {
            params: declared.iter().map(|&p| core.locals[p].ty).collect(),
            ret: func.ret,
            fails: func.fails,
            suspends: func.suspends,
            ev,
            sets: None,
        };
        self.intern(Type::Fn(fn_ty))
    }

    pub(super) fn call(&mut self, callee: &CoreCallee, args: &[Expr], suspends: bool) -> R {
        let (callee, ret, fails) = match callee {
            CoreCallee::Direct { f, .. } => {
                let func = &self.sh.core.functions[*f];
                (Callee::Direct(self.sh.fn_map[ix(*f)]), func.ret, func.fails)
            }
            CoreCallee::Indirect(c) => {
                let v = self.expr(c)?;
                let Type::Fn(ft) = self.ty(v.ty).clone() else {
                    self.error("a call of a value that is not a function".into());
                    return Ok(Val {
                        op: None,
                        ty: TypeId::UNIT,
                    });
                };
                let c = self.must_var(&v, "a function value");
                (Callee::Indirect(c), ft.ret, ft.fails)
            }
            CoreCallee::Op {
                handler,
                effect,
                op,
                ty_args,
            } => {
                let v = self.expr(handler)?;
                let handler = self.must_var(&v, "a handler");
                let def = &self.sh.core.effects[*effect].ops[*op];
                let (ret, fails) = (def.ret, def.fails);
                let ret = self.subst(ret, ty_args);
                (
                    Callee::Op {
                        handler,
                        effect: *effect,
                        op: *op,
                    },
                    ret,
                    fails,
                )
            }
        };
        // The operation's own type parameters (Scope's `fork[A]`) are boxed (`Any`): a child
        // function whose value is not a reference is wrapped to box it.
        let generic = match &callee {
            Callee::Op { effect, op, .. } => {
                let def = &self.sh.core.effects[*effect].ops[*op];
                (def.type_params > 0).then(|| def.params.clone())
            }
            _ => None,
        };
        let mut ops = Vec::with_capacity(args.len());
        for (i, a) in args.iter().enumerate() {
            let v = self.expr(a)?;
            let boxed = generic.as_ref().and_then(|ps| ps.get(i).copied());
            match (v.op, boxed) {
                (Some(Operand::Var(x)), Some(param)) => {
                    ops.push(Operand::Var(self.box_result(x, v.ty, param)));
                }
                (op, _) => ops.extend(op),
            }
        }
        let on_fail = fails.then(|| self.on_fail());
        self.emit(
            ret,
            Value::Call {
                callee,
                args: ops,
                suspends,
                on_fail,
            },
        )
    }

    /// The function value `f` (of type `ty`) passed where the parameter type `param` returns a
    /// type parameter of the operation (`Any`, a box): `f` itself if it returns a reference,
    /// otherwise a closure that calls it and boxes its value. A value without representation
    /// becomes an empty string (any reference does: nothing reads it).
    fn box_result(&mut self, f: VarId, ty: TypeId, param: TypeId) -> VarId {
        let (Type::Fn(ft), Type::Fn(pt)) = (self.ty(ty).clone(), self.ty(param).clone()) else {
            return f;
        };
        if !matches!(self.ty(pt.ret), Type::Param(_)) || self.repr(ft.ret) == Repr::Ref {
            return f;
        }
        let fail = std::mem::take(&mut self.fail);
        let fails = std::mem::replace(&mut self.fails, ft.fails);
        let mut params = Vec::new();
        for &p in &ft.params {
            params.extend(self.new_var("x", p));
        }
        if ft.ev {
            params.push(self.var("ev", TypeId::EVIDENCE));
        }
        self.open();
        let on_fail = ft.fails.then(|| self.on_fail());
        let r = self.let_(
            "r",
            ft.ret,
            Value::Call {
                callee: Callee::Indirect(f),
                args: params.iter().map(|&p| Operand::Var(p)).collect(),
                suspends: ft.suspends,
                on_fail,
            },
        );
        let value = match r {
            Some(r) => Operand::Var(self.assign("a", TypeId::ANY, Value::Coerce(r))),
            None => Operand::Const(Const::String(String::new())),
        };
        if self.ty(ft.ret) == &Type::Never {
            self.term(Term::Unreachable);
        } else {
            self.term(Term::Return(Some(value)));
        }
        let body = self.close();
        self.fail = fail;
        self.fails = fails;
        let boxed_ty = self.intern(Type::Fn(FnType {
            ret: TypeId::ANY,
            ..ft.clone()
        }));
        let dst = self.var("f", boxed_ty);
        self.push(Stmt::Let {
            dst: Some(dst),
            value: Value::Closure(Box::new(Closure {
                params,
                captures: Vec::new(),
                body,
                suspends: ft.suspends,
            })),
            on_defect: None,
            on_suspend: None,
        });
        dst
    }

    pub(super) fn builtin(&mut self, op: Builtin, ty_args: &[TypeId], args: &[Expr]) -> R {
        match op {
            Builtin::Dbg => {
                let v = self.expr(&args[0])?;
                let shown = self.show(ty_args[0], v.op.clone());
                self.let_(
                    TMP,
                    TypeId::UNIT,
                    Value::Runtime {
                        op: RuntimeOp::DebugPrint,
                        args: vec![shown],
                    },
                );
                Ok(v)
            }
            Builtin::DebugPrint => {
                let v = self.expr(&args[0])?;
                let text = v.op.unwrap_or(Operand::Const(Const::String(String::new())));
                self.let_(
                    TMP,
                    TypeId::UNIT,
                    Value::Runtime {
                        op: RuntimeOp::DebugPrint,
                        args: vec![text],
                    },
                );
                Ok(Val {
                    op: None,
                    ty: TypeId::UNIT,
                })
            }
            Builtin::Inspect => {
                let v = self.expr(&args[0])?;
                let shown = self.show(ty_args[0], v.op);
                Ok(Val {
                    op: Some(shown),
                    ty: TypeId::STRING,
                })
            }
            Builtin::Panic => {
                let v = self.expr(&args[0])?;
                let msg = v.op.unwrap_or(Operand::Const(Const::String(String::new())));
                self.term(Term::Defect(msg, None));
                Err(Div)
            }
            Builtin::Todo => {
                self.term(Term::Defect(
                    Operand::Const(Const::String("not yet implemented".into())),
                    None,
                ));
                Err(Div)
            }
            Builtin::AssertFail => {
                let v = self.expr(&args[0])?;
                let msg = v.op.unwrap_or(Operand::Const(Const::String(String::new())));
                let Expr::Lit(Lit::String(at)) = &args[1] else {
                    panic!("invalid IR: the location of an assert is not a string literal");
                };
                self.term(Term::Defect(msg, Some(at.clone())));
                Err(Div)
            }
            Builtin::ShowString => {
                let v = self.expr(&args[0])?;
                let shown = self.show(TypeId::STRING, v.op);
                self.val(Some(shown), TypeId::STRING)
            }
            Builtin::ShowNested => unreachable!("monomorphization replaces String.show_nested"),
            Builtin::CellNew | Builtin::CellGet | Builtin::CellSet => {
                // A cell is a container: its `()` contents keep a slot.
                let args = self.slots(args)?;
                let content = ty_args[0];
                let ty = match op {
                    Builtin::CellNew => self.intern(Type::Cell(content)),
                    Builtin::CellGet => content,
                    _ => TypeId::UNIT,
                };
                self.emit(ty, Value::Builtin { op, args })
            }
            _ => {
                let mut ops = Vec::with_capacity(args.len());
                for a in args {
                    ops.extend(self.expr(a)?.op);
                }
                let ty = self.sig_type(op.info().ret, ty_args);
                self.emit(ty, Value::Builtin { op, args: ops })
            }
        }
    }

    fn sig_type(&mut self, sig: Sig, ty_args: &[TypeId]) -> TypeId {
        match sig {
            Sig::Int => TypeId::INT,
            Sig::Int64 => TypeId::INT64,
            Sig::Float => TypeId::FLOAT,
            Sig::Bool => TypeId::BOOL,
            Sig::String => TypeId::STRING,
            Sig::Bytes => TypeId::BYTES,
            Sig::Array(t) => {
                let t = self.sig_type(*t, ty_args);
                self.intern(Type::Array(t))
            }
            Sig::Unit => TypeId::UNIT,
            Sig::Never => TypeId::NEVER,
            Sig::List(t) => {
                let t = self.sig_type(*t, ty_args);
                self.intern(Type::List(t))
            }
            Sig::Option(t) => {
                let t = self.sig_type(*t, ty_args);
                let option = self.sh.core.option_enum();
                self.intern(Type::Enum(option, vec![t]))
            }
            Sig::Param(i) => ty_args.get(i as usize).copied().unwrap_or(TypeId::ANY),
        }
    }

    pub(super) fn binary(&mut self, op: BinOp, ty: OpTy, lhs: &Expr, rhs: &Expr) -> R {
        if let BinOp::And | BinOp::Or = op {
            let a = self.expr(lhs)?;
            let cond = a.op.unwrap_or(Operand::Const(Const::Bool(false)));
            let short = Operand::Const(Const::Bool(op == BinOp::Or));
            let (right, _) = self.branch(rhs);
            self.open();
            self.term(Term::Yield(Some(short)));
            let shorted = self.close();
            let (then, else_) = if op == BinOp::And {
                (right, shorted)
            } else {
                (shorted, right)
            };
            let dst = self.var(TMP, TypeId::BOOL);
            self.push(Stmt::If {
                dst: Some(dst),
                cond,
                then,
                else_,
            });
            return self.val(Some(Operand::Var(dst)), TypeId::BOOL);
        }
        let a = self.expr(lhs)?;
        let b = self.expr(rhs)?;
        let (op, ty) = self.binop(op, ty, &a, &b);
        self.val(Some(op), ty)
    }

    /// A binary operation other than `&&` / `||` on converted operands, and its type.
    pub(super) fn binop(&mut self, op: BinOp, ty: OpTy, a: &Val, b: &Val) -> (Operand, TypeId) {
        let zero = Operand::Const(Const::Int(0));
        let (x, y) = (
            a.op.clone().unwrap_or(zero.clone()),
            b.op.clone().unwrap_or(zero),
        );
        if ty == OpTy::Structural {
            let eq = self.eq(a.ty, a.op.clone(), b.op.clone());
            let r = match op {
                BinOp::Eq => eq,
                _ => Operand::Var(self.assign(
                    TMP,
                    TypeId::BOOL,
                    Value::Prim {
                        op: PrimOp::Not,
                        ty: PrimTy::I32,
                        args: vec![eq],
                    },
                )),
            };
            return (r, TypeId::BOOL);
        }
        let prim = match op {
            BinOp::Add => PrimOp::Add,
            BinOp::Sub => PrimOp::Sub,
            BinOp::Mul => PrimOp::Mul,
            BinOp::Div => PrimOp::Div,
            BinOp::Rem => PrimOp::Rem,
            BinOp::Eq => PrimOp::Eq,
            BinOp::Ne => PrimOp::Ne,
            BinOp::Lt => PrimOp::Lt,
            BinOp::Le => PrimOp::Le,
            BinOp::Gt => PrimOp::Gt,
            BinOp::Ge => PrimOp::Ge,
            BinOp::And | BinOp::Or => unreachable!("short-circuiting operators are branches"),
        };
        let rty = if prim.is_comparison() {
            TypeId::BOOL
        } else {
            a.ty
        };
        let v = self.assign(
            TMP,
            rty,
            Value::Prim {
                op: prim,
                ty: prim_ty(ty),
                args: vec![x, y],
            },
        );
        (Operand::Var(v), rty)
    }

    pub(super) fn lambda(&mut self, params: &[Local], ty: TypeId, body: &Expr) -> R {
        let Type::Fn(ft) = self.ty(ty).clone() else {
            self.error("a lambda whose type is not a function type".into());
            return Ok(Val {
                op: None,
                ty: TypeId::UNIT,
            });
        };
        // A closure body has its own failure target.
        let fail = std::mem::take(&mut self.fail);
        let fails = std::mem::replace(&mut self.fails, ft.fails);
        let mut ps = Vec::with_capacity(params.len());
        for &p in params {
            ps.extend(self.local_var(p));
        }
        self.open();
        if let Ok(v) = self.expr(body) {
            self.term(Term::Return(v.op));
        }
        let body = self.close();
        self.fail = fail;
        self.fails = fails;
        let dst = self.var("f", ty);
        // A closure whose type says it may suspend is made suspending even if its body does not:
        // calls of a function value follow its type.
        self.push(Stmt::Let {
            dst: Some(dst),
            value: Value::Closure(Box::new(Closure {
                params: ps,
                captures: Vec::new(),
                body,
                suspends: ft.suspends,
            })),
            on_defect: None,
            on_suspend: None,
        });
        Ok(Val {
            op: Some(Operand::Var(dst)),
            ty,
        })
    }

    pub(super) fn handler(&mut self, name: &str, effect: EffectId, ops: &[Expr]) -> R {
        let mut vs = Vec::with_capacity(ops.len());
        for op in ops {
            let v = self.expr(op)?;
            vs.push(self.must_var(&v, "an operation's implementation"));
        }
        let ty = self.intern(Type::Handler(effect));
        self.emit(
            ty,
            Value::Handler {
                name: name.to_owned(),
                effect,
                ops: vs,
            },
        )
    }

    pub(super) fn fiber(&mut self, op: FiberOp, args: &[Expr]) -> R {
        let mut ops = Vec::with_capacity(args.len());
        let mut first = None;
        for a in args {
            let v = self.expr(a)?;
            first.get_or_insert(v.ty);
            ops.push(v.op.unwrap_or(Operand::Const(Const::Unit)));
        }
        let ty = self.fiber_result(op, first);
        // The `fails` of a scope and of `Cause.raise` over-approximates a set type argument that
        // monomorphisation made empty. When their failure would leave a body that cannot fail,
        // the types say it cannot fail (the raised cause is then a `Cause[{}]`).
        let op = match op {
            FiberOp::Scope { fails: true } if !self.fails && self.fail.is_empty() => {
                FiberOp::Scope { fails: false }
            }
            FiberOp::CauseRaise { fails: true } if !self.fails && self.fail.is_empty() => {
                FiberOp::CauseRaise { fails: false }
            }
            op => op,
        };
        let on_fail = op.fails().then(|| self.on_fail());
        self.emit(
            ty,
            Value::Fiber {
                op,
                args: ops,
                on_fail,
            },
        )
    }

    /// The type of a concurrency operation (`Typer::fiber_result`). `first` is the type of its
    /// first argument.
    fn fiber_result(&mut self, op: FiberOp, first: Option<TypeId>) -> TypeId {
        let first = first.unwrap_or(TypeId::ANY);
        let fn_ret = |l: &Self, t: TypeId| match l.ty(t) {
            Type::Fn(f) => f.ret,
            Type::Never => TypeId::NEVER,
            _ => TypeId::ANY,
        };
        let enum_arg = |l: &Self, t: TypeId| match l.ty(t) {
            Type::Enum(_, args) => args.first().copied().unwrap_or(TypeId::ANY),
            Type::Never => TypeId::NEVER,
            _ => TypeId::ANY,
        };
        let exit_type = |l: &mut Self, exit: emela_core::EnumId, a: TypeId| {
            let mut args = vec![TypeId::UNIT; l.sh.core.enums[exit].params];
            if let Some(f) = args.first_mut() {
                *f = a;
            }
            l.intern(Type::Enum(exit, args))
        };
        match op {
            FiberOp::Scope { .. } => fn_ret(self, first),
            FiberOp::Fork | FiberOp::Deferred | FiberOp::Semaphore | FiberOp::Queue => TypeId::ANY,
            FiberOp::Join | FiberOp::DeferredAwait => enum_arg(self, first),
            FiberOp::Await { exit } => {
                let a = enum_arg(self, first);
                exit_type(self, exit, a)
            }
            FiberOp::Attempt { exit, .. } => {
                let a = fn_ret(self, first);
                exit_type(self, exit, a)
            }
            FiberOp::Interrupt | FiberOp::Acquire | FiberOp::Release | FiberOp::Close => {
                TypeId::UNIT
            }
            FiberOp::Complete | FiberOp::TryAcquire | FiberOp::Offer | FiberOp::TryOffer => {
                TypeId::BOOL
            }
            FiberOp::Available | FiberOp::Size => TypeId::INT,
            FiberOp::Poll | FiberOp::Take | FiberOp::QueuePoll => {
                let a = enum_arg(self, first);
                let option = self.sh.core.option_enum();
                self.intern(Type::Enum(option, vec![a]))
            }
            FiberOp::CauseKind { kind } => {
                let args = vec![TypeId::UNIT; self.sh.core.enums[kind].params];
                self.intern(Type::Enum(kind, args))
            }
            FiberOp::CauseToString => TypeId::STRING,
            FiberOp::CauseRaise { .. } => TypeId::NEVER,
        }
    }
}

pub(super) fn lit_val(lit: &Lit) -> Val {
    let (c, ty) = match lit {
        Lit::Int(n) => (Const::Int(*n), TypeId::INT),
        Lit::Int64(n) => (Const::Int64(*n), TypeId::INT64),
        Lit::Float(x) => (Const::Float(*x), TypeId::FLOAT),
        Lit::Bool(b) => (Const::Bool(*b), TypeId::BOOL),
        Lit::String(s) => (Const::String(s.clone()), TypeId::STRING),
        Lit::Unit => {
            return Val {
                op: None,
                ty: TypeId::UNIT,
            };
        }
    };
    Val {
        op: Some(Operand::Const(c)),
        ty,
    }
}

pub(super) fn lit_const(lit: &Lit) -> Option<Const> {
    lit_val(lit).op.map(|o| match o {
        Operand::Const(c) => c,
        Operand::Var(_) => unreachable!(),
    })
}

pub(super) fn prim_ty(ty: OpTy) -> PrimTy {
    match ty {
        OpTy::Int | OpTy::Bool => PrimTy::I32,
        OpTy::Int64 => PrimTy::I64,
        OpTy::Float => PrimTy::F64,
        OpTy::String | OpTy::Structural => PrimTy::String,
    }
}
