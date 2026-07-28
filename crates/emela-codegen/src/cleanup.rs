//! Expansion of [`IrExpr::Cleanup`] — deterministic resource cleanup (spec 0056).
//!
//! A `defer` block item lowers to one `cleanup { body, action }` node, whose
//! meaning is "evaluate `body`, and evaluate `action` on every exit from it".
//! This pass rewrites it to the ordinary IR nodes, one copy of `action` per exit
//! path (spec 0056's compilation note; the same scheme Zig uses):
//!
//! ```text
//! let $dfvN = try { body' } catch { $dfeN -> action; throw $dfeN }
//! action
//! $dfvN
//! ```
//!
//! Two details make this correct rather than merely plausible.
//!
//! *Self tail calls.* A jump out of the scope (spec 0045) is an exit too, and
//! Emela has no loop construct — every iteration is a self tail call, so a
//! `defer` that did not fire on this path would never fire inside a loop at all.
//! But backends disagree on what a self tail call *is*: WebAssembly emits a
//! `br` that skips the value path, while JavaScript returns a trampoline marker
//! that flows out through it. Hence [`TailMode`]: under `Jump` each self tail
//! call gets its own copy of the action, under `Value` the value path already
//! covers it and a copy would run the action twice per iteration.
//!
//! *Order.* Expansion is bottom-up, so the action an outer `cleanup` inserts
//! before a self tail call lands *after* the one an inner `cleanup` inserted —
//! innermost first, as spec 0056 D6 requires.
//!
//! The pass runs before [`crate::rc::insert_rc_ops`]. Placing each action in the
//! value position of a `let` is what orders it before the releases of the
//! bindings it reads: the RC pass descends into a `let`'s continuation, never
//! its value, so `release` lands between the action and the exit.

use crate::ir::{IrArm, IrExpr, IrPattern, IrProgram};
use crate::ir_walk::{walk_children, walk_children_mut};
use crate::types::Type;

/// How a backend implements a self tail call (spec 0045), which decides whether
/// a jump skips the value path of an enclosing scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TailMode {
    /// A jump back to the function head (WebAssembly's `br`). It skips the
    /// value path, so every self tail call needs its own copy of the action.
    Jump,
    /// A trampoline marker returned as an ordinary value (JavaScript). It flows
    /// out through the value path, which already runs the action.
    Value,
}

/// Rewrites every [`IrExpr::Cleanup`] in `program` to ordinary IR nodes.
pub fn expand_cleanups(program: &mut IrProgram, mode: TailMode) {
    for function in &mut program.functions {
        let mut fresh = Fresh(0);
        expand(&mut function.body, mode, &mut fresh);
    }
}

/// Bottom-up: children first, so an inner `cleanup` is already expanded when
/// the one enclosing it inserts its own action before a shared exit.
fn expand(expr: &mut IrExpr, mode: TailMode, fresh: &mut Fresh) {
    walk_children_mut(expr, &mut |child| expand(child, mode, fresh));
    if matches!(expr, IrExpr::Cleanup { .. }) {
        let IrExpr::Cleanup { body, action } = std::mem::replace(expr, IrExpr::Unit) else {
            unreachable!("just matched a Cleanup");
        };
        *expr = build(*body, *action, mode, fresh);
    }
}

fn build(body: IrExpr, action: IrExpr, mode: TailMode, fresh: &mut Fresh) -> IrExpr {
    let ty = body.ty();
    let mut body = body;
    if mode == TailMode::Jump {
        before_tail_self_calls(&mut body, &action, fresh);
    }
    // The error path. Only guard a body that can actually raise past this
    // point: an unconditional `try` in a non-throwing function would put a
    // rethrow on a boundary that has no error channel to report it on.
    if let Some(err_ty) = escaping_error_ty(&body) {
        let err = fresh.next("e");
        let rethrow = seq(
            action.clone(),
            IrExpr::Throw {
                value: Box::new(IrExpr::Var {
                    name: err.clone(),
                    ty: err_ty.clone(),
                }),
            },
            fresh,
        );
        body = IrExpr::Try {
            body: Box::new(body),
            arms: vec![IrArm {
                pattern: IrPattern::Wildcard {
                    binding: Some((err, err_ty)),
                },
                guard: None,
                body: rethrow,
            }],
            ty: ty.clone(),
            // The RC pass binds the caught error itself (spec 0048 A7).
            err_name: None,
        };
    }
    // The value path: evaluate the body first, then the action, then yield.
    let value = fresh.next("v");
    IrExpr::Let {
        name: value.clone(),
        value_ty: ty.clone(),
        value: Box::new(body),
        next: Box::new(seq(action, IrExpr::Var { name: value, ty }, fresh)),
    }
}

