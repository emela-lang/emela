//! The Emela intermediate representation.
//!
//! The IR is the boundary between the frontend (source -> IR, in the `emela`
//! crate) and code generation (IR -> artifact, the [`crate::Backend`] trait).
//! It is serializable so it can also be handed to external-process plugins.
//!
//! Every node carries enough type information that [`IrExpr::ty`] is total:
//! backends (notably WebAssembly) need concrete types to pick representations,
//! and the frontend already computes them during lowering.

use serde::{Deserialize, Serialize};

use crate::types::{BinaryOp, EffectRow, FunctionType, Type};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IrProgram {
    pub functions: Vec<IrFunction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IrParam {
    pub name: String,
    pub ty: Type,
}

/// A variable captured by a closure, with its type. The order of this list is
/// the closure's environment layout: backends store and load captures in it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IrCapture {
    pub name: String,
    pub ty: Type,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IrFunction {
    pub name: String,
    pub params: Vec<IrParam>,
    pub ret: Type,
    /// The error type this function may throw (spec 0011), if any. When set, the
    /// function reports on the error channel in addition to its value channel.
    #[serde(default)]
    pub throws: Option<Type>,
    pub effects: EffectRow,
    pub body: IrExpr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IrExpr {
    Int(i32),
    Float(f64),
    Bool(bool),
    String(String),
    /// A `Char` literal as a Unicode scalar value (spec 0017).
    Char(u32),
    Unit,
    Array {
        elem_ty: Type,
        elems: Vec<IrExpr>,
    },
    Var {
        name: String,
        ty: Type,
    },
    FunctionRef {
        name: String,
        sig: FunctionType,
    },
    Let {
        name: String,
        value_ty: Type,
        value: Box<IrExpr>,
        next: Box<IrExpr>,
    },
    Call {
        callee: Box<IrExpr>,
        args: Vec<IrExpr>,
        ret: Type,
    },
    /// A call to a platform function (spec 0013), resolved by the backend's
    /// runtime. `name` is the canonical platform name, e.g. `io.write_stdout`.
    /// A fallible entry (spec 0043) carries its error type in `throws`: the
    /// host reports failure through the ordinary Result representation and the
    /// backend unwraps it at the call site like any throwing call.
    Platform {
        name: String,
        args: Vec<IrExpr>,
        ret: Type,
        #[serde(default)]
        throws: Option<Type>,
    },
    /// A call to an intrinsic (spec 0021), inlined by the backend to a native
    /// instruction. `name` is the intrinsic's bare name, e.g. `i32_add`. Pure.
    Intrinsic {
        name: String,
        args: Vec<IrExpr>,
        ret: Type,
    },
    Fn {
        params: Vec<IrParam>,
        ret: Type,
        #[serde(default)]
        throws: Option<Type>,
        effects: EffectRow,
        captures: Vec<IrCapture>,
        body: Box<IrExpr>,
    },
    Binary {
        op: BinaryOp,
        ty: Type,
        left: Box<IrExpr>,
        right: Box<IrExpr>,
    },
    /// `if cond { then } else { els }` (spec 0015). Both branches have type `ty`.
    If {
        cond: Box<IrExpr>,
        then: Box<IrExpr>,
        els: Box<IrExpr>,
        ty: Type,
    },
    /// `a ++ b` (spec 0017): String concatenation. The user-level `++` operator
    /// lowers through the `Concat` trait to the `string_concat` intrinsic (spec
    /// 0020/0021); this node remains for compiler-internal concatenation, e.g.
    /// building a `@test` failure message (spec 0040).
    Concat {
        left: Box<IrExpr>,
        right: Box<IrExpr>,
    },
    /// A record value (spec 0006). `fields` are in declaration order. The
    /// layout is the enum payload layout without a tag: one 8-byte slot per
    /// field.
    RecordValue {
        ty: Type,
        fields: Vec<IrExpr>,
    },
    /// A record field access `value.field` (spec 0006): reads the field at
    /// `index` (declaration order). `field_ty` picks the load width.
    FieldAccess {
        target: Box<IrExpr>,
        index: u32,
        field_ty: Type,
    },
    /// An enum or `Option` value (spec 0005/0001). `tag` selects the variant in
    /// declaration order; `payload` carries its fields.
    EnumValue {
        ty: Type,
        variant: String,
        tag: u32,
        payload: Vec<IrExpr>,
    },
    /// A `match` over an enum/`Option` (spec 0005). Arms are tried top to bottom.
    Match {
        scrutinee: Box<IrExpr>,
        arms: Vec<IrArm>,
        ty: Type,
    },
    /// `throw e` (spec 0011): raise `e` on the error channel. Type `Never`.
    Throw {
        value: Box<IrExpr>,
    },
    /// `try { body } catch { arms }` (spec 0011): evaluate `body`, routing any
    /// thrown error to `arms`.
    Try {
        body: Box<IrExpr>,
        arms: Vec<IrArm>,
        ty: Type,
        /// The name the caught error value is bound under for the catch arms
        /// (spec 0048): the RC pass sets it and releases it at each arm tail;
        /// backends that emit RC ops bind the error local under this name.
        /// `None` before the pass runs (and for host-GC backends).
        #[serde(default)]
        err_name: Option<String>,
    },
    /// `expr?` (spec 0011): take the success value, short-circuiting the
    /// enclosing function on error (`Throws`) or `None` (`Option`).
    Question {
        value: Box<IrExpr>,
        mode: QuestionMode,
        ty: Type,
    },
    /// `panic(msg)` (spec 0011): unrecoverable abort. Type `Never`.
    Panic {
        message: Box<IrExpr>,
    },
    /// A direct self-recursive call in tail position (spec 0045). Backends emit
    /// it as a jump back to the function head (no stack growth) instead of a
    /// call. Produced by [`crate::tailcall::rewrite_self_tail_calls`]; never
    /// built by the frontend directly.
    TailSelfCall {
        args: Vec<IrExpr>,
        ty: Type,
    },
    /// `retain e` (spec 0048 A8): +1 the heap value `e` evaluates to, then
    /// yield that value. Produced by [`crate::rc::insert_rc_ops`] in owned
    /// positions (after the pass `e` is always a `Var`); never built by the
    /// frontend. Backends that reclaim by other means (host GC) treat it as
    /// `e` itself (spec 0048 A9).
    Retain {
        value: Box<IrExpr>,
    },
    /// `release x in e` (spec 0048 A8): -1 the binding `name` of type `ty`
    /// (reclaiming its value at zero), then evaluate `next`. Produced by
    /// [`crate::rc::insert_rc_ops`] at scope exits; never built by the
    /// frontend. Backends that reclaim by other means treat it as `next`
    /// (spec 0048 A9).
    Release {
        name: String,
        ty: Type,
        next: Box<IrExpr>,
    },
}

impl IrExpr {
    /// The Emela result type of this expression. Total: every variant yields a
    /// type without re-running inference.
    pub fn ty(&self) -> Type {
        match self {
            IrExpr::Int(_) => Type::Int,
            IrExpr::Float(_) => Type::Float,
            IrExpr::Bool(_) => Type::Bool,
            IrExpr::String(_) => Type::String,
            IrExpr::Char(_) => Type::Char,
            IrExpr::Concat { .. } => Type::String,
            IrExpr::Unit => Type::Unit,
            IrExpr::Array { elem_ty, .. } => Type::Array(Box::new(elem_ty.clone())),
            IrExpr::Var { ty, .. } => ty.clone(),
            IrExpr::FunctionRef { sig, .. } => Type::Function(sig.clone()),
            IrExpr::Let { next, .. } => next.ty(),
            IrExpr::Call { ret, .. } => ret.clone(),
            IrExpr::Platform { ret, .. } => ret.clone(),
            IrExpr::Intrinsic { ret, .. } => ret.clone(),
            IrExpr::Fn {
                params,
                ret,
                throws,
                effects,
                ..
            } => Type::Function(FunctionType {
                params: params.iter().map(|param| param.ty.clone()).collect(),
                ret: Box::new(ret.clone()),
                throws: throws.clone().map(Box::new),
                effects: effects.clone(),
            }),
            IrExpr::Binary { op, ty, .. } => match op {
                BinaryOp::Eq
                | BinaryOp::Lt
                | BinaryOp::Ne
                | BinaryOp::Gt
                | BinaryOp::Le
                | BinaryOp::Ge => Type::Bool,
                _ => ty.clone(),
            },
            IrExpr::EnumValue { ty, .. }
            | IrExpr::RecordValue { ty, .. }
            | IrExpr::Match { ty, .. }
            | IrExpr::Try { ty, .. }
            | IrExpr::If { ty, .. }
            | IrExpr::Question { ty, .. }
            | IrExpr::TailSelfCall { ty, .. } => ty.clone(),
            IrExpr::FieldAccess { field_ty, .. } => field_ty.clone(),
            IrExpr::Throw { .. } | IrExpr::Panic { .. } => Type::Never,
            IrExpr::Retain { value } => value.ty(),
            IrExpr::Release { next, .. } => next.ty(),
        }
    }
}

/// One arm of a `match` or `try`/`catch` (spec 0005/0011).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IrArm {
    pub pattern: IrPattern,
    pub guard: Option<IrExpr>,
    pub body: IrExpr,
}

/// A pattern matched against an enum/`Option` scrutinee.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IrPattern {
    /// A specific variant, selected by `tag`. Each payload field is bound by
    /// `Some((name, ty))` or ignored with `None`.
    Variant {
        variant: String,
        tag: u32,
        bindings: Vec<Option<(String, Type)>>,
    },
    /// A wildcard (`_`) or catch-all binding, which binds the whole scrutinee.
    Wildcard { binding: Option<(String, Type)> },
}

