//! Lowering: the typed AST -> the `emela-codegen` IR.
//!
//! The IR is fully typed, so every node records the type that the type checker
//! already computed. Lambdas additionally record their captured variables, in
//! a stable order, for closure-converting backends. Calls to `extern fn`
//! platform functions (spec 0013) become `IrExpr::Platform` nodes.

use std::collections::{HashMap, HashSet};

use emela_codegen::{
    BinaryOp, FunctionType, IrCapture, IrExpr, IrFunction, IrParam, IrProgram, Type,
};

use crate::ast::{BlockItem, Expr, Program};
use crate::typecheck::TypedProgram;

type FunctionTypes = HashMap<String, FunctionType>;
type Scope = HashMap<String, Type>;

/// A platform function in scope: its canonical name and return type.
struct ExternInfo {
    canonical: String,
    ret: Type,
}
type Externs = HashMap<String, ExternInfo>;

pub(crate) fn lower(program: &Program, typed: &TypedProgram) -> IrProgram {
    let function_types: FunctionTypes = typed
        .functions
        .iter()
        .map(|function| {
            (
                function.name.clone(),
                FunctionType {
                    params: function.params.clone(),
                    ret: Box::new(function.ret.clone()),
                    effects: function.effects.clone(),
                },
            )
        })
        .collect();
    let externs: Externs = program
        .externs
        .iter()
        .map(|declaration| {
            (
                declaration.name.clone(),
                ExternInfo {
                    canonical: declaration.canonical(),
                    ret: declaration.ret.clone(),
                },
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
                .map(|param| IrParam {
                    name: param.name.clone(),
                    ty: param.ty.clone(),
                })
                .collect(),
            ret: typed.ret.clone(),
            effects: typed.effects.clone(),
            body: lower_function_body(function, &function_types, &externs),
        })
        .collect();
    IrProgram { functions }
}

fn lower_function_body(
    function: &crate::ast::Function,
    function_types: &FunctionTypes,
    externs: &Externs,
) -> IrExpr {
    let mut scope = function
        .params
        .iter()
        .map(|param| (param.name.clone(), param.ty.clone()))
        .collect();
    lower_block(&function.body.items, &mut scope, function_types, externs).0
}

fn lower_block(
    items: &[BlockItem],
    scope: &mut Scope,
    function_types: &FunctionTypes,
    externs: &Externs,
) -> (IrExpr, Type) {
    match items.split_first() {
        None => (IrExpr::Unit, Type::Unit),
        Some((BlockItem::Expr(expr), [])) => lower_expr(expr, scope, function_types, externs),
        Some((BlockItem::Expr(_), rest)) => lower_block(rest, scope, function_types, externs),
        Some((
            BlockItem::Let {
                name, ty, value, ..
            },
            rest,
        )) => {
            let expected_elem = match (value, ty) {
                (Expr::Array(_, _), Some(Type::Array(element))) => Some(element.as_ref()),
                _ => None,
            };
            let (value, inferred) = match value {
                Expr::Array(elements, _) => {
                    lower_array(elements, scope, function_types, externs, expected_elem)
                }
                _ => lower_expr(value, scope, function_types, externs),
            };
            let value_ty = ty.clone().unwrap_or(inferred);
            scope.insert(name.clone(), value_ty.clone());
            let (next, next_ty) = lower_block(rest, scope, function_types, externs);
            (
                IrExpr::Let {
                    name: name.clone(),
                    value_ty,
                    value: Box::new(value),
                    next: Box::new(next),
                },
                next_ty,
            )
        }
    }
}

fn lower_array(
    elements: &[Expr],
    scope: &mut Scope,
    function_types: &FunctionTypes,
    externs: &Externs,
    expected_elem: Option<&Type>,
) -> (IrExpr, Type) {
    let lowered = elements
        .iter()
        .map(|element| lower_expr(element, scope, function_types, externs))
        .collect::<Vec<_>>();
    let elem_ty = lowered
        .first()
        .map(|(_, ty)| ty.clone())
        .or_else(|| expected_elem.cloned())
        .unwrap_or(Type::Unit);
    (
        IrExpr::Array {
            elem_ty: elem_ty.clone(),
            elems: lowered.into_iter().map(|(expr, _)| expr).collect(),
        },
        Type::Array(Box::new(elem_ty)),
    )
}

fn lower_expr(
    expr: &Expr,
    scope: &mut Scope,
    function_types: &FunctionTypes,
    externs: &Externs,
) -> (IrExpr, Type) {
    match expr {
        Expr::Int(value, _) => (IrExpr::Int(*value), Type::Int),
        Expr::Float(value, _) => (IrExpr::Float(*value), Type::Float),
        Expr::Bool(value, _) => (IrExpr::Bool(*value), Type::Bool),
        Expr::String(value, _) => (IrExpr::String(value.clone()), Type::String),
        Expr::Array(elements, _) => lower_array(elements, scope, function_types, externs, None),
        Expr::Unit(_) => (IrExpr::Unit, Type::Unit),
        Expr::Var(name, _) => {
            if let Some(ty) = scope.get(name) {
                (
                    IrExpr::Var {
                        name: name.clone(),
                        ty: ty.clone(),
                    },
                    ty.clone(),
                )
            } else if let Some(sig) = function_types.get(name) {
                (
                    IrExpr::FunctionRef {
                        name: name.clone(),
                        sig: sig.clone(),
                    },
                    Type::Function(sig.clone()),
                )
            } else {
                (
                    IrExpr::Var {
                        name: name.clone(),
                        ty: Type::Unit,
                    },
                    Type::Unit,
                )
            }
        }
        Expr::Call { callee, args, .. } => {
            // A call to a platform function (extern) lowers to a Platform node.
            if let Expr::Var(name, _) = callee.as_ref() {
                if let Some(info) = externs.get(name) {
                    let ret = info.ret.clone();
                    let args = args
                        .iter()
                        .map(|arg| lower_expr(arg, scope, function_types, externs).0)
                        .collect();
                    return (
                        IrExpr::Platform {
                            name: info.canonical.clone(),
                            args,
                            ret: ret.clone(),
                        },
                        ret,
                    );
                }
            }
            let (callee, callee_ty) = lower_expr(callee, scope, function_types, externs);
            let ret = match callee_ty {
                Type::Function(function) => (*function.ret).clone(),
                _ => Type::Unit,
            };
            (
                IrExpr::Call {
                    callee: Box::new(callee),
                    args: args
                        .iter()
                        .map(|arg| lower_expr(arg, scope, function_types, externs).0)
                        .collect(),
                    ret: ret.clone(),
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
            let captures = lambda_captures(params, body, scope);
            let mut fn_scope = scope.clone();
            for param in params {
                fn_scope.insert(param.name.clone(), param.ty.clone());
            }
            let (body, _) = lower_block(&body.items, &mut fn_scope, function_types, externs);
            let ir_params: Vec<IrParam> = params
                .iter()
                .map(|param| IrParam {
                    name: param.name.clone(),
                    ty: param.ty.clone(),
                })
                .collect();
            let signature = FunctionType {
                params: ir_params.iter().map(|param| param.ty.clone()).collect(),
                ret: Box::new(ret.clone()),
                effects: effects.clone(),
            };
            (
                IrExpr::Fn {
                    params: ir_params,
                    ret: ret.clone(),
                    effects: effects.clone(),
                    captures,
                    body: Box::new(body),
                },
                Type::Function(signature),
            )
        }
        Expr::Binary {
            op, left, right, ..
        } => {
            let (left, left_ty) = lower_expr(left, scope, function_types, externs);
            let (right, _) = lower_expr(right, scope, function_types, externs);
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
        Expr::Block(block) => {
            lower_block(&block.items, &mut scope.clone(), function_types, externs)
        }
    }
}

/// The variables a lambda captures from its enclosing runtime scope, in
/// first-occurrence order. Top-level functions and platform functions are not
/// in `scope`, so they are never captured.
fn lambda_captures(
    params: &[crate::ast::Param],
    body: &crate::ast::Block,
    scope: &Scope,
) -> Vec<IrCapture> {
    let bound: HashSet<String> = params.iter().map(|param| param.name.clone()).collect();
    let mut free = Vec::new();
    free_vars_block(&body.items, &bound, &mut free);
    free.into_iter()
        .filter_map(|name| {
            scope.get(&name).map(|ty| IrCapture {
                name,
                ty: ty.clone(),
            })
        })
        .collect()
}

fn free_vars_block(items: &[BlockItem], bound: &HashSet<String>, out: &mut Vec<String>) {
    let mut bound = bound.clone();
    for item in items {
        match item {
            BlockItem::Let { name, value, .. } => {
                free_vars_expr(value, &bound, out);
                bound.insert(name.clone());
            }
            BlockItem::Expr(expr) => free_vars_expr(expr, &bound, out),
        }
    }
}

fn free_vars_expr(expr: &Expr, bound: &HashSet<String>, out: &mut Vec<String>) {
    match expr {
        Expr::Var(name, _) => {
            if !bound.contains(name) && !out.contains(name) {
                out.push(name.clone());
            }
        }
        Expr::Array(elements, _) => {
            for element in elements {
                free_vars_expr(element, bound, out);
            }
        }
        Expr::Call { callee, args, .. } => {
            free_vars_expr(callee, bound, out);
            for arg in args {
                free_vars_expr(arg, bound, out);
            }
        }
        Expr::Binary { left, right, .. } => {
            free_vars_expr(left, bound, out);
            free_vars_expr(right, bound, out);
        }
        Expr::Fn { params, body, .. } => {
            let mut inner = bound.clone();
            for param in params {
                inner.insert(param.name.clone());
            }
            free_vars_block(&body.items, &inner, out);
        }
        Expr::Block(block) => free_vars_block(&block.items, bound, out),
        Expr::Int(_, _)
        | Expr::Float(_, _)
        | Expr::Bool(_, _)
        | Expr::String(_, _)
        | Expr::Unit(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_program;
    use crate::typecheck;

    fn lower_source(source: &str) -> IrProgram {
        let program = parse_program("test", source).expect("parse");
        let typed = typecheck::check(&program).expect("typecheck");
        lower(&program, &typed)
    }

    fn main_body(ir: &IrProgram) -> &IrExpr {
        &ir.functions
            .iter()
            .find(|function| function.name == "main")
            .expect("main")
            .body
    }

    // Walk to the first `Fn` literal in an expression tree.
    fn first_lambda(expr: &IrExpr) -> Option<&IrExpr> {
        match expr {
            IrExpr::Fn { .. } => Some(expr),
            IrExpr::Let { value, next, .. } => first_lambda(value).or_else(|| first_lambda(next)),
            IrExpr::Call { callee, args, .. } => {
                first_lambda(callee).or_else(|| args.iter().find_map(first_lambda))
            }
            IrExpr::Binary { left, right, .. } => {
                first_lambda(left).or_else(|| first_lambda(right))
            }
            IrExpr::Array { elems, .. } => elems.iter().find_map(first_lambda),
            _ => None,
        }
    }

    #[test]
    fn lambda_captures_enclosing_binding() {
        let ir = lower_source(
            "fn make_adder(n: Int) -> (Int) -> Int {\n  fn (x: Int) -> Int { x + n }\n}\nfn main() -> Int { let a = make_adder(1) a(41) }\n",
        );
        let adder = ir
            .functions
            .iter()
            .find(|function| function.name == "make_adder")
            .expect("make_adder");
        let lambda = first_lambda(&adder.body).expect("lambda");
        let IrExpr::Fn { captures, .. } = lambda else {
            panic!("expected Fn");
        };
        assert_eq!(captures.len(), 1);
        assert_eq!(captures[0].name, "n");
        assert_eq!(captures[0].ty, Type::Int);
    }

    #[test]
    fn top_level_functions_are_not_captured() {
        let ir = lower_source(
            "fn helper(x: Int) -> Int { x }\nfn main() -> Int {\n  let k = 2\n  let f = fn (x: Int) -> Int { helper(x) + k }\n  f(40)\n}\n",
        );
        let lambda = first_lambda(main_body(&ir)).expect("lambda");
        let IrExpr::Fn { captures, .. } = lambda else {
            panic!("expected Fn");
        };
        let names: Vec<&str> = captures.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["k"]);
    }
}
