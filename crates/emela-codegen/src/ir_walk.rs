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
        IrExpr::Platform { args, .. }
        | IrExpr::Intrinsic { args, .. }
        | IrExpr::TailSelfCall { args, .. } => {
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
        IrExpr::RecordValue { fields, .. } => fields.iter().for_each(|e| walk(e, visit)),
        IrExpr::FieldAccess { target, .. } => walk(target, visit),
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
        IrExpr::Throw { value } | IrExpr::Question { value, .. } | IrExpr::Retain { value } => {
            walk(value, visit)
        }
        IrExpr::Release { next, .. } => walk(next, visit),
        IrExpr::Cleanup { body, action } => {
            walk(body, visit);
            walk(action, visit);
        }
        IrExpr::Panic { message } => walk(message, visit),
        _ => {}
    }
}

/// Visits the immediate sub-expressions of `expr` — one level only, so a caller
/// can decide per node whether to descend. Passes that rewrite in place (the
/// cleanup expansion, spec 0056; the RC pass, spec 0048) share this instead of
/// each spelling out the same match.
pub(crate) fn walk_children_mut(expr: &mut IrExpr, visit: &mut impl FnMut(&mut IrExpr)) {
    match expr {
        IrExpr::Array { elems, .. } => elems.iter_mut().for_each(visit),
        IrExpr::Let { value, next, .. } => {
            visit(value);
            visit(next);
        }
        IrExpr::Call { callee, args, .. } => {
            visit(callee);
            args.iter_mut().for_each(visit);
        }
        IrExpr::Platform { args, .. }
        | IrExpr::Intrinsic { args, .. }
        | IrExpr::TailSelfCall { args, .. } => args.iter_mut().for_each(visit),
        IrExpr::Fn { body, .. } => visit(body),
        IrExpr::Binary { left, right, .. } | IrExpr::Concat { left, right } => {
            visit(left);
            visit(right);
        }
        IrExpr::If {
            cond, then, els, ..
        } => {
            visit(cond);
            visit(then);
            visit(els);
        }
        IrExpr::RecordValue { fields, .. } => fields.iter_mut().for_each(visit),
        IrExpr::FieldAccess { target, .. } => visit(target),
        IrExpr::EnumValue { payload, .. } => payload.iter_mut().for_each(visit),
        IrExpr::Match {
            scrutinee, arms, ..
        } => {
            visit(scrutinee);
            walk_arms_mut(arms, visit);
        }
        IrExpr::Try { body, arms, .. } => {
            visit(body);
            walk_arms_mut(arms, visit);
        }
        IrExpr::Throw { value } | IrExpr::Question { value, .. } | IrExpr::Retain { value } => {
            visit(value)
        }
        IrExpr::Panic { message } => visit(message),
        IrExpr::Release { next, .. } => visit(next),
        IrExpr::Cleanup { body, action } => {
            visit(body);
            visit(action);
        }
        IrExpr::Int(_)
        | IrExpr::Float(_)
        | IrExpr::Bool(_)
        | IrExpr::String(_)
        | IrExpr::Char(_)
        | IrExpr::Unit
        | IrExpr::Var { .. }
        | IrExpr::FunctionRef { .. } => {}
    }
}

/// The immutable twin of [`walk_children_mut`].
pub(crate) fn walk_children<'a>(expr: &'a IrExpr, visit: &mut impl FnMut(&'a IrExpr)) {
    match expr {
        IrExpr::Array { elems, .. } => elems.iter().for_each(visit),
        IrExpr::Let { value, next, .. } => {
            visit(value);
            visit(next);
        }
        IrExpr::Call { callee, args, .. } => {
            visit(callee);
            args.iter().for_each(visit);
        }
        IrExpr::Platform { args, .. }
        | IrExpr::Intrinsic { args, .. }
        | IrExpr::TailSelfCall { args, .. } => args.iter().for_each(visit),
        IrExpr::Fn { body, .. } => visit(body),
        IrExpr::Binary { left, right, .. } | IrExpr::Concat { left, right } => {
            visit(left);
            visit(right);
        }
        IrExpr::If {
            cond, then, els, ..
        } => {
            visit(cond);
            visit(then);
            visit(els);
        }
        IrExpr::RecordValue { fields, .. } => fields.iter().for_each(visit),
        IrExpr::FieldAccess { target, .. } => visit(target),
        IrExpr::EnumValue { payload, .. } => payload.iter().for_each(visit),
        IrExpr::Match {
            scrutinee, arms, ..
        } => {
            visit(scrutinee);
            for arm in arms {
                arm.guard.iter().for_each(&mut *visit);
                visit(&arm.body);
            }
        }
        IrExpr::Try { body, arms, .. } => {
            visit(body);
            for arm in arms {
                arm.guard.iter().for_each(&mut *visit);
                visit(&arm.body);
            }
        }
        IrExpr::Throw { value } | IrExpr::Question { value, .. } | IrExpr::Retain { value } => {
            visit(value)
        }
        IrExpr::Panic { message } => visit(message),
        IrExpr::Release { next, .. } => visit(next),
        IrExpr::Cleanup { body, action } => {
            visit(body);
            visit(action);
        }
        IrExpr::Int(_)
        | IrExpr::Float(_)
        | IrExpr::Bool(_)
        | IrExpr::String(_)
        | IrExpr::Char(_)
        | IrExpr::Unit
        | IrExpr::Var { .. }
        | IrExpr::FunctionRef { .. } => {}
    }
}

fn walk_arms_mut(arms: &mut [IrArm], visit: &mut impl FnMut(&mut IrExpr)) {
    for arm in arms {
        arm.guard.iter_mut().for_each(&mut *visit);
        visit(&mut arm.body);
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
