//! Values that reach an unsupported feature (fibers). `Fiber.attempt` and the operations of
//! `Std.Cause` run on the current fiber, so they are supported (`emit::runtime::attempt`).

use super::*;

use emela_lir::Value;

pub(super) fn value_unsupported(value: &Value, found: &mut Found) {
    if let Value::Fiber { op, .. } = value
        && let Some(name) = fiber_name(*op)
        && !found.fibers.contains(&name)
    {
        found.fibers.push(name);
    }
}

/// The name of a concurrency operation that needs fibers, or `None` for one that does not.
fn fiber_name(op: emela_core::FiberOp) -> Option<&'static str> {
    use emela_core::FiberOp::*;
    Some(match op {
        Scope { .. } => "Fiber.scope",
        Fork => "Fiber.fork",
        Deferred => "Deferred",
        Join => "Fiber.join",
        Await { .. } => "Fiber.await",
        Interrupt => "Fiber.interrupt",
        Complete => "Deferred.complete",
        DeferredAwait => "Deferred.await",
        Poll => "Deferred.poll",
        Attempt { .. } | CauseKind { .. } | CauseToString | CauseRaise { .. } => return None,
    })
}
