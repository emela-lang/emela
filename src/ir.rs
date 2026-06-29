use std::collections::HashMap;

use crate::ast::{BinaryOp, EffectRow, Expr, FunctionType, Program, Type};
use crate::typecheck::TypedProgram;

#[derive(Debug, Clone)]
pub(crate) struct IrProgram {
    pub(crate) functions: Vec<IrFunction>,
}

#[derive(Debug, Clone)]
pub(crate) struct IrFunction {
    pub(crate) name: String,
    pub(crate) params: Vec<String>,
    pub(crate) ret: Type,
    pub(crate) effects: EffectRow,
    pub(crate) body: IrExpr,
}

#[derive(Debug, Clone)]
pub(crate) enum IrExpr {
    Int(i32),
    Float(f64),
    Bool(bool),
    String(String),
    Array(Vec<IrExpr>),
    Unit,
    Var(String),
    FunctionRef(String),
    Let {
        name: String,
        value: Box<IrExpr>,
        next: Box<IrExpr>,
    },
    Call {
        callee: Box<IrExpr>,
        args: Vec<IrExpr>,
    },
    Fn {
        params: Vec<String>,
        body: Box<IrExpr>,
    },
    Binary {
        op: BinaryOp,
        ty: Type,
        left: Box<IrExpr>,
        right: Box<IrExpr>,
    },
}

pub(crate) fn lower(program: &Program, typed: &TypedProgram) -> IrProgram {
    let function_types: HashMap<_, _> = typed
        .functions
        .iter()
        .map(|function| {
            (
                function.name.clone(),
                Type::Function(FunctionType {
                    params: function.params.clone(),
                    ret: Box::new(function.ret.clone()),
                    effects: function.effects.clone(),
                }),
            )
        })
        .collect();
    let functions = program
        .functions
        .iter()
        .zip(typed.functions.iter())
        .map(|(function, typed)| IrFunction {
            name: function.name.clone(),
            params: function
                .params
                .iter()
                .map(|param| param.name.clone())
                .collect(),
            ret: typed.ret.clone(),
            effects: typed.effects.clone(),
            body: lower_function_body(function, &function_types),
        })
        .collect();
    IrProgram { functions }
}

pub(crate) fn emit_text(program: &IrProgram) -> String {
    let mut out = String::new();
    for function in &program.functions {
        out.push_str("fn ");
        out.push_str(&function.name);
        out.push('(');
        out.push_str(&function.params.join(", "));
        out.push_str(") -> ");
        out.push_str(&type_name(&function.ret));
        out.push_str(" uses {");
        out.push_str(&function.effects.effects.join(", "));
        out.push_str("} {\n");
        emit_expr_text(&function.body, 1, &mut out);
        out.push_str("}\n\n");
    }
    out
}

fn emit_expr_text(expr: &IrExpr, indent: usize, out: &mut String) {
    let pad = "  ".repeat(indent);
    match expr {
        IrExpr::Let { name, value, next } => {
            out.push_str(&pad);
            out.push_str("let ");
            out.push_str(name);
            out.push_str(" = ");
            out.push_str(&inline_expr(value));
            out.push('\n');
            emit_expr_text(next, indent, out);
        }
        other => {
            out.push_str(&pad);
            out.push_str("return ");
            out.push_str(&inline_expr(other));
            out.push('\n');
        }
    }
}

fn inline_expr(expr: &IrExpr) -> String {
    match expr {
        IrExpr::Int(value) => value.to_string(),
        IrExpr::Float(value) => value.to_string(),
        IrExpr::Bool(value) => value.to_string(),
        IrExpr::String(value) => format!("{value:?}"),
        IrExpr::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(inline_expr)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        IrExpr::Unit => "()".to_string(),
        IrExpr::Var(name) => format!("%{name}"),
        IrExpr::FunctionRef(name) => format!("@{name}"),
        IrExpr::Let { .. } => {
            let mut out = String::from("{\n");
            emit_expr_text(expr, 1, &mut out);
            out.push('}');
            out
        }
        IrExpr::Call { callee, args } => format!(
            "call {}({})",
            inline_callee(callee),
            args.iter().map(inline_expr).collect::<Vec<_>>().join(", ")
        ),
        IrExpr::Fn { params, body } => {
            format!("fn ({}) {{ {} }}", params.join(", "), inline_expr(body))
        }
        IrExpr::Binary {
            op,
            ty,
            left,
            right,
        } => format!(
            "{}.{} {}, {}",
            ir_op(*op),
            ir_type_suffix(ty),
            inline_expr(left),
            inline_expr(right)
        ),
    }
}

fn ir_op(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "add",
        BinaryOp::Sub => "sub",
        BinaryOp::Mul => "mul",
        BinaryOp::Eq => "eq",
        BinaryOp::Lt => "lt",
    }
}

fn ir_type_suffix(ty: &Type) -> &'static str {
    match ty {
        Type::Float => "f64",
        _ => "i32",
    }
}

