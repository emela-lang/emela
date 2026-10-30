//! Operation facts passed from planning to C formatting.
use crate::contract::ir::{Const, FnId, Operand, VarId};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum StringOutput {
    #[default]
    None,
    Buffer,
    Call(FnId),
}

#[derive(Clone, Copy, Default)]
pub(crate) enum BufferResult {
    #[default]
    String,
    OptionalString,
    Any,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum InputUse {
    ReadOnly,
    #[default]
    Escapes,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct StringBehavior {
    pub output: StringOutput,
    pub result: BufferResult,
    pub reusable_input: Option<VarId>,
    #[doc = " Whether input references can outlive this operation."]
    pub input_use: InputUse,
}

pub(crate) struct StringBounds<'a> {
    pub values: &'a HashMap<VarId, usize>,
    pub capacity: u16,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResultOwnership {
    Owned,
    Borrowed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppendMode {
    Constant,
    AliasInput,
    Buffered,
}

impl StringBehavior {
    pub(crate) fn needs_output(self, outputs: &HashSet<FnId>) -> bool {
        match self.output {
            StringOutput::None => false,
            StringOutput::Buffer => true,
            StringOutput::Call(id) => outputs.contains(&id),
        }
    }
}
impl StringBounds<'_> {
    pub(crate) fn operand(&self, operand: &Operand) -> usize {
        match operand {
            Operand::Const(Const::String(value)) => value.len(),
            Operand::Var(variable) => self
                .values
                .get(variable)
                .copied()
                .unwrap_or(usize::from(self.capacity)),
            _ => usize::from(self.capacity),
        }
    }
}
