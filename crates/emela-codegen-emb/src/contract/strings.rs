//! Resolved string storage and operation modes. No allocation analysis or C formatting.
use crate::contract::{
    ir::{FnId, VarId},
    operations::ResultOwnership,
};
use std::collections::{HashMap, HashSet};
pub(crate) struct StringBuffers {
    pub(crate) result_ownership: HashMap<VarId, ResultOwnership>,
    pub(crate) append_modes: HashMap<VarId, crate::contract::operations::AppendMode>,
    pub(crate) discarded_results: HashSet<VarId>,
    pub(crate) buffers: HashMap<VarId, (Option<usize>, u16)>,
    pub(crate) declarations: Vec<(String, usize)>,
    pub(crate) output_calls: HashSet<FnId>,
    pub(crate) returns_to_caller: bool,
    pub(crate) capacity: u16,
}
impl StringBuffers {
    pub(crate) fn has_string_output_call(&self, function: FnId) -> bool {
        self.output_calls.contains(&function)
    }
}
