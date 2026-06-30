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
    pub effects: EffectRow,
    pub body: IrExpr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IrExpr {
    Int(i32),
    Float(f64),
    Bool(bool),
    String(String),
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
    Fn {
        params: Vec<IrParam>,
        ret: Type,
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
            IrExpr::Unit => Type::Unit,
            IrExpr::Array { elem_ty, .. } => Type::Array(Box::new(elem_ty.clone())),
            IrExpr::Var { ty, .. } => ty.clone(),
            IrExpr::FunctionRef { sig, .. } => Type::Function(sig.clone()),
            IrExpr::Let { next, .. } => next.ty(),
            IrExpr::Call { ret, .. } => ret.clone(),
            IrExpr::Fn {
                params,
                ret,
                effects,
                ..
            } => Type::Function(FunctionType {
                params: params.iter().map(|param| param.ty.clone()).collect(),
                ret: Box::new(ret.clone()),
                effects: effects.clone(),
            }),
            IrExpr::Binary { op, ty, .. } => match op {
                BinaryOp::Eq | BinaryOp::Lt => Type::Bool,
                _ => ty.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fn_ty(params: Vec<Type>, ret: Type) -> FunctionType {
        FunctionType {
            params,
            ret: Box::new(ret),
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
