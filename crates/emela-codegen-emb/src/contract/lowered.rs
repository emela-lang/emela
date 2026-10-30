//! Output of upstream adaptation. All identifiers refer to the owned backend program.
use crate::contract::ir::{Const, FnId, Function, LabelId, Program, TypeId, VarId};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub(crate) struct LoweredModule {
    pub(crate) lowered: Program,
    pub(crate) suspensions: HashMap<FnId, SuspensionInput>,
    pub(crate) closures: ClosureMetadata,
    pub(crate) const_groups: Vec<ConstGroup>,
    pub(crate) runtime_consts: HashSet<FnId>,
    pub(crate) deferred_pool: bool,
    pub(crate) emitted_functions: HashSet<FnId>,
    pub(crate) discarded_results: HashMap<FnId, HashSet<VarId>>,
    pub(crate) const_outcomes: HashMap<FnId, Result<ConstOutcome, &'static str>>,
}
#[derive(Debug, Clone, Default)]
pub(crate) struct ClosureMetadata {
    pub(crate) functions: HashMap<FnId, usize>,
    pub(crate) families: HashMap<FnId, (FnId, usize)>,
    pub(crate) environments: Vec<ClosureEnvironment>,
}
#[derive(Debug, Clone)]
pub(crate) struct ClosureEnvironment {
    pub(crate) parent: FnId,
    pub(crate) function_type: TypeId,
    pub(crate) captures: Vec<CaptureBinding>,
}
#[derive(Debug, Clone)]
pub(crate) struct CaptureBinding {
    pub(crate) source: VarId,
    pub(crate) local: VarId,
    pub(crate) ty: TypeId,
}
#[derive(Debug, Clone)]
pub(crate) struct ConstGroup {
    pub(crate) canonical: FnId,
    pub(crate) specializations: Vec<FnId>,
}
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ConstOutcome {
    Value(Const),
    Defect { kind: &'static str, message: String },
    SelfReference { message: String },
}

#[derive(Debug, Clone)]
pub(crate) struct SuspensionInput {
    pub(crate) function: Function,
    pub(crate) error_variable: VarId,
    pub(crate) error_label: LabelId,
}
