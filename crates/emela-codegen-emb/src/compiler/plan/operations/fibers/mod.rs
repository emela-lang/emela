use super::*;

pub(in crate::compiler::plan) fn string_behavior(
    op: crate::contract::ir::FiberOp,
) -> StringBehavior {
    match op {
        crate::contract::ir::FiberOp::CauseToString => cause_to_string::string_behavior(),
        crate::contract::ir::FiberOp::Scope { .. }
        | crate::contract::ir::FiberOp::Fork
        | crate::contract::ir::FiberOp::Deferred
        | crate::contract::ir::FiberOp::Join
        | crate::contract::ir::FiberOp::Await { .. }
        | crate::contract::ir::FiberOp::Interrupt
        | crate::contract::ir::FiberOp::Attempt { .. }
        | crate::contract::ir::FiberOp::Complete
        | crate::contract::ir::FiberOp::DeferredAwait
        | crate::contract::ir::FiberOp::Poll
        | crate::contract::ir::FiberOp::CauseKind { .. }
        | crate::contract::ir::FiberOp::CauseRaise => StringBehavior::default(),
    }
}

pub(in crate::compiler::plan) mod cause_to_string;