/// Inserts `action` immediately before every self tail call that jumps out of
/// this scope. A nested function literal is its own function (spec 0045 T4);
/// the jump's argument expressions are evaluated before it jumps, so they need
/// no copy.
fn before_tail_self_calls(expr: &mut IrExpr, action: &IrExpr, fresh: &mut Fresh) {
    match expr {
        IrExpr::TailSelfCall { .. } => {
            let jump = std::mem::replace(expr, IrExpr::Unit);
            *expr = seq(action.clone(), jump, fresh);
        }
        IrExpr::Fn { .. } => {}
        other => walk_children_mut(other, &mut |child| {
            before_tail_self_calls(child, action, fresh)
        }),
    }
}

/// The type of an error that can escape `expr` on the error channel (spec
/// 0011), if one can.
///
/// A `try` catches everything its body raises — catch arms are exhaustive — so
/// only a rethrow from an arm escapes it. A nested function literal reports at
/// its own boundary. `panic` is not the error channel.
fn escaping_error_ty(expr: &IrExpr) -> Option<Type> {
    match expr {
        IrExpr::Throw { value } => Some(value.ty()),
        IrExpr::Question { value, .. } => escaping_error_ty(value),
        IrExpr::Call { callee, args, .. } => throws_of(&callee.ty())
            .or_else(|| escaping_error_ty(callee))
            .or_else(|| args.iter().find_map(escaping_error_ty)),
        IrExpr::Platform { throws, args, .. } => throws
            .clone()
            .or_else(|| args.iter().find_map(escaping_error_ty)),
        IrExpr::Try { arms, .. } => arms.iter().find_map(|arm| {
            arm.guard
                .as_ref()
                .and_then(escaping_error_ty)
                .or_else(|| escaping_error_ty(&arm.body))
        }),
        IrExpr::Fn { .. } => None,
        other => {
            let mut found = None;
            walk_children(other, &mut |child| {
                if found.is_none() {
                    found = escaping_error_ty(child);
                }
            });
            found
        }
    }
}

fn throws_of(ty: &Type) -> Option<Type> {
    match ty {
        Type::Function(function) => function.throws.as_deref().cloned(),
        _ => None,
    }
}

/// `first; second` — the IR has no statement sequence, so a discarded value is
/// a `let` to a temporary nobody reads. Keeping the action in a value position
/// is also what orders it before the RC pass's releases (see the module doc).
fn seq(first: IrExpr, second: IrExpr, fresh: &mut Fresh) -> IrExpr {
    IrExpr::Let {
        name: fresh.next("a"),
        value_ty: first.ty(),
        value: Box::new(first),
        next: Box::new(second),
    }
}

/// Fresh names for this pass. `$` cannot appear in an identifier, and the tag
/// keeps them apart from the RC pass's `$rc` and lowering's `$stmt`.
struct Fresh(u32);

