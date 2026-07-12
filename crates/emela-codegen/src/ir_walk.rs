//! Shared read-only traversal of the IR.
//!
//! Backends need to walk an [`IrProgram`] for several purposes: discovering the
//! intrinsics (spec 0021) and platform functions (spec 0013) it references (for
//! coverage checks and runtime bundling), collecting lambdas, and so on. The
//! traversal lives here so every backend shares one definition instead of each
//! reimplementing the same match over [`IrExpr`].

use std::collections::HashSet;

use crate::ir::{IrArm, IrExpr, IrProgram};

/// Visits every sub-expression of `expr`, parents before children (pre-order).
pub fn walk<'a>(expr: &'a IrExpr, visit: &mut impl FnMut(&'a IrExpr)) {
    visit(expr);
    match expr {
        IrExpr::Array { elems, .. } => elems.iter().for_each(|e| walk(e, visit)),
        IrExpr::Let { value, next, .. } => {
            walk(value, visit);
            walk(next, visit);
        }
        IrExpr::Call { callee, args, .. } => {
            walk(callee, visit);
            args.iter().for_each(|a| walk(a, visit));
        }
        IrExpr::Platform { args, .. } | IrExpr::Intrinsic { args, .. } => {
            args.iter().for_each(|a| walk(a, visit));
        }
        IrExpr::If {
            cond, then, els, ..
        } => {
            walk(cond, visit);
            walk(then, visit);
            walk(els, visit);
        }
        IrExpr::Fn { body, .. } => walk(body, visit),
        IrExpr::Binary { left, right, .. } | IrExpr::Concat { left, right } => {
            walk(left, visit);
            walk(right, visit);
        }
        IrExpr::EnumValue { payload, .. } => payload.iter().for_each(|e| walk(e, visit)),
        IrExpr::Match {
            scrutinee, arms, ..
        } => {
            walk(scrutinee, visit);
            walk_arms(arms, visit);
        }
        IrExpr::Try { body, arms, .. } => {
            walk(body, visit);
            walk_arms(arms, visit);
        }
        IrExpr::Throw { value } | IrExpr::Question { value, .. } => walk(value, visit),
        IrExpr::Panic { message } => walk(message, visit),
        IrExpr::CharFromCode(value) | IrExpr::StringFromChar(value) => walk(value, visit),
        IrExpr::ArrayLength(array) => walk(array, visit),
        IrExpr::ArrayGet { array, index, .. } => {
            walk(array, visit);
            walk(index, visit);
        }
        IrExpr::ArrayPush { array, value, .. } => {
            walk(array, visit);
            walk(value, visit);
        }
        _ => {}
    }
}

fn walk_arms<'a>(arms: &'a [IrArm], visit: &mut impl FnMut(&'a IrExpr)) {
    for arm in arms {
        if let Some(guard) = &arm.guard {
            walk(guard, visit);
        }
        walk(&arm.body, visit);
    }
}

/// The intrinsic names the program references, in first-occurrence order.
pub fn used_intrinsics(program: &IrProgram) -> Vec<String> {
    let mut order = Vec::new();
    let mut seen = HashSet::new();
    for function in &program.functions {
        walk(&function.body, &mut |expr| {
            if let IrExpr::Intrinsic { name, .. } = expr
                && seen.insert(name.clone())
            {
                order.push(name.clone());
            }
        });
    }
    order
}

/// The platform-function names the program references, in first-occurrence order.
pub fn used_platform_fns(program: &IrProgram) -> Vec<String> {
    let mut order = Vec::new();
    let mut seen = HashSet::new();
    for function in &program.functions {
        walk(&function.body, &mut |expr| {
            if let IrExpr::Platform { name, .. } = expr
                && seen.insert(name.clone())
            {
                order.push(name.clone());
            }
        });
    }
    order
}
