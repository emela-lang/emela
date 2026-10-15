//! Small helpers for building IR, used by tests and lowering.

use crate::intrinsics::Builtin;
use crate::ir::*;

pub fn int(n: i32) -> Expr {
    Expr::Lit(Lit::Int(n))
}

pub fn int64(n: i64) -> Expr {
    Expr::Lit(Lit::Int64(n))
}

pub fn float(x: f64) -> Expr {
    Expr::Lit(Lit::Float(x))
}

pub fn bool(b: bool) -> Expr {
    Expr::Lit(Lit::Bool(b))
}

pub fn string(s: &str) -> Expr {
    Expr::Lit(Lit::String(s.to_owned()))
}

pub fn unit() -> Expr {
    Expr::Lit(Lit::Unit)
}

pub fn var(l: Local) -> Expr {
    Expr::Var(l)
}

pub fn let_(var: Local, value: Expr, body: Expr) -> Expr {
    Expr::Let {
        var,
        value: Box::new(value),
        body: Box::new(body),
    }
}

pub fn call(f: FnId, args: Vec<Expr>) -> Expr {
    Expr::Call {
        callee: Callee::Direct(f),
        args,
        fails: false,
        suspends: false,
    }
}

pub fn call_value(callee: Expr, args: Vec<Expr>) -> Expr {
    Expr::Call {
        callee: Callee::Indirect(Box::new(callee)),
        args,
        fails: false,
        suspends: false,
    }
}

/// A direct call to a fallible function (`fails: true`).
pub fn call_fails(f: FnId, args: Vec<Expr>) -> Expr {
    Expr::Call {
        callee: Callee::Direct(f),
        args,
        fails: true,
        suspends: false,
    }
}

/// A call to a fallible function value (`fails: true`).
pub fn call_value_fails(callee: Expr, args: Vec<Expr>) -> Expr {
    Expr::Call {
        callee: Callee::Indirect(Box::new(callee)),
        args,
        fails: true,
        suspends: false,
    }
}

/// A direct call to a suspending function (`suspends: true`).
pub fn call_suspends(f: FnId, args: Vec<Expr>) -> Expr {
    Expr::Call {
        callee: Callee::Direct(f),
        args,
        fails: false,
        suspends: true,
    }
}

/// A call to a suspending function value (`suspends: true`).
pub fn call_value_suspends(callee: Expr, args: Vec<Expr>) -> Expr {
    Expr::Call {
        callee: Callee::Indirect(Box::new(callee)),
        args,
        fails: false,
        suspends: true,
    }
}

/// A call to an operation of the handler value `handler` (spec 8.2).
pub fn call_op(
    handler: Expr,
    effect: EffectId,
    op: usize,
    args: Vec<Expr>,
    fails: bool,
    suspends: bool,
) -> Expr {
    Expr::Call {
        callee: Callee::Op {
            handler: Box::new(handler),
            effect,
            op,
        },
        args,
        fails,
        suspends,
    }
}

/// `fail e`．
pub fn fail(e: Expr) -> Expr {
    Expr::Fail(Box::new(e))
}

/// `body escape { arms }`．
pub fn escape(body: Expr, arms: Vec<Arm>) -> Expr {
    Expr::Escape {
        body: Box::new(body),
        arms,
    }
}

pub fn builtin(op: Builtin, args: Vec<Expr>) -> Expr {
    Expr::Builtin {
        op,
        ty_args: Vec::new(),
        args,
    }
}

/// A call to a builtin that takes type arguments (`dbg`).
pub fn builtin_ty(op: Builtin, ty_args: Vec<TypeId>, args: Vec<Expr>) -> Expr {
    Expr::Builtin { op, ty_args, args }
}

pub fn ctor(ctor: CtorRef, args: Vec<Expr>) -> Expr {
    Expr::Ctor { ctor, args }
}

pub fn field(base: Expr, ctor: CtorRef, index: usize) -> Expr {
    Expr::Field {
        base: Box::new(base),
        ctor,
        index,
    }
}

pub fn if_(cond: Expr, then: Expr, else_: Expr) -> Expr {
    Expr::If {
        cond: Box::new(cond),
        then: Box::new(then),
        else_: Box::new(else_),
    }
}

pub fn match_(scrutinee: Expr, arms: Vec<Arm>) -> Expr {
    Expr::Match {
        scrutinee: Box::new(scrutinee),
        arms,
    }
}

pub fn arm(pat: Pat, body: Expr) -> Arm {
    Arm {
        pat,
        guard: None,
        body,
    }
}

pub fn arm_if(pat: Pat, guard: Expr, body: Expr) -> Arm {
    Arm {
        pat,
        guard: Some(guard),
        body,
    }
}

pub fn bin(op: BinOp, ty: OpTy, lhs: Expr, rhs: Expr) -> Expr {
    Expr::Binary {
        op,
        ty,
        lhs: Box::new(lhs),
        rhs: Box::new(rhs),
    }
}

pub fn unary(op: UnOp, ty: OpTy, operand: Expr) -> Expr {
    Expr::Unary {
        op,
        ty,
        operand: Box::new(operand),
    }
}

pub fn lambda(params: Vec<Local>, body: Expr) -> Expr {
    Expr::Lambda {
        params,
        body: Box::new(body),
        suspends: false,
    }
}

/// A suspending lambda (`suspends: true`).
pub fn suspend_lambda(params: Vec<Local>, body: Expr) -> Expr {
    Expr::Lambda {
        params,
        body: Box::new(body),
        suspends: true,
    }
}

pub fn list(elems: Vec<Expr>) -> Expr {
    Expr::List { elems, tail: None }
}

pub fn list_with_tail(elems: Vec<Expr>, tail: Expr) -> Expr {
    Expr::List {
        elems,
        tail: Some(Box::new(tail)),
    }
}

pub fn lit_part(s: &str) -> StrPart {
    StrPart::Lit(s.to_owned())
}

pub fn value_part(e: Expr, ty: TypeId) -> StrPart {
    StrPart::Value(e, ty)
}

pub fn p_ctor(ctor: CtorRef, fields: Vec<Pat>) -> Pat {
    Pat::Ctor { ctor, fields }
}

pub fn p_list(elems: Vec<Pat>, rest: Option<Pat>) -> Pat {
    Pat::List {
        elems,
        rest: rest.map(Box::new),
    }
}

/// A concurrency operation (spec chapter 11).
pub fn fiber(op: FiberOp, args: Vec<Expr>) -> Expr {
    Expr::Fiber { op, args }
}