impl Fresh {
    fn next(&mut self, tag: &str) -> String {
        let n = self.0;
        self.0 += 1;
        format!("$df{tag}{n}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{IrFunction, IrParam};
    use crate::ir_walk::walk;
    use crate::types::{EffectRow, FunctionType, Type};

    /// `Io.print(tag)` stands in for a finalizer: a platform call is visible in
    /// the expanded tree and, being infallible, models spec 0056 D8.
    fn action(tag: &str) -> IrExpr {
        IrExpr::Platform {
            name: "io.write_stdout".to_string(),
            args: vec![IrExpr::String(tag.to_string())],
            ret: Type::Unit,
            throws: None,
        }
    }

    fn err_ty() -> Type {
        Type::Enum("E".to_string(), Vec::new())
    }

    /// A call to a function that throws `E`.
    fn throwing_call() -> IrExpr {
        IrExpr::Call {
            callee: Box::new(IrExpr::FunctionRef {
                name: "may_fail".to_string(),
                sig: FunctionType {
                    params: Vec::new(),
                    ret: Box::new(Type::Int),
                    throws: Some(Box::new(err_ty())),
                    effects: EffectRow::default(),
                },
            }),
            args: Vec::new(),
            ret: Type::Int,
        }
    }

    fn cleanup(body: IrExpr, tag: &str) -> IrExpr {
        IrExpr::Cleanup {
            body: Box::new(body),
            action: Box::new(action(tag)),
        }
    }

    fn expanded(body: IrExpr, mode: TailMode) -> IrExpr {
        let mut program = IrProgram {
            functions: vec![IrFunction {
                name: "f".to_string(),
                params: vec![IrParam {
                    name: "n".to_string(),
                    ty: Type::Int,
                }],
                ret: Type::Int,
                throws: None,
                effects: EffectRow::default(),
                body,
            }],
        };
        expand_cleanups(&mut program, mode);
        program.functions.pop().expect("one function").body
    }

    /// The action tags reached along `expr`, in evaluation order.
    fn tags(expr: &IrExpr) -> Vec<String> {
        let mut out = Vec::new();
        walk(expr, &mut |e| {
            if let IrExpr::Platform { name, args, .. } = e
                && name == "io.write_stdout"
                && let Some(IrExpr::String(tag)) = args.first()
            {
                out.push(tag.clone());
            }
        });
        out
    }

    fn count_tries(expr: &IrExpr) -> usize {
        let mut n = 0;
        walk(expr, &mut |e| {
            if matches!(e, IrExpr::Try { .. }) {
                n += 1;
            }
        });
        n
    }

    #[test]
    fn the_value_path_runs_the_action_after_the_body() {
        let expanded = expanded(cleanup(IrExpr::Int(1), "a"), TailMode::Jump);
        // `let $dfv = 1; let $dfa = action; $dfv`
        let IrExpr::Let { value, next, .. } = &expanded else {
            panic!("expected a let, got {expanded:?}");
        };
        assert!(matches!(value.as_ref(), IrExpr::Int(1)));
        let IrExpr::Let { value, next, .. } = next.as_ref() else {
            panic!("expected the action's let");
        };
        assert!(matches!(value.as_ref(), IrExpr::Platform { .. }));
        assert!(matches!(next.as_ref(), IrExpr::Var { .. }));
    }

    #[test]
    fn a_body_that_cannot_raise_gets_no_try() {
        let expanded = expanded(cleanup(IrExpr::Int(1), "a"), TailMode::Jump);
        assert_eq!(count_tries(&expanded), 0);
        assert_eq!(tags(&expanded), vec!["a"]);
    }

    #[test]
    fn the_error_path_rethrows_after_the_action() {
        let body = IrExpr::Question {
            value: Box::new(throwing_call()),
            ty: Type::Int,
        };
        let expanded = expanded(cleanup(body, "a"), TailMode::Jump);
        assert_eq!(count_tries(&expanded), 1);
        // One copy on the value path and one on the error path.
        assert_eq!(tags(&expanded), vec!["a", "a"]);

        let mut arms = None;
        walk(&expanded, &mut |e| {
            if let IrExpr::Try { arms: a, .. } = e {
                arms = Some(a.clone());
            }
        });
        let arms = arms.expect("a try");
        let [arm] = arms.as_slice() else {
            panic!("expected one catch arm");
        };
        let IrPattern::Wildcard {
            binding: Some((name, _)),
        } = &arm.pattern
        else {
            panic!("expected a catch-all binding");
        };
        // The arm is `action; throw <the caught error>`.
        let IrExpr::Let { value, next, .. } = &arm.body else {
            panic!("expected the action's let");
        };
        assert!(matches!(value.as_ref(), IrExpr::Platform { .. }));
        let IrExpr::Throw { value } = next.as_ref() else {
            panic!("expected a rethrow");
        };
        assert!(matches!(value.as_ref(), IrExpr::Var { name: n, .. } if n == name));
    }

    #[test]
    fn an_inner_try_that_rethrows_still_gets_protection() {
        // `try { may_fail()? } catch { e -> throw e }` raises past this point
        // even though its own body's raise is caught.
        let inner = IrExpr::Try {
            body: Box::new(IrExpr::Question {
                value: Box::new(throwing_call()),
                ty: Type::Int,
            }),
            arms: vec![IrArm {
                pattern: IrPattern::Wildcard {
                    binding: Some(("e".to_string(), err_ty())),
                },
                guard: None,
                body: IrExpr::Throw {
                    value: Box::new(IrExpr::Var {
                        name: "e".to_string(),
                        ty: err_ty(),
                    }),
                },
            }],
            ty: Type::Int,
            err_name: None,
        };
        let expanded = expanded(cleanup(inner, "a"), TailMode::Jump);
        assert_eq!(count_tries(&expanded), 2, "the guard try must be added");
    }

    #[test]
    fn an_inner_try_that_handles_everything_needs_no_guard() {
        let inner = IrExpr::Try {
            body: Box::new(IrExpr::Question {
                value: Box::new(throwing_call()),
                ty: Type::Int,
            }),
            arms: vec![IrArm {
                pattern: IrPattern::Wildcard { binding: None },
                guard: None,
                body: IrExpr::Int(0),
            }],
            ty: Type::Int,
            err_name: None,
        };
        let expanded = expanded(cleanup(inner, "a"), TailMode::Jump);
        assert_eq!(count_tries(&expanded), 1, "only the body's own try");
    }

    #[test]
    fn jump_mode_inserts_the_action_before_a_self_tail_call() {
        let body = IrExpr::TailSelfCall {
            args: vec![IrExpr::Int(1)],
            ty: Type::Int,
        };
        let expanded = expanded(cleanup(body, "a"), TailMode::Jump);
        // The jump copy, then the value-path copy that the jump skips.
        assert_eq!(tags(&expanded), vec!["a", "a"]);

        let mut before_jump = false;
        walk(&expanded, &mut |e| {
            if let IrExpr::Let { value, next, .. } = e
                && matches!(value.as_ref(), IrExpr::Platform { .. })
                && matches!(next.as_ref(), IrExpr::TailSelfCall { .. })
            {
                before_jump = true;
            }
        });
        assert!(before_jump, "the action must sit right before the jump");
    }

    #[test]
    fn value_mode_leaves_self_tail_calls_alone() {
        let body = IrExpr::TailSelfCall {
            args: vec![IrExpr::Int(1)],
            ty: Type::Int,
        };
        let expanded = expanded(cleanup(body, "a"), TailMode::Value);
        // The marker flows out through the value path, which runs the action.
        assert_eq!(tags(&expanded), vec!["a"]);
    }

    fn action_tag(expr: &IrExpr) -> Option<String> {
        match expr {
            IrExpr::Platform { name, args, .. } if name == "io.write_stdout" => {
                match args.first() {
                    Some(IrExpr::String(tag)) => Some(tag.clone()),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// The action tags evaluated on the way to the self tail call, in order. A
    /// jump inside a `let`'s value means its continuation never runs, so the
    /// path stops there.
    fn path_to_jump(expr: &IrExpr) -> Option<Vec<String>> {
        match expr {
            IrExpr::TailSelfCall { .. } => Some(Vec::new()),
            IrExpr::Let { value, next, .. } => {
                if let Some(inside) = path_to_jump(value) {
                    return Some(inside);
                }
                let mut rest = path_to_jump(next)?;
                if let Some(tag) = action_tag(value) {
                    rest.insert(0, tag);
                }
                Some(rest)
            }
            IrExpr::Try { body, .. } => path_to_jump(body),
            _ => None,
        }
    }

    /// The action tags evaluated on the way to the scope's value, in order.
    fn path_to_value(expr: &IrExpr) -> Vec<String> {
        match expr {
            IrExpr::Let { value, next, .. } => {
                let mut path = path_to_value(value);
                if let Some(tag) = action_tag(value) {
                    path.push(tag);
                }
                path.extend(path_to_value(next));
                path
            }
            IrExpr::Try { body, .. } => path_to_value(body),
            _ => Vec::new(),
        }
    }

    #[test]
    fn nested_cleanups_run_inside_out_before_a_jump() {
        // `defer a; defer b; <jump>` — b is registered later, so it runs first.
        let inner = cleanup(
            IrExpr::TailSelfCall {
                args: vec![IrExpr::Int(1)],
                ty: Type::Int,
            },
            "b",
        );
        let expanded = expanded(cleanup(inner, "a"), TailMode::Jump);
        assert_eq!(
            path_to_jump(&expanded),
            Some(vec!["b".to_string(), "a".to_string()])
        );
    }

    #[test]
    fn nested_cleanups_run_inside_out_on_the_value_path() {
        let inner = cleanup(IrExpr::Int(1), "b");
        let expanded = expanded(cleanup(inner, "a"), TailMode::Jump);
        assert_eq!(path_to_value(&expanded), vec!["b", "a"]);
    }

    #[test]
    fn a_lambda_body_is_a_separate_function() {
        let lambda = IrExpr::Fn {
            params: Vec::new(),
            ret: Type::Int,
            throws: None,
            effects: EffectRow::default(),
            captures: Vec::new(),
            body: Box::new(IrExpr::TailSelfCall {
                args: vec![IrExpr::Int(1)],
                ty: Type::Int,
            }),
        };
        let body = IrExpr::Let {
            name: "g".to_string(),
            value_ty: Type::Int,
            value: Box::new(lambda),
            next: Box::new(IrExpr::Int(0)),
        };
        let expanded = expanded(cleanup(body, "a"), TailMode::Jump);
        // Only the value path's copy: the lambda's jump is not our exit.
        assert_eq!(tags(&expanded), vec!["a"]);
    }
}
