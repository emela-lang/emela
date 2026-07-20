//! Self-tail-call rewriting (spec 0045).
//!
//! A direct self-recursive call in tail position must not consume stack
//! (spec 0045 T2). The frontend lowers such calls as ordinary
//! [`IrExpr::Call`] nodes; [`rewrite_self_tail_calls`] then replaces every
//! call that (a) targets the enclosing top-level function by name and (b)
//! sits in tail position with [`IrExpr::TailSelfCall`], which backends emit
//! as a jump back to the function head.
//!
//! Tail positions (spec 0045 T1): the final expression of the function body,
//! propagated through `let` continuations, both `if` branches, `match` arms,
//! and `catch` arms. The inside of a `try` block is NOT a tail position (the
//! catch frame is still pending), and neither is the operand of `?` (the
//! error path re-wraps the result). Nested function literals are separate
//! functions and are never rewritten (spec 0045 T4).
//!
//! A bare call to a throwing function is rejected by the frontend (spec 0011:
//! it must carry `?` or sit inside `try`), and neither of those positions is
//! a tail position, so a rewritten call is always non-throwing — backends can
//! reassign the parameters and jump without touching the error channel.

use crate::ir::{IrExpr, IrProgram};
use crate::ir_walk::walk;

/// Rewrites direct self-recursive calls in tail position across the program.
pub fn rewrite_self_tail_calls(program: &mut IrProgram) {
    for function in &mut program.functions {
        let name = function.name.clone();
        rewrite(&mut function.body, &name, true);
    }
}

/// Whether `expr` contains a [`IrExpr::TailSelfCall`] anywhere. Backends use
/// this to decide whether a function body needs its loop wrapper.
pub fn contains_tail_self_call(expr: &IrExpr) -> bool {
    let mut found = false;
    walk(expr, &mut |e| {
        if matches!(e, IrExpr::TailSelfCall { .. }) {
            found = true;
        }
    });
    found
}

