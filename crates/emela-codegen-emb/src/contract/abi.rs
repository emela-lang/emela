//! Immutable C ABI data. Preparation and emission are implemented by stage-local traits.
use crate::contract::ir::{EffectId, EnumId, FnId, Repr, TypeId, TypeTable, VarId};
use std::collections::{HashMap, HashSet};

pub(crate) struct TypeShapes<'a> {
    pub(crate) core: &'a crate::contract::ir::Definitions,
    pub(crate) types: TypeTable,
    pub(crate) aggregates: Vec<AggregateLayout>,
    pub(crate) aggregate_ids: HashMap<TypeId, usize>,
    pub(crate) recursive_edges: HashSet<(TypeId, usize, usize)>,
    pub(crate) expanding_enums: HashSet<EnumId>,
    pub(crate) recursive_box_types: Vec<TypeId>,
    pub(crate) recursive_box_ids: HashMap<TypeId, usize>,
    pub(crate) list_types: Vec<TypeId>,
    pub(crate) list_ids: HashMap<TypeId, usize>,
    pub(crate) function_types: Vec<TypeId>,
    pub(crate) closure_function_types: HashSet<TypeId>,
    pub(crate) cell_types: Vec<TypeId>,
    pub(crate) cell_ids: HashMap<TypeId, usize>,
    pub(crate) function_adapters: Vec<FunctionAdapterLayout>,
    pub(crate) function_adapter_ids: HashMap<(TypeId, TypeId), usize>,
    pub(crate) handler_layouts: Vec<HandlerLayout>,
    pub(crate) handler_layout_ids: HashMap<EffectId, usize>,
    pub(crate) any_box_types: Vec<TypeId>,
    pub(crate) any_box_ids: HashMap<TypeId, usize>,
}

#[derive(Default)]
pub(crate) struct FunctionAbis {
    pub(crate) discarded_results: HashMap<FnId, HashSet<VarId>>,
    pub(crate) function_results: HashMap<FnId, TypeId>,
    pub(crate) closure_functions: HashMap<FnId, usize>,
    pub(crate) closure_environments: Vec<crate::contract::lowered::ClosureEnvironment>,
    pub(crate) owned_string_returns: HashSet<String>,
    pub(crate) runtime_consts: HashSet<FnId>,
    pub(crate) owned_aggregate_returns: HashSet<String>,
    pub(crate) error_shows: Vec<(EnumId, FnId)>,
}

#[derive(Default)]
pub(crate) struct RuntimeValues {
    pub(crate) resume_types: Vec<TypeId>,
    pub(crate) has_resumes: bool,
    pub(crate) has_unwind: bool,
    pub(crate) has_scopes: bool,
    pub(crate) deferred_pool: bool,
    pub(crate) deferred_complete_types: Vec<TypeId>,
    pub(crate) deferred_read_types: Vec<TypeId>,
    pub(crate) deferred_poll_types: Vec<TypeId>,
    pub(crate) deferred_await_types: Vec<TypeId>,
    pub(crate) child_join_types: Vec<TypeId>,
    pub(crate) child_await_types: Vec<TypeId>,
    pub(crate) child_interrupts: bool,
    pub(crate) has_causes: bool,
    pub(crate) has_cause_raise: bool,
    pub(crate) attempt_types: Vec<(TypeId, TypeId)>,
    pub(crate) fork_types: Vec<TypeId>,
    pub(crate) evidence_nodes_required: bool,
    pub(crate) any_abi: bool,
    pub(crate) any_box_required: bool,
    pub(crate) error_types: Vec<(EnumId, TypeId)>,
    pub(crate) has_error_abi: bool,
    pub(crate) managed_error_payloads: bool,
}

/// Mutable ABI data used only while planning.
pub(crate) struct LayoutBuilder<'a> {
    pub(crate) shapes: TypeShapes<'a>,
    pub(crate) functions: FunctionAbis,
    pub(crate) runtime: RuntimeValues,
    pub(crate) properties: Option<LayoutProperties>,
}

/// A layout whose cached properties have been finalized for emission.
pub(crate) struct CLayoutPlan<'a> {
    builder: LayoutBuilder<'a>,
}

impl<'a> LayoutBuilder<'a> {
    pub(crate) fn finish(mut self, properties: LayoutProperties) -> CLayoutPlan<'a> {
        assert!(self.properties.is_none());
        self.properties = Some(properties);
        CLayoutPlan { builder: self }
    }
}

impl<'a> std::ops::Deref for CLayoutPlan<'a> {
    type Target = LayoutBuilder<'a>;

    fn deref(&self) -> &Self::Target {
        &self.builder
    }
}
pub(crate) struct LayoutProperties {
    pub(crate) types: Vec<TypeProperties>,
    pub(crate) cell_cycles: bool,
    pub(crate) error_declarations: HashSet<usize>,
}
pub(crate) struct TypeProperties {
    pub(crate) representation: Repr,
    pub(crate) owned: bool,
    pub(crate) heap: bool,
    pub(crate) borrowed_views: bool,
    pub(crate) cell_cycles: bool,
}

#[derive(Debug)]
pub(crate) struct AggregateLayout {
    pub(crate) ty: TypeId,
    pub(crate) kind: AggregateKind,
}

#[derive(Debug)]
pub(crate) enum AggregateKind {
    Tuple(Vec<TypeId>),
    Enum(Vec<Vec<TypeId>>),
}

pub(crate) struct HandlerLayout {
    pub(crate) effect: EffectId,
    pub(crate) operations: Vec<TypeId>,
}

pub(crate) struct FunctionAdapterLayout {
    pub(crate) source: TypeId,
    pub(crate) destination: TypeId,
}
