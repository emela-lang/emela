use super::*;

pub(in crate::compiler::plan) mod builtins;
pub(in crate::compiler::plan) mod failures;
pub(in crate::compiler::plan) mod fibers;
pub(in crate::compiler::plan) mod primitive_operations;
pub(in crate::compiler::plan) mod runtime;
pub(in crate::compiler::plan) mod statements;
pub(in crate::compiler::plan) mod terms;
pub(in crate::compiler::plan) mod values;

use super::suspend::{Graph, LoopTarget, Node};
pub(in crate::compiler::plan) struct LoweringContext<'a> {
    pub(in crate::compiler::plan) graph: &'a mut Graph,
    pub(in crate::compiler::plan) function: &'a Function,
}
