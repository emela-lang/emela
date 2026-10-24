//! Values that reach an unsupported feature (fibers).

use super::*;

use emela_lir::Value;

pub(super) fn value_unsupported(value: &Value, found: &mut Found) {
    if let Value::Fiber { op, .. } = value {
        let name = fiber_name(*op);
        if !found.fibers.contains(&name) {
            found.fibers.push(name);
        }
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