fn type_name(ty: &Type) -> String {
    match ty {
        Type::Unit => "Unit".to_string(),
        Type::Bool => "Bool".to_string(),
        Type::Int => "Int".to_string(),
        Type::Float => "Float".to_string(),
        Type::String => "String".to_string(),
        Type::Array(element) => format!("Array<{}>", type_name(element)),
        Type::Record => "Record".to_string(),
        Type::Enum => "Enum".to_string(),
        Type::Function(function) => format!(
            "({}) -> {} uses {{{}}}",
            function
                .params
                .iter()
                .map(type_name)
                .collect::<Vec<_>>()
                .join(", "),
            type_name(&function.ret),
            function.effects.effects.join(", ")
        ),
        Type::OpaqueFunction => "Function".to_string(),
    }
}

fn inline_callee(expr: &IrExpr) -> String {
    match expr {
        IrExpr::FunctionRef(name) => format!("@{name}"),
        other => inline_expr(other),
    }
}

fn lower_function_body(
    function: &crate::ast::Function,
    function_types: &HashMap<String, Type>,
) -> IrExpr {
    let mut scope = function
        .params
        .iter()
        .map(|param| (param.name.clone(), param.ty.clone()))
        .collect();
    lower_block(&function.body.items, &mut scope, function_types).0
}

fn lower_block(
    items: &[crate::ast::BlockItem],
    scope: &mut HashMap<String, Type>,
    function_types: &HashMap<String, Type>,
) -> (IrExpr, Type) {
    match items.split_first() {
        None => (IrExpr::Unit, Type::Unit),
        Some((crate::ast::BlockItem::Expr(expr), [])) => lower_expr(expr, scope, function_types),
        Some((crate::ast::BlockItem::Expr(_), rest)) => lower_block(rest, scope, function_types),
        Some((
            crate::ast::BlockItem::Let {
                name, ty, value, ..
            },
            rest,
        )) => {
            let (value, value_ty) = lower_expr(value, scope, function_types);
            scope.insert(name.clone(), ty.clone().unwrap_or(value_ty));
            let (next, next_ty) = lower_block(rest, scope, function_types);
            (
                IrExpr::Let {
                    name: name.clone(),
                    value: Box::new(value),
                    next: Box::new(next),
                },
                next_ty,
            )
        }
    }
}

fn lower_expr(
    expr: &Expr,
    scope: &mut HashMap<String, Type>,
    function_types: &HashMap<String, Type>,
) -> (IrExpr, Type) {
    match expr {
        Expr::Int(value, _) => (IrExpr::Int(*value), Type::Int),
        Expr::Float(value, _) => (IrExpr::Float(*value), Type::Float),
        Expr::Bool(value, _) => (IrExpr::Bool(*value), Type::Bool),
        Expr::String(value, _) => (IrExpr::String(value.clone()), Type::String),
        Expr::Array(values, _) => {
            let lowered = values
                .iter()
                .map(|value| lower_expr(value, scope, function_types))
                .collect::<Vec<_>>();
            let ty = lowered
                .first()
                .map(|(_, ty)| Type::Array(Box::new(ty.clone())))
                .unwrap_or_else(|| Type::Array(Box::new(Type::Unit)));
            (
                IrExpr::Array(lowered.into_iter().map(|(expr, _)| expr).collect()),
                ty,
            )
        }
        Expr::Unit(_) => (IrExpr::Unit, Type::Unit),
        Expr::Var(name, _) => {
            if let Some(ty) = scope.get(name) {
                (IrExpr::Var(name.clone()), ty.clone())
            } else if let Some(ty) = function_types.get(name) {
                (IrExpr::FunctionRef(name.clone()), ty.clone())
            } else {
                (IrExpr::Var(name.clone()), Type::Unit)
            }
        }
        Expr::Call { callee, args, .. } => {
            let (callee, callee_ty) = lower_expr(callee, scope, function_types);
            let ret = match callee_ty {
                Type::Function(function) => (*function.ret).clone(),
                _ => Type::Unit,
            };
            (
                IrExpr::Call {
                    callee: Box::new(callee),
                    args: args
                        .iter()
                        .map(|arg| lower_expr(arg, scope, function_types).0)
                        .collect(),
                },
                ret,
            )
        }
        Expr::Fn {
            params,
            ret,
            effects,
            body,
            ..
        } => {
            let mut fn_scope = scope.clone();
            for param in params {
                fn_scope.insert(param.name.clone(), param.ty.clone());
            }
            let (body, _) = lower_block(&body.items, &mut fn_scope, function_types);
            (
                IrExpr::Fn {
                    params: params.iter().map(|param| param.name.clone()).collect(),
                    body: Box::new(body),
                },
                Type::Function(FunctionType {
                    params: params.iter().map(|param| param.ty.clone()).collect(),
                    ret: Box::new(ret.clone()),
                    effects: effects.clone(),
                }),
            )
        }
        Expr::Binary {
            op, left, right, ..
        } => {
            let (left, left_ty) = lower_expr(left, scope, function_types);
            let (right, _) = lower_expr(right, scope, function_types);
            let result_ty = match op {
                BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul => left_ty.clone(),
                BinaryOp::Eq | BinaryOp::Lt => Type::Bool,
            };
            (
                IrExpr::Binary {
                    op: *op,
                    ty: left_ty,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                result_ty,
            )
        }
        Expr::Block(block) => lower_block(&block.items, &mut scope.clone(), function_types),
    }
}
