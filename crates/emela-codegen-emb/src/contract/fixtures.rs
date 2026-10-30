//! Hand-authored boundary fixtures. They do not invoke any compiler stage.
use crate::contract::{
    Options,
    abi::{FunctionAbis, LayoutBuilder, LayoutProperties, RuntimeValues, TypeShapes},
    ir::{Definitions, Program},
    lowered::LoweredModule,
    planned::PlannedModule,
};
pub(crate) fn lowered() -> LoweredModule {
    LoweredModule {
        lowered: Program {
            core: Definitions::default(),
            functions: Default::default(),
            error_shows: vec![],
            const_function_groups: vec![],
            has_scopes: false,
        },
        suspensions: Default::default(),
        closures: Default::default(),
        const_groups: vec![],
        runtime_consts: Default::default(),
        deferred_pool: false,
        emitted_functions: Default::default(),
        discarded_results: Default::default(),
        const_outcomes: Default::default(),
    }
}
pub(crate) fn planned(module: &LoweredModule) -> PlannedModule<'_> {
    let shapes = TypeShapes {
        core: &module.lowered.core,
        types: module.lowered.core.types.clone(),
        aggregates: Default::default(),
        aggregate_ids: Default::default(),
        recursive_edges: Default::default(),
        expanding_enums: Default::default(),
        recursive_box_types: Default::default(),
        recursive_box_ids: Default::default(),
        list_types: Default::default(),
        list_ids: Default::default(),
        function_types: Default::default(),
        closure_function_types: Default::default(),
        cell_types: Default::default(),
        cell_ids: Default::default(),
        function_adapters: Default::default(),
        function_adapter_ids: Default::default(),
        handler_layouts: Default::default(),
        handler_layout_ids: Default::default(),
        any_box_types: Default::default(),
        any_box_ids: Default::default(),
    };
    let layouts = LayoutBuilder {
        shapes,
        functions: FunctionAbis::default(),
        runtime: RuntimeValues::default(),
        properties: None,
    }
    .finish(LayoutProperties {
        types: vec![],
        cell_cycles: false,
        error_declarations: Default::default(),
    });
    PlannedModule {
        metadata: Default::default(),
        aliases: Default::default(),
        const_emergency_bytes: Default::default(),
        options: Options::default(),
        services: Default::default(),
        suspensions: Default::default(),
        input: module,
        layouts,
        external_bindings: Default::default(),
        const_bridges: Default::default(),
        derived_plan: Default::default(),
        emitted_functions: Default::default(),
        heap_functions: Default::default(),
        string_output_functions: Default::default(),
        const_outcomes: Default::default(),
        names: Default::default(),
        public_names: Default::default(),
        strings: Default::default(),
        function_ref_adapters: Default::default(),
        main_entry: Default::default(),
        has_scheduler: Default::default(),
        has_tasks: Default::default(),
        has_failing_tasks: Default::default(),
        task_value_types: Default::default(),
        runtime: Default::default(),
    }
}
