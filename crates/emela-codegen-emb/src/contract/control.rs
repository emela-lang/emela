//! Final structured-control and frame decisions, without planner scratch state.
use crate::contract::{
    ir::{CaseKey, Function, LabelId, Operand, Stmt, StmtId, SwitchOn, VarId},
    strings::StringBuffers,
};
use std::collections::{HashMap, HashSet};
pub(crate) enum ControlNode {
    Statement(StmtId, usize),
    Assign(Vec<(VarId, Operand)>, usize),
    Branch(Operand, usize, usize),
    Switch(SwitchOn, Vec<(CaseKey, usize)>, Option<usize>),
    Return(Option<Operand>),
    SetError(Operand, usize, usize),
    CheckError {
        normal: usize,
        failed: usize,
        slot: usize,
    },
    Catch(VarId, usize, usize),
    Fail(usize),
    Defect(Operand),
    Unreachable,
    Protect {
        cleanup: usize,
        body: usize,
    },
    Leave {
        keep: usize,
        next: usize,
    },
    CleanupEnd,
}
pub(crate) struct ControlGraph {
    pub(crate) nodes: Vec<ControlNode>,
    pub(crate) max_depth: usize,
    pub(crate) max_error_depth: usize,
    pub(crate) entry: usize,
}
pub(crate) struct SuspensionPlan<'a> {
    pub(crate) function: &'a Function,
    pub(crate) graph: ControlGraph,
    pub(crate) active: HashSet<VarId>,
    pub(crate) strings: StringBuffers,
    pub(crate) has_result: bool,
    pub(crate) has_failure: bool,
    pub(crate) error_variable: VarId,
    pub(crate) error_label: LabelId,
    pub(crate) statements: HashMap<StmtId, Stmt>,
}
