use super::*;
pub(in crate::compiler::emit) mod builtin_utilities;
pub(in crate::compiler::emit) mod builtins;
pub(in crate::compiler::emit) mod statements;
pub(in crate::compiler::emit) mod values;
pub(in crate::compiler::emit) struct Context<'a, 'ir> {
    pub(in crate::compiler::emit) layouts: &'a TypeLayouts<'ir>,
    pub(in crate::compiler::emit) function: &'a Function,
    pub(in crate::compiler::emit) writer: &'a mut Writer,
    pub(in crate::compiler::emit) names: &'a HashMap<FnId, String>,
    pub(in crate::compiler::emit) string_buffers: &'a StringBufferPlan,
    pub(in crate::compiler::emit) loops: &'a mut Vec<Vec<VarId>>,
    pub(in crate::compiler::emit) blocks: &'a mut Vec<BlockTarget>,
    pub(in crate::compiler::emit) protects: &'a mut Vec<ProtectTarget>,
    pub(in crate::compiler::emit) next_temp: &'a mut usize,
    pub(in crate::compiler::emit) next_label: &'a mut usize,
}
pub(in crate::compiler::emit) struct BuiltinContext<'a, 'ir> {
    pub(in crate::compiler::emit) layouts: &'a TypeLayouts<'ir>,
    pub(in crate::compiler::emit) function: &'a Function,
    pub(in crate::compiler::emit) string_buffers: &'a StringBufferPlan,
    pub(in crate::compiler::emit) writer: &'a mut Writer,
}
use builtin_utilities::ResultType;

type Emission = Result<(), Vec<Error>>;

pub(in crate::compiler::emit) mod constants;

pub(in crate::compiler::emit) mod operands;

pub(in crate::compiler::emit) mod primitive_operations;

pub(in crate::compiler::emit) mod primitive_types;

pub(in crate::compiler::emit) mod terms;

pub(in crate::compiler::emit) mod failures;

pub(in crate::compiler::emit) mod runtime;

pub(in crate::compiler::emit) mod case_keys;
pub(in crate::compiler::emit) mod fibers;
pub(in crate::compiler::emit) mod switches;
use crate::contract::control::ControlNode as Node;
pub(in crate::compiler::emit) struct SuspendContext<'a, 'ir> {
    pub(in crate::compiler::emit) flow: &'a crate::contract::ir::Program,
    pub(in crate::compiler::emit) layouts: &'a TypeLayouts<'ir>,
    pub(in crate::compiler::emit) function: &'a Function,
    pub(in crate::compiler::emit) writer: &'a mut Writer,
    pub(in crate::compiler::emit) names: &'a HashMap<FnId, String>,
    pub(in crate::compiler::emit) strings: &'a StringBufferPlan,
    pub(in crate::compiler::emit) next_temp: &'a mut usize,
    pub(in crate::compiler::emit) next: usize,
    pub(in crate::compiler::emit) index: usize,
    pub(in crate::compiler::emit) failing: bool,
    pub(in crate::compiler::emit) call_error_var: VarId,
    pub(in crate::compiler::emit) call_error_label: LabelId,
}

use super::suspend::{emit_operation, emit_sync_statement, prepare_result};

pub(in crate::compiler::emit) mod control_utilities;

use super::suspend::retain_frame_value;