/// What `?` propagates (spec 0011).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuestionMode {
    /// Propagate a thrown error to the enclosing `throws` channel.
    Throws,
    /// Propagate `None` to the enclosing `Option` return.
    Option,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fn_ty(params: Vec<Type>, ret: Type) -> FunctionType {
        FunctionType {
            params,
            ret: Box::new(ret),
            throws: None,
            effects: EffectRow::default(),
        }
    }

    #[test]
    fn ty_is_total_over_variants() {
        assert_eq!(IrExpr::Int(1).ty(), Type::Int);
        assert_eq!(IrExpr::Float(1.0).ty(), Type::Float);
        assert_eq!(IrExpr::Bool(true).ty(), Type::Bool);
        assert_eq!(IrExpr::String("x".into()).ty(), Type::String);
        assert_eq!(IrExpr::Unit.ty(), Type::Unit);
        assert_eq!(
            IrExpr::Array {
                elem_ty: Type::Int,
                elems: vec![IrExpr::Int(1)]
            }
            .ty(),
            Type::Array(Box::new(Type::Int))
        );
        assert_eq!(
            IrExpr::Var {
                name: "x".into(),
                ty: Type::Bool
            }
            .ty(),
            Type::Bool
        );
        assert_eq!(
            IrExpr::FunctionRef {
                name: "f".into(),
                sig: fn_ty(vec![Type::Int], Type::Int)
            }
            .ty(),
            Type::Function(fn_ty(vec![Type::Int], Type::Int))
        );
        assert_eq!(
            IrExpr::Let {
                name: "x".into(),
                value_ty: Type::Int,
                value: Box::new(IrExpr::Int(1)),
                next: Box::new(IrExpr::Bool(true)),
            }
            .ty(),
            Type::Bool
        );
        assert_eq!(
            IrExpr::Call {
                callee: Box::new(IrExpr::FunctionRef {
                    name: "f".into(),
                    sig: fn_ty(vec![], Type::Float)
                }),
                args: vec![],
                ret: Type::Float,
            }
            .ty(),
            Type::Float
        );
        assert_eq!(
            IrExpr::Binary {
                op: BinaryOp::Lt,
                ty: Type::Int,
                left: Box::new(IrExpr::Int(1)),
                right: Box::new(IrExpr::Int(2)),
            }
            .ty(),
            Type::Bool
        );
        assert_eq!(
            IrExpr::Binary {
                op: BinaryOp::Add,
                ty: Type::Float,
                left: Box::new(IrExpr::Float(1.0)),
                right: Box::new(IrExpr::Float(2.0)),
            }
            .ty(),
            Type::Float
        );
    }
}
