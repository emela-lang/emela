//! IR を組み立てる小さな関数．テストと，後で書く変換の両方で使う．

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
    }
}

pub fn call_value(callee: Expr, args: Vec<Expr>) -> Expr {
    Expr::Call {
        callee: Callee::Indirect(Box::new(callee)),
        args,
    }
}

pub fn builtin(op: Builtin, args: Vec<Expr>) -> Expr {
    Expr::Builtin {
        op,
        ty_args: Vec::new(),
        args,
    }
}

/// 型引数を取る組み込み関数（`dbg`）の呼び出し．
pub fn builtin_ty(op: Builtin, ty_args: Vec<Type>, args: Vec<Expr>) -> Expr {
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

pub fn value_part(e: Expr, ty: Type) -> StrPart {
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
