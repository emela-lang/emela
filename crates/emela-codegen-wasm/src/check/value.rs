//! Values that reach an unsupported feature.

use super::*;

use emela_core::Builtin;
use emela_lir::{Callee, Function, Module, Value};

pub(super) fn value_unsupported(m: &Module, f: &Function, value: &Value, found: &mut Found) {
    match value {
        Value::Fiber { op, .. } => {
            let name = fiber_name(*op);
            if !found.fibers.contains(&name) {
                found.fibers.push(name);
            }
        }
        // A `suspend fn` operation called where nothing may suspend: its implementation runs in
        // place (spec 8.7), which needs the suspend machinery too.
        Value::Call {
            callee: Callee::Op { effect, op, .. },
            ..
        } if m.core.effects[*effect].ops[*op].suspend => {
            let effect = &m.core.effects[*effect];
            found.suspending(format!(
                "`{}.{}` (a `suspend fn` operation)",
                display_name(&effect.name),
                effect.ops[*op].name
            ));
        }
        Value::Closure(c) if c.suspends => {
            found.suspending(format!("a closure in `{}`", display_name(&f.name)));
        }
        Value::Builtin { op, .. } => {
            let message = match op {
                Builtin::FloatToString
                | Builtin::FloatParse
                | Builtin::FloatFloor
                | Builtin::FloatCeil
                | Builtin::FloatRound
                | Builtin::FloatTruncate => {
                    "`Float` operation, which target `wasm` does not support yet".to_owned()
                }
                _ => return,
            };
            found.report(WasmErrorKind::Unsupported, message);
        }
        _ => {}
    }
}

fn fiber_name(op: emela_core::FiberOp) -> &'static str {
    use emela_core::FiberOp::*;
    match op {
        Scope { .. } => "Fiber.scope",
        Fork => "Fiber.fork",
        Deferred => "Deferred",
        Join => "Fiber.join",
        Await { .. } => "Fiber.await",
        Interrupt => "Fiber.interrupt",
        Attempt { .. } => "Fiber.attempt",
        Complete => "Deferred.complete",
        DeferredAwait => "Deferred.await",
        Poll => "Deferred.poll",
        CauseKind { .. } => "Cause.kind",
        CauseToString => "Cause.to_string",
        CauseRaise => "Cause.raise",
    }
}