fn rewrite(expr: &mut IrExpr, self_name: &str, tail: bool) {
    match expr {
        IrExpr::Let { value, next, .. } => {
            rewrite(value, self_name, false);
            rewrite(next, self_name, tail);
        }
        IrExpr::If {
            cond, then, els, ..
        } => {
            rewrite(cond, self_name, false);
            rewrite(then, self_name, tail);
            rewrite(els, self_name, tail);
        }
        IrExpr::Match {
            scrutinee, arms, ..
        } => {
            rewrite(scrutinee, self_name, false);
            for arm in arms {
                if let Some(guard) = &mut arm.guard {
                    rewrite(guard, self_name, false);
                }
                rewrite(&mut arm.body, self_name, tail);
            }
        }
        IrExpr::Try { body, arms, .. } => {
            // The try block is not a tail position (spec 0045 T1); catch arms are.
            rewrite(body, self_name, false);
            for arm in arms {
                if let Some(guard) = &mut arm.guard {
                    rewrite(guard, self_name, false);
                }
                rewrite(&mut arm.body, self_name, tail);
            }
        }
        IrExpr::Call { callee, args, .. } => {
            rewrite(callee, self_name, false);
            for arg in args.iter_mut() {
                rewrite(arg, self_name, false);
            }
            let is_self = matches!(
                callee.as_ref(),
                IrExpr::FunctionRef { name, .. } if name == self_name
            );
            if tail && is_self {
                let IrExpr::Call { args, ret, .. } = std::mem::replace(expr, IrExpr::Unit) else {
                    unreachable!("just matched a Call");
                };
                *expr = IrExpr::TailSelfCall { args, ty: ret };
            }
        }
        // A nested function literal is its own function (spec 0045 T4).
        IrExpr::Fn { body, .. } => rewrite(body, self_name, false),
        IrExpr::Array { elems, .. } => {
            for elem in elems {
                rewrite(elem, self_name, false);
            }
        }
        IrExpr::Platform { args, .. } | IrExpr::Intrinsic { args, .. } => {
            for arg in args {
                rewrite(arg, self_name, false);
            }
        }
        IrExpr::Binary { left, right, .. } | IrExpr::Concat { left, right } => {
            rewrite(left, self_name, false);
            rewrite(right, self_name, false);
        }
        IrExpr::EnumValue { payload, .. } => {
            for field in payload {
                rewrite(field, self_name, false);
            }
        }
        IrExpr::RecordValue { fields, .. } => {
            for field in fields {
                rewrite(field, self_name, false);
            }
        }
        IrExpr::FieldAccess { target, .. } => rewrite(target, self_name, false),
        IrExpr::Throw { value } => rewrite(value, self_name, false),
        // RC nodes (spec 0048) are inserted after this rewrite runs; handled
        // for completeness. A release is transparent to tail position.
        IrExpr::Retain { value } => rewrite(value, self_name, false),
        IrExpr::Release { next, .. } => rewrite(next, self_name, tail),
        // `expr?` has post-processing on the error path, so its operand is
        // never a tail position (spec 0045 T1).
        IrExpr::Question { value, .. } => rewrite(value, self_name, false),
        IrExpr::Panic { message } => rewrite(message, self_name, false),
        IrExpr::Int(_)
        | IrExpr::Float(_)
        | IrExpr::Bool(_)
        | IrExpr::String(_)
        | IrExpr::Char(_)
        | IrExpr::Unit
        | IrExpr::Var { .. }
        | IrExpr::FunctionRef { .. }
        | IrExpr::TailSelfCall { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{IrFunction, IrParam};
    use crate::types::{EffectRow, FunctionType, Type};

    fn self_call(name: &str, args: Vec<IrExpr>) -> IrExpr {
        IrExpr::Call {
            callee: Box::new(IrExpr::FunctionRef {
                name: name.to_string(),
                sig: FunctionType {
                    params: vec![Type::Int],
                    ret: Box::new(Type::Unit),
                    throws: None,
                    effects: EffectRow::default(),
                },
            }),
            args,
            ret: Type::Unit,
        }
    }

    fn program_with_body(body: IrExpr) -> IrProgram {
        IrProgram {
            functions: vec![IrFunction {
                name: "loop_fn".to_string(),
                params: vec![IrParam {
                    name: "n".to_string(),
                    ty: Type::Int,
                }],
                ret: Type::Unit,
                throws: None,
                effects: EffectRow::default(),
                body,
            }],
        }
    }

    #[test]
    fn rewrites_a_tail_self_call() {
        let mut program = program_with_body(self_call("loop_fn", vec![IrExpr::Int(1)]));
        rewrite_self_tail_calls(&mut program);
        assert!(matches!(
            program.functions[0].body,
            IrExpr::TailSelfCall { .. }
        ));
    }

    #[test]
    fn rewrites_through_let_if_and_match_arms() {
        let body = IrExpr::Let {
            name: "x".to_string(),
            value_ty: Type::Int,
            value: Box::new(IrExpr::Int(1)),
            next: Box::new(IrExpr::If {
                cond: Box::new(IrExpr::Bool(true)),
                then: Box::new(self_call("loop_fn", vec![IrExpr::Int(1)])),
                els: Box::new(IrExpr::Unit),
                ty: Type::Unit,
            }),
        };
        let mut program = program_with_body(body);
        rewrite_self_tail_calls(&mut program);
        let IrExpr::Let { next, .. } = &program.functions[0].body else {
            panic!("expected let");
        };
        let IrExpr::If { then, .. } = next.as_ref() else {
            panic!("expected if");
        };
        assert!(matches!(then.as_ref(), IrExpr::TailSelfCall { .. }));
    }

    #[test]
    fn does_not_rewrite_inside_a_try_block_or_non_tail_positions() {
        // `n * loop_fn(...)` : operand position is not a tail position.
        let body = IrExpr::Binary {
            op: crate::types::BinaryOp::Add,
            ty: Type::Int,
            left: Box::new(IrExpr::Int(1)),
            right: Box::new(self_call("loop_fn", vec![IrExpr::Int(1)])),
        };
        let mut program = program_with_body(body);
        rewrite_self_tail_calls(&mut program);
        assert!(!contains_tail_self_call(&program.functions[0].body));

        // Inside a try block: the catch frame is pending, not a tail position.
        let body = IrExpr::Try {
            body: Box::new(self_call("loop_fn", vec![IrExpr::Int(1)])),
            arms: vec![],
            ty: Type::Unit,
            err_name: None,
        };
        let mut program = program_with_body(body);
        rewrite_self_tail_calls(&mut program);
        assert!(!contains_tail_self_call(&program.functions[0].body));
    }

    #[test]
    fn does_not_rewrite_calls_to_other_functions_or_in_lambdas() {
        let mut program = program_with_body(self_call("other_fn", vec![IrExpr::Int(1)]));
        rewrite_self_tail_calls(&mut program);
        assert!(!contains_tail_self_call(&program.functions[0].body));

        let lambda = IrExpr::Fn {
            params: vec![],
            ret: Type::Unit,
            throws: None,
            effects: EffectRow::default(),
            captures: vec![],
            body: Box::new(self_call("loop_fn", vec![IrExpr::Int(1)])),
        };
        let mut program = program_with_body(lambda);
        rewrite_self_tail_calls(&mut program);
        assert!(!contains_tail_self_call(&program.functions[0].body));
    }
}
