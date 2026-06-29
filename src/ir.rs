use crate::ast::{BinaryOp, EffectRow, Expr, Program, Type};
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
    Bool(bool),
    String(String),
    Unit,
    Var(String),
    Let {
        name: String,
        value: Box<IrExpr>,
        next: Box<IrExpr>,
    },
    Call {
        name: String,
        args: Vec<IrExpr>,
    },
    Binary {
        op: BinaryOp,
        left: Box<IrExpr>,
        right: Box<IrExpr>,
    },
}

pub(crate) fn lower(program: &Program, typed: &TypedProgram) -> IrProgram {
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
            body: lower_block(&function.body.items),
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
        out.push_str(type_name(&function.ret));
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
        IrExpr::Bool(value) => value.to_string(),
        IrExpr::String(value) => format!("{value:?}"),
        IrExpr::Unit => "()".to_string(),
        IrExpr::Var(name) => format!("%{name}"),
        IrExpr::Let { .. } => {
            let mut out = String::from("{\n");
            emit_expr_text(expr, 1, &mut out);
            out.push('}');
            out
        }
        IrExpr::Call { name, args } => format!(
            "call @{}({})",
            name,
            args.iter().map(inline_expr).collect::<Vec<_>>().join(", ")
        ),
        IrExpr::Binary { op, left, right } => format!(
            "{} {}, {}",
            ir_op(*op),
            inline_expr(left),
            inline_expr(right)
        ),
    }
}

fn ir_op(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "add.i32",
        BinaryOp::Sub => "sub.i32",
        BinaryOp::Mul => "mul.i32",
        BinaryOp::Eq => "eq.i32",
        BinaryOp::Lt => "lt.i32",
    }
}

fn type_name(ty: &Type) -> &'static str {
    match ty {
        Type::Unit => "Unit",
        Type::Bool => "Bool",
        Type::Int => "Int",
        Type::String => "String",
    }
}

fn lower_block(items: &[crate::ast::BlockItem]) -> IrExpr {
    match items.split_first() {
        None => IrExpr::Unit,
        Some((crate::ast::BlockItem::Expr(expr), [])) => lower_expr(expr),
        Some((crate::ast::BlockItem::Expr(_), rest)) => lower_block(rest),
        Some((crate::ast::BlockItem::Let { name, value, .. }, rest)) => IrExpr::Let {
            name: name.clone(),
            value: Box::new(lower_expr(value)),
            next: Box::new(lower_block(rest)),
        },
    }
}

fn lower_expr(expr: &Expr) -> IrExpr {
    match expr {
        Expr::Int(value, _) => IrExpr::Int(*value),
        Expr::Bool(value, _) => IrExpr::Bool(*value),
        Expr::String(value, _) => IrExpr::String(value.clone()),
        Expr::Unit(_) => IrExpr::Unit,
        Expr::Var(name, _) => IrExpr::Var(name.clone()),
        Expr::Call { name, args, .. } => IrExpr::Call {
            name: name.clone(),
            args: args.iter().map(lower_expr).collect(),
        },
        Expr::Binary {
            op, left, right, ..
        } => IrExpr::Binary {
            op: *op,
            left: Box::new(lower_expr(left)),
            right: Box::new(lower_expr(right)),
        },
        Expr::Block(block) => lower_block(&block.items),
    }
}
