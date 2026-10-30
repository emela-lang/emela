//! Complete ABI and resource decisions consumed by C rendering.
use crate::contract::{
    abi::CLayoutPlan,
    auxiliary::{ConstBridgePlan, DerivationPlan},
    bindings::ExternalBindings,
    ir::{FnId, TypeId},
    lowered::{ConstOutcome, LoweredModule},
    runtime::RuntimeRequirements,
    strings::StringBuffers,
};
use std::collections::{HashMap, HashSet};
pub(crate) struct PlannedModule<'a> {
    pub(crate) metadata: crate::contract::artifact::Metadata,
    pub(crate) aliases: Vec<(String, String)>,
    pub(crate) const_emergency_bytes: usize,
    pub(crate) options: crate::contract::Options,
    pub(crate) services: HashMap<FnId, crate::contract::services::Operation>,
    pub(crate) suspensions: HashMap<FnId, crate::contract::control::SuspensionPlan<'a>>,
    pub(crate) input: &'a LoweredModule,
    pub(crate) layouts: CLayoutPlan<'a>,
    pub(crate) external_bindings: ExternalBindings,
    pub(crate) const_bridges: ConstBridgePlan,
    pub(crate) derived_plan: DerivationPlan,
    pub(crate) emitted_functions: HashSet<FnId>,
    pub(crate) heap_functions: HashSet<FnId>,
    pub(crate) string_output_functions: HashSet<FnId>,
    pub(crate) const_outcomes: HashMap<FnId, ConstOutcome>,
    pub(crate) names: HashMap<FnId, String>,
    pub(crate) public_names: HashMap<FnId, String>,
    pub(crate) strings: HashMap<FnId, StringBuffers>,
    pub(crate) function_ref_adapters: Vec<FunctionRefAdapter>,
    pub(crate) main_entry: Option<FnId>,
    pub(crate) has_scheduler: bool,
    pub(crate) has_tasks: bool,
    pub(crate) has_failing_tasks: bool,
    pub(crate) task_value_types: Vec<TypeId>,
    pub(crate) runtime: RuntimeRequirements,
}
pub(crate) struct FunctionRefAdapter {
    pub(crate) target: FnId,
    pub(crate) function_type: TypeId,
    pub(crate) takes_output: bool,
    pub(crate) capacity: u16,
}
